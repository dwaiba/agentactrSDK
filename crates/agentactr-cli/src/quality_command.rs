use crate::{
    append_run_event, configured_repo_inspection, create_dir, current_epoch_millis,
    load_agentactr_config, load_run_artifact_context, print_quality_plan, print_repo_inspection,
    terminate_child, validate_run_worktree_scope, write_file, RunEventContext,
};
use agentactr_sdk::RepoInspection;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) fn write_quality_status(
    report_path: &Path,
    success: bool,
    failed_reason: Option<&str>,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "schema_version": "0.1",
        "success": success,
        "report_path": report_path.display().to_string(),
        "failed_reason": failed_reason,
    });
    write_file(
        agentactr_sdk::quality_status_path(report_path),
        &serde_json::to_string_pretty(&payload)
            .map_err(|e| format!("render quality status: {e}"))?,
    )
}

pub(crate) struct QualityCommandOutput {
    pub(crate) executed_command: String,
    pub(crate) status: String,
    pub(crate) success: bool,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

struct TimedCommandOutput {
    status: Option<std::process::ExitStatus>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

impl TimedCommandOutput {
    fn status_text(&self) -> String {
        if self.timed_out {
            format!("timed out after {}s", quality_gate_timeout().as_secs())
        } else {
            self.status
                .map(|status| status.to_string())
                .unwrap_or_else(|| "terminated without exit status".to_string())
        }
    }

    fn success(&self) -> bool {
        !self.timed_out && self.status.is_some_and(|status| status.success())
    }
}

pub(crate) fn run_quality_command(
    name: &str,
    command: &str,
    worktree: &Path,
) -> Result<QualityCommandOutput, String> {
    if let Some(output) = run_go_logical_quality_command(command, worktree)? {
        return Ok(output);
    }

    let mut process = Command::new("sh");
    process.arg("-lc").arg(command).current_dir(worktree);
    let output = run_command_with_timeout(&mut process, name, quality_gate_timeout())?;
    Ok(QualityCommandOutput {
        executed_command: command.to_string(),
        status: output.status_text(),
        success: output.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn quality_gate_timeout() -> Duration {
    Duration::from_secs(30 * 60)
}

fn run_command_with_timeout(
    command: &mut Command,
    name: &str,
    timeout: Duration,
) -> Result<TimedCommandOutput, String> {
    configure_quality_process_group(command);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("start quality gate `{name}`: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("quality gate `{name}` stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("quality gate `{name}` stderr unavailable"))?;
    let stdout_thread = thread::spawn(move || read_stream_to_string(stdout));
    let stderr_thread = thread::spawn(move || read_stream_to_string(stderr));
    let (status, timed_out) = wait_quality_child(&mut child, timeout)?;
    let stdout = stdout_thread
        .join()
        .map_err(|_| format!("quality gate `{name}` stdout reader panicked"))??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| format!("quality gate `{name}` stderr reader panicked"))??;
    Ok(TimedCommandOutput {
        status,
        stdout,
        stderr,
        timed_out,
    })
}

fn read_stream_to_string(mut stream: impl Read) -> Result<String, String> {
    let mut output = Vec::new();
    stream
        .read_to_end(&mut output)
        .map_err(|e| format!("read command stream: {e}"))?;
    Ok(String::from_utf8_lossy(&output).to_string())
}

fn wait_quality_child(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<(Option<std::process::ExitStatus>, bool), String> {
    let start = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("poll quality gate: {e}"))?
        {
            return Ok((Some(status), false));
        }
        if start.elapsed() >= timeout {
            terminate_child(child, Duration::from_secs(2));
            return Ok((None, true));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn run_gofmt_check(worktree: &Path) -> Result<QualityCommandOutput, String> {
    run_gofmt_check_with_executed_command(worktree, "gofmt -l <go files>".to_string())
}

fn run_gofmt_check_with_executed_command(
    worktree: &Path,
    executed_command: String,
) -> Result<QualityCommandOutput, String> {
    let files = collect_go_files(worktree)?;
    if files.is_empty() {
        return Ok(QualityCommandOutput {
            executed_command,
            status: "skipped: no Go files".to_string(),
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        });
    }
    let mut process = Command::new("gofmt");
    process.arg("-l").args(&files).current_dir(worktree);
    let output = run_command_with_timeout(&mut process, "gofmt", quality_gate_timeout())?;
    let success = output.success() && output.stdout.trim().is_empty();
    let status = if success || output.timed_out {
        output.status_text()
    } else {
        format!("{}; files need gofmt", output.status_text())
    };
    Ok(QualityCommandOutput {
        executed_command,
        status,
        success,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn run_go_mod_tidy_check(worktree: &Path) -> Result<QualityCommandOutput, String> {
    run_go_mod_tidy_check_for_module(worktree, worktree, ".".to_string())
}

fn run_go_mod_tidy_check_for_module(
    repo_root: &Path,
    module_root: &Path,
    module_display: String,
) -> Result<QualityCommandOutput, String> {
    let executed_command = if module_display == "." {
        "go mod tidy in temporary copy".to_string()
    } else {
        format!("cd {module_display} && go mod tidy in temporary copy")
    };
    if !module_root.join("go.mod").exists() {
        return Ok(QualityCommandOutput {
            executed_command,
            status: "failed: go.mod missing".to_string(),
            success: false,
            stdout: String::new(),
            stderr: "go mod tidy-check requires go.mod".to_string(),
        });
    }
    let temp_root = env::temp_dir().join(format!(
        "agentactr-go-tidy-check-{}-{}",
        std::process::id(),
        current_epoch_millis()
    ));
    let copy_result = copy_worktree_for_quality_check(repo_root, &temp_root);
    if let Err(err) = copy_result {
        let _ = fs::remove_dir_all(&temp_root);
        return Err(err);
    }
    let module_rel = module_root
        .strip_prefix(repo_root)
        .map_err(|e| {
            let _ = fs::remove_dir_all(&temp_root);
            format!(
                "strip module root {} from repo root {}: {e}",
                module_root.display(),
                repo_root.display()
            )
        })?
        .to_path_buf();
    let temp_module_root = temp_root.join(&module_rel);
    let mut process = Command::new("go");
    process
        .arg("mod")
        .arg("tidy")
        .current_dir(&temp_module_root);
    let output =
        match run_command_with_timeout(&mut process, "go mod tidy-check", quality_gate_timeout()) {
            Ok(output) => output,
            Err(err) => {
                let _ = fs::remove_dir_all(&temp_root);
                return Err(err);
            }
        };
    let changed = module_file_changed(module_root, &temp_module_root, "go.mod")
        || module_file_changed(module_root, &temp_module_root, "go.sum");
    let success = output.success() && !changed;
    let status = if success || output.timed_out {
        output.status_text()
    } else if changed {
        format!("{}; go.mod/go.sum would change", output.status_text())
    } else {
        output.status_text()
    };
    let _ = fs::remove_dir_all(&temp_root);
    Ok(QualityCommandOutput {
        executed_command,
        status,
        success,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn run_go_logical_quality_command(
    command: &str,
    worktree: &Path,
) -> Result<Option<QualityCommandOutput>, String> {
    match command {
        "gofmt-check" => return run_gofmt_check(worktree).map(Some),
        "go mod tidy-check" => return run_go_mod_tidy_check(worktree).map(Some),
        _ => {}
    }

    let Some((scope, logical_command)) = parse_scoped_go_logical_command(command)? else {
        return Ok(None);
    };
    let scoped_worktree = resolve_scoped_quality_dir(worktree, &scope)?;
    let scoped_prefix = format!("cd {scope}");
    match logical_command {
        "gofmt-check" => run_gofmt_check_with_executed_command(
            &scoped_worktree,
            format!("{scoped_prefix} && gofmt -l <go files>"),
        )
        .map(Some),
        "go mod tidy-check" => {
            run_go_mod_tidy_check_for_module(worktree, &scoped_worktree, scope).map(Some)
        }
        _ => Ok(None),
    }
}

fn parse_scoped_go_logical_command(
    command: &str,
) -> Result<Option<(String, &'static str)>, String> {
    let Some((cd_part, logical_command)) = command
        .strip_suffix(" && gofmt-check")
        .map(|prefix| (prefix, "gofmt-check"))
        .or_else(|| {
            command
                .strip_suffix(" && go mod tidy-check")
                .map(|prefix| (prefix, "go mod tidy-check"))
        })
    else {
        return Ok(None);
    };
    let Some(scope_word) = cd_part.strip_prefix("cd ") else {
        return Ok(None);
    };
    let scope = unquote_shell_word(scope_word)
        .ok_or_else(|| format!("unsupported scoped Go quality command `{command}`"))?;
    Ok(Some((scope, logical_command)))
}

fn unquote_shell_word(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-'))
    {
        return Some(value.to_string());
    }

    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\'' {
            loop {
                match chars.next() {
                    Some('\'') => break,
                    Some(quoted) => output.push(quoted),
                    None => return None,
                }
            }
        } else if ch == '\\' && chars.peek() == Some(&'\'') {
            chars.next();
            output.push('\'');
        } else {
            return None;
        }
    }
    Some(output)
}

fn resolve_scoped_quality_dir(worktree: &Path, scope: &str) -> Result<PathBuf, String> {
    let relative = Path::new(scope);
    if scope.trim().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!("invalid scoped Go quality directory `{scope}`"));
    }
    let canonical_worktree = fs::canonicalize(worktree)
        .map_err(|e| format!("canonicalize worktree {}: {e}", worktree.display()))?;
    let scoped = fs::canonicalize(worktree.join(relative)).map_err(|e| {
        format!(
            "canonicalize scoped Go quality directory {}: {e}",
            worktree.join(relative).display()
        )
    })?;
    if !scoped.starts_with(&canonical_worktree) {
        return Err(format!(
            "scoped Go quality directory `{scope}` escapes worktree {}",
            worktree.display()
        ));
    }
    Ok(scoped)
}

fn collect_go_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_go_files_inner(root, root, &mut files)?;
    Ok(files)
}

fn collect_go_files_inner(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read {} entry: {e}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("read {} type: {e}", path.display()))?;
        if file_type.is_dir() {
            if should_skip_quality_dir(&path) {
                continue;
            }
            collect_go_files_inner(root, &path, files)?;
        } else if file_type.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("go")
        {
            files.push(
                path.strip_prefix(root)
                    .map_err(|e| format!("strip {} prefix: {e}", path.display()))?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

fn should_skip_quality_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|value| value.to_str()),
        Some(".git" | ".agentactr" | "target" | "node_modules")
    )
}

fn copy_worktree_for_quality_check(source: &Path, target: &Path) -> Result<(), String> {
    create_dir(target)?;
    copy_worktree_dir(source, source, target)
}

fn copy_worktree_dir(root: &Path, source: &Path, target_root: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|e| format!("read {}: {e}", source.display()))? {
        let entry = entry.map_err(|e| format!("read {} entry: {e}", source.display()))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|e| format!("strip {} prefix: {e}", path.display()))?;
        if should_skip_quality_dir(&path) {
            continue;
        }
        let target = target_root.join(relative);
        let file_type = entry
            .file_type()
            .map_err(|e| format!("read {} type: {e}", path.display()))?;
        if file_type.is_dir() {
            create_dir(&target)?;
            copy_worktree_dir(root, &path, target_root)?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                create_dir(parent)?;
            }
            fs::copy(&path, &target)
                .map_err(|e| format!("copy {} to {}: {e}", path.display(), target.display()))?;
        }
    }
    Ok(())
}

fn module_file_changed(original_root: &Path, temp_root: &Path, file: &str) -> bool {
    let original_path = original_root.join(file);
    let temp_path = temp_root.join(file);
    let original = fs::read(&original_path).ok();
    let temp = fs::read(&temp_path).ok();
    original != temp
}

pub(crate) fn run_quality_gates_to_report(
    inspection: &RepoInspection,
    worktree: &Path,
    report_path: &Path,
    domain_gate_opt_ins: &[String],
) -> Result<(), String> {
    let effective = inspection;
    let mut report = String::new();
    report.push_str(&format!(
        "stack={}\nconfidence={}\nworktree={}\n\n",
        effective.primary_stack.as_str(),
        effective.confidence,
        worktree.display()
    ));
    if effective.quality_plan.is_empty() && effective.domain_quality_plan.is_empty() {
        report.push_str("quality_plan=empty\n");
        write_file(report_path, &report)?;
        let err = "strict quality gate failed: no quality plan for detected stack".to_string();
        write_quality_status(report_path, false, Some(&err))?;
        return Err(err);
    }
    let typed_language_gates_present = effective
        .domain_quality_plan
        .iter()
        .any(|gate| gate.domain.starts_with("language.") && gate.command.as_deref().is_some());
    if typed_language_gates_present && !effective.quality_plan.is_empty() {
        report.push_str("legacy_stack_quality_plan=migrated_to_typed_domain_gates\n\n");
        write_file(report_path, &report)?;
    } else {
        for cmd in &effective.quality_plan {
            println!("quality gate: {} -> {}", cmd.name, cmd.command);
            let output = match run_quality_command(&cmd.name, &cmd.command, worktree) {
                Ok(output) => output,
                Err(err) => {
                    write_quality_status(report_path, false, Some(&err))?;
                    return Err(err);
                }
            };
            report.push_str(&format!(
                "## {}\ncommand={}\nstatus={}\nrequired={}\nfinal_gate={}\n\nstdout:\n{}\n\nstderr:\n{}\n\n",
                cmd.name,
                output.executed_command,
                output.status,
                cmd.required,
                cmd.non_mutating_final_gate,
                output.stdout,
                output.stderr
            ));
            write_file(report_path, &report)?;
            if cmd.required && !output.success {
                let report_ref = repo_relative_agentactr_path(report_path);
                let network_guidance =
                    quality_network_failure_guidance(&cmd.name, &output.stdout, &output.stderr);
                if let Some(guidance) = network_guidance.as_ref() {
                    report.push_str("\nnetwork_guidance:\n");
                    report.push_str(guidance);
                    report.push('\n');
                    write_file(report_path, &report)?;
                }
                let mut err = format!(
                    "strict quality gate failed: {} exited with {}; report={}",
                    cmd.name, output.status, report_ref
                );
                if let Some(guidance) = network_guidance {
                    err.push_str("; ");
                    err.push_str(&guidance);
                }
                write_quality_status(report_path, false, Some(&err))?;
                return Err(err);
            }
        }
    }
    for gate in &effective.domain_quality_plan {
        let Some(command) = gate.command.as_deref() else {
            report.push_str(&format!(
                "## domain:{}\ndomain={}\ntool={}\nstatus=finding-only\nrequired={}\nsetup_guidance={}\n\n",
                gate.name,
                gate.domain,
                gate.tool,
                gate.required,
                gate.setup_guidance.join(" | ")
            ));
            write_file(report_path, &report)?;
            continue;
        };
        let enabled_by_config = domain_gate_enabled_by_config(gate, domain_gate_opt_ins);
        if (gate.opt_in_required || gate.network_required || gate.credential_required)
            && !enabled_by_config
        {
            println!(
                "domain quality gate skipped: {} -> {} (opt_in={} network={} credentials={})",
                gate.name,
                command,
                gate.opt_in_required,
                gate.network_required,
                gate.credential_required
            );
            report.push_str(&format!(
                "## domain:{}\ndomain={}\ntool={}\ncommand={}\nstatus=skipped\nrequired={}\nopt_in_required={}\nnetwork_required={}\ncredential_required={}\nenabled_by_config=false\nsetup_guidance={}\n\n",
                gate.name,
                gate.domain,
                gate.tool,
                command,
                gate.required,
                gate.opt_in_required,
                gate.network_required,
                gate.credential_required,
                gate.setup_guidance.join(" | ")
            ));
            write_file(report_path, &report)?;
            continue;
        }
        println!("domain quality gate: {} -> {}", gate.name, command);
        let output = match run_quality_command(&gate.name, command, worktree) {
            Ok(output) => output,
            Err(err) => {
                write_quality_status(report_path, false, Some(&err))?;
                return Err(err);
            }
        };
        report.push_str(&format!(
            "## domain:{}\ndomain={}\ntool={}\ncommand={}\nstatus={}\nrequired={}\nmutates={}\nenabled_by_config={}\n\nstdout:\n{}\n\nstderr:\n{}\n\n",
            gate.name,
            gate.domain,
            gate.tool,
            output.executed_command,
            output.status,
            gate.required,
            gate.mutates,
            enabled_by_config,
            output.stdout,
            output.stderr
        ));
        write_file(report_path, &report)?;
        if gate.required && !output.success {
            let report_ref = repo_relative_agentactr_path(report_path);
            let mut err = format!(
                "strict domain quality gate failed: {} exited with {}; report={}",
                gate.name, output.status, report_ref
            );
            if gate.degraded_if_missing {
                err.push_str("; domain governance is degraded until the required toolchain/configuration is present");
            }
            write_quality_status(report_path, false, Some(&err))?;
            return Err(err);
        }
    }
    write_quality_status(report_path, true, None)?;
    Ok(())
}

fn domain_gate_enabled_by_config(
    gate: &agentactr_sdk::DomainQualityGate,
    opt_ins: &[String],
) -> bool {
    let gate_key = format!("{}:{}", gate.domain, gate.name);
    opt_ins.iter().any(|value| {
        let normalized = value.trim().to_ascii_lowercase();
        normalized == "all"
            || normalized == gate.domain
            || normalized == gate.name
            || normalized == gate_key
            || normalized == format!("{}:*", gate.domain)
    })
}

fn repo_relative_agentactr_path(path: &Path) -> String {
    let components: Vec<_> = path.components().collect();
    for (idx, component) in components.iter().enumerate() {
        if component.as_os_str() == ".agentactr" {
            let mut out = PathBuf::new();
            for component in &components[idx..] {
                out.push(component.as_os_str());
            }
            return out.display().to_string();
        }
    }
    path.display().to_string()
}

fn quality_network_failure_guidance(name: &str, stdout: &str, stderr: &str) -> Option<String> {
    let output = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    let network_markers = [
        "connectionrefused",
        "failedtoopensocket",
        "failed to open socket",
        "could not resolve",
        "couldn't resolve",
        "temporary failure in name resolution",
        "name or service not known",
        "network is unreachable",
        "failed to connect",
        "econnrefused",
        "enotfound",
        "etimedout",
        "timed out connecting",
        "dial tcp",
        "lookup ",
        "unable to access 'https://",
        "error sending request",
        "download of config.json failed",
        "downloading package manifest",
    ];
    if !network_markers.iter().any(|marker| output.contains(marker)) {
        return None;
    }
    Some(format!(
        "quality gate `{name}` appears to require network access; fail-closed/non-interactive runs do not request approval. Preinstall/cache dependencies or rerun with `--human-intervention interactive --codex-approval on-request`."
    ))
}

pub(crate) fn cmd_quality(args: &mut [String]) -> Result<(), String> {
    if args.get(1).map(String::as_str) == Some("run") {
        let run_id = args.get(2).ok_or("usage: agentactr quality run RUN_ID")?;
        let config = load_agentactr_config(None)?;
        let mut context = load_run_artifact_context(&config, run_id)?;
        context.worktree = validate_run_worktree_scope(&config, &context)?;
        let inspection = configured_repo_inspection(&context.worktree, &config);
        let ts = current_epoch_millis();
        let report_path = context
            .artifact_dir
            .join(format!("quality_report.rerun.{ts}.txt"));
        let result = run_quality_gates_to_report(
            &inspection,
            &context.worktree,
            &report_path,
            &config.quality.domain_gate_opt_ins,
        );
        let event_type = if result.is_ok() {
            "quality.rerun.completed"
        } else {
            "quality.rerun.failed"
        };
        append_run_event(
            &RunEventContext::root(&config, &context.run_id, &context.repo, &context.issue),
            event_type,
            serde_json::json!({
                "worktree": context.worktree.display().to_string(),
                "report": report_path.display().to_string(),
                "selected_stack": inspection.primary_stack.as_str(),
                "quality_gate_count": inspection.quality_plan.len(),
                "error": result.as_ref().err().cloned(),
            }),
        )?;
        return match result {
            Ok(()) => {
                println!("quality report: {}", report_path.display());
                Ok(())
            }
            Err(err) => Err(err),
        };
    }
    if args.get(1).map(String::as_str) != Some("plan") {
        return Err("usage: agentactr quality plan | quality run RUN_ID".to_string());
    }
    let config = load_agentactr_config(None)?;
    let inspection = configured_repo_inspection(Path::new("."), &config);
    print_repo_inspection(&inspection);
    if inspection.quality_plan.is_empty() {
        println!("quality plan: none; supported stack was not detected");
    } else {
        print_quality_plan(&inspection);
    }
    Ok(())
}

#[cfg(unix)]
fn configure_quality_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `pre_exec` runs in the child after fork and before exec. The
    // closure only calls the async-signal-safe `setpgid(0, 0)` syscall and
    // converts errno into `std::io::Error`; it does not allocate, lock, or
    // access shared Rust state.
    unsafe {
        command.pre_exec(|| {
            unsafe extern "C" {
                fn setpgid(pid: i32, pgid: i32) -> i32;
            }
            if setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_quality_process_group(_command: &mut Command) {}

#[cfg(unix)]
pub(crate) fn terminate_process_group(child: &std::process::Child, signal: &str) {
    let _ = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(format!("-{}", child.id()))
        .status();
}

#[cfg(not(unix))]
pub(crate) fn terminate_process_group(_child: &std::process::Child, _signal: &str) {}

#[cfg(unix)]
pub(crate) fn quality_process_group_alive(process_group_id: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(format!("-{process_group_id}"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(crate) fn quality_process_group_alive(_process_group_id: u32) -> bool {
    false
}
