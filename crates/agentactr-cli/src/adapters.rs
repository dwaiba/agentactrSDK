use crate::artifacts::sha256_hex_bytes;
use crate::linux_memory::{
    attach_pid_to_cgroup, preserve_memory_debug_bundle, start_memory_monitor, ActiveMemoryRegistry,
    MemoryMonitor, MemoryMonitorSubject, MemoryTraceContext,
};
use crate::{current_epoch_millis, iso_timestamp_from_epoch_millis};
pub(crate) use agentactr_codex::{
    append_codex_project_profile_overrides, CodexAppServerAdapter, CodexRuntimeAdapter,
    CodexSdkAdapter,
};
use agentactr_codex::{CodexMemoryMonitor, CodexMemorySupervisor};
use agentactr_sdk::{
    AdapterCapabilities, AdapterVersionReport, AgentactrConfig, CandidateQuery, ClaimRequest,
    ClaimResult, CommentRef, CommentRequest, CommitRef, CommitRequest, GithubConfig, Issue,
    IssueAppliedMetadata, IssueCommentKind, IssueCreateRequest, IssueCreateResult, IssueId,
    IssueLinkRequest, IssueLinkResult, IssueProjectFieldValue, IssueRequestedMetadata,
    IssueTracker, MemoryGroupId, MergePlan, MergePlanRequest, ReleaseRequest, ReleaseResult,
    RuntimeProcessEvent, RuntimeProcessEventKind, TrackerConfig, VcsCapabilities, VcsConfig,
    VersionControl, WorkspaceDiff, WorktreeRef, WorktreeRequest,
};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::warn;

#[derive(Clone, Debug)]
struct GithubIssueMutationResult {
    labels: Vec<String>,
    state: String,
    state_reason: Option<String>,
    artifact: PathBuf,
}

#[derive(Clone, Debug)]
struct GithubStandardLabel {
    name: &'static str,
    color: &'static str,
    description: &'static str,
}

const GITHUB_STANDARD_LABELS: &[GithubStandardLabel] = &[
    GithubStandardLabel {
        name: "bug",
        color: "d73a4a",
        description: "Something is not working",
    },
    GithubStandardLabel {
        name: "dependencies",
        color: "0366d6",
        description: "Dependency update or maintenance",
    },
    GithubStandardLabel {
        name: "documentation",
        color: "0075ca",
        description: "Documentation updates",
    },
    GithubStandardLabel {
        name: "duplicate",
        color: "cfd3d7",
        description: "Duplicate issue",
    },
    GithubStandardLabel {
        name: "enhancement",
        color: "a2eeef",
        description: "New feature or improvement",
    },
    GithubStandardLabel {
        name: "go",
        color: "00add8",
        description: "Go implementation work",
    },
    GithubStandardLabel {
        name: "good first issue",
        color: "7057ff",
        description: "Good entry point for contributors",
    },
    GithubStandardLabel {
        name: "help wanted",
        color: "008672",
        description: "Extra attention is requested",
    },
    GithubStandardLabel {
        name: "invalid",
        color: "e4e669",
        description: "Invalid or unsupported request",
    },
    GithubStandardLabel {
        name: "python:uv",
        color: "3776ab",
        description: "Python uv toolchain work",
    },
    GithubStandardLabel {
        name: "question",
        color: "d876e3",
        description: "Further information is requested",
    },
    GithubStandardLabel {
        name: "tool",
        color: "5319e7",
        description: "Tooling and automation work",
    },
    GithubStandardLabel {
        name: "wontfix",
        color: "ffffff",
        description: "This will not be worked",
    },
];

#[derive(Clone, Debug)]
struct GithubProjectField {
    id: String,
    name: String,
    options: Vec<GithubProjectFieldOption>,
}

#[derive(Clone, Debug)]
struct GithubProjectFieldOption {
    id: String,
    name: String,
}

#[derive(Clone, Debug)]
struct GithubProject {
    id: String,
    title: String,
    number: u32,
    fields: Vec<GithubProjectField>,
}

#[derive(Clone, Debug)]
struct GithubProjectOwner {
    id: String,
}

#[derive(Clone, Debug)]
struct GithubProjectAutomationResult {
    applied_fields: Vec<IssueProjectFieldValue>,
    artifacts: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
struct GithubClaimMarker {
    schema_version: String,
    run_id: String,
    owner_id: String,
    fencing_token: String,
    expires_at: String,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct GithubLifecycleCommentMarker {
    schema_version: String,
    run_id: String,
    kind: String,
    key: String,
    digest: String,
}

pub(crate) fn codex_runtime_adapter(
    config: &AgentactrConfig,
) -> Result<CodexRuntimeAdapter, String> {
    CodexRuntimeAdapter::new(&config.codex).map(|adapter| {
        adapter.with_process_execution(config.execution.clone(), config.linux_memory.clone())
    })
}

#[derive(Clone)]
pub(crate) struct CliCodexMemorySupervisor {
    memory_groups: Arc<std::collections::HashMap<MemoryGroupId, PathBuf>>,
    active_memory: ActiveMemoryRegistry,
    artifact_dir: PathBuf,
    trace_path: PathBuf,
    repo: String,
    issue: String,
}

impl CliCodexMemorySupervisor {
    pub(crate) fn new(
        memory_groups: std::collections::HashMap<MemoryGroupId, PathBuf>,
        artifact_dir: PathBuf,
        trace_path: PathBuf,
        repo: String,
        issue: String,
    ) -> Self {
        Self {
            memory_groups: Arc::new(memory_groups),
            active_memory: ActiveMemoryRegistry::default(),
            artifact_dir,
            trace_path,
            repo,
            issue,
        }
    }
}

struct CliCodexMemoryMonitor(MemoryMonitor);

impl CodexMemoryMonitor for CliCodexMemoryMonitor {
    fn failure(&self) -> Option<String> {
        self.0.failure()
    }

    fn stop(self: Box<Self>) -> Result<(), String> {
        let Self(monitor) = *self;
        monitor.stop()
    }
}

impl CodexMemorySupervisor for CliCodexMemorySupervisor {
    fn observe(&self, event: &RuntimeProcessEvent) -> Result<(), String> {
        persist_runtime_process_event(
            &self.artifact_dir,
            &self.trace_path,
            &self.repo,
            &self.issue,
            event,
        )
    }

    fn start(
        &self,
        event: &RuntimeProcessEvent,
        artifact_dir: &Path,
    ) -> Result<Option<Box<dyn CodexMemoryMonitor>>, String> {
        let root_pid = event
            .attribution
            .root_pid
            .ok_or("runtime process event did not include a root pid")?
            .0;
        let group_id = event
            .attribution
            .memory_group_id
            .as_ref()
            .ok_or("runtime process event did not include a memory group id")?;
        let cgroup = self.memory_groups.get(group_id).ok_or_else(|| {
            format!(
                "memory group {} is not registered in this runtime process supervisor",
                group_id.as_str()
            )
        })?;
        attach_pid_to_cgroup(cgroup, root_pid)?;
        let subject = MemoryMonitorSubject {
            group_id: Some(group_id.clone()),
            agent_run_id: event.agent_run_id.clone(),
            runtime_event: event.clone(),
            read_only_helper: event.attribution.parent_agent_run_id.is_some(),
            priority: if event.attribution.parent_agent_run_id.is_some() {
                10
            } else {
                0
            },
        };
        start_memory_monitor(
            cgroup,
            root_pid,
            artifact_dir,
            Some(subject),
            Some(self.active_memory.clone()),
            Some(MemoryTraceContext {
                trace_path: self.trace_path.clone(),
                repo: self.repo.clone(),
                issue: self.issue.clone(),
            }),
            Some(Arc::new(self.clone())),
        )
        .map(|monitor| {
            monitor.map(|monitor| {
                Box::new(CliCodexMemoryMonitor(monitor)) as Box<dyn CodexMemoryMonitor>
            })
        })
    }

    fn preserve_debug_bundle(
        &self,
        event: Option<&RuntimeProcessEvent>,
        artifact_dir: &Path,
        reason: &str,
    ) -> Result<(), String> {
        let root_pid = event
            .and_then(|event| event.attribution.root_pid)
            .map(|pid| pid.0);
        let cgroup = event
            .and_then(|event| event.attribution.memory_group_id.as_ref())
            .and_then(|group_id| self.memory_groups.get(group_id))
            .map(PathBuf::as_path);
        preserve_memory_debug_bundle(artifact_dir, cgroup, root_pid, reason).map(|_| ())
    }

    fn cancel_process_tree(
        &self,
        event: &RuntimeProcessEvent,
        reason: &str,
    ) -> Result<String, String> {
        let root_pid = event
            .attribution
            .root_pid
            .ok_or("runtime process cancellation requires root pid")?
            .0;
        let signal_target = event
            .attribution
            .process_group_id
            .map(|pgid| format!("-{}", pgid.0.abs()))
            .unwrap_or_else(|| root_pid.to_string());
        let process_group_id = event.attribution.process_group_id.map(|pgid| pgid.0);
        if !runtime_target_alive(root_pid, process_group_id) {
            return Ok(format!(
                "runtime process {} already exited before cancellation for {reason}",
                root_pid
            ));
        }
        send_runtime_signal("TERM", &signal_target)?;
        if wait_for_runtime_exit(root_pid, process_group_id, Duration::from_secs(2)) {
            return Ok(format!(
                "runtime process group {signal_target} exited after SIGTERM for {reason}"
            ));
        }
        send_runtime_signal("KILL", &signal_target)?;
        if wait_for_runtime_exit(root_pid, process_group_id, Duration::from_secs(2)) {
            return Ok(format!(
                "runtime process group {signal_target} exited after SIGKILL for {reason}"
            ));
        }
        Err(format!(
            "runtime process group {signal_target} did not exit after SIGTERM/SIGKILL for {reason}"
        ))
    }
}

fn persist_runtime_process_event(
    artifact_dir: &Path,
    trace_path: &Path,
    repo: &str,
    issue: &str,
    event: &RuntimeProcessEvent,
) -> Result<(), String> {
    let payload = runtime_process_payload(event);
    write_jsonl_event(
        &artifact_dir.join("runtime_process_events.jsonl"),
        &payload,
        "runtime process artifact",
    )?;
    let event_type = format!("runtime.process.{}", runtime_process_kind(event.kind));
    let parent_agent_run_id = event
        .attribution
        .parent_agent_run_id
        .as_ref()
        .map(|agent| agent.as_str());
    let span_id = runtime_process_span_id(event.run_id.as_str(), event.agent_run_id.as_str());
    let parent_span_id = parent_agent_run_id
        .map(|agent_run_id| runtime_process_span_id(event.run_id.as_str(), agent_run_id));
    let ts_unix_ms = current_epoch_millis();
    let trace_event = serde_json::json!({
        "schema_version": "0.1",
        "ts": iso_timestamp_from_epoch_millis(ts_unix_ms),
        "ts_unix_ms": ts_unix_ms,
        "run_id": event.run_id.as_str(),
        "issue_id": format!("github:{repo}#{issue}"),
        "agent_run_id": event.agent_run_id.as_str(),
        "parent_agent_run_id": parent_agent_run_id,
        "event_type": event_type,
        "span_id": span_id,
        "parent_span_id": parent_span_id,
        "payload": payload,
    });
    write_jsonl_event(trace_path, &trace_event, "runtime process trace")
}

fn write_jsonl_event(path: &Path, value: &serde_json::Value, label: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open {label} {}: {e}", path.display()))?;
    writeln!(file, "{value}").map_err(|e| format!("write {label} {}: {e}", path.display()))
}

fn send_runtime_signal(signal: &str, target: &str) -> Result<(), String> {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(target)
        .status()
        .map_err(|e| format!("send SIG{signal} to runtime target {target}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "send SIG{signal} to runtime target {target} exited with {status}"
        ))
    }
}

fn wait_for_runtime_exit(root_pid: u32, process_group_id: Option<i64>, grace: Duration) -> bool {
    let start = SystemTime::now();
    loop {
        if !runtime_target_alive(root_pid, process_group_id) {
            return true;
        }
        let elapsed = start.elapsed().unwrap_or_default();
        if elapsed >= grace {
            return false;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn runtime_target_alive(root_pid: u32, process_group_id: Option<i64>) -> bool {
    if let Some(process_group_id) = process_group_id {
        runtime_process_group_alive(process_group_id)
    } else {
        runtime_pid_alive(root_pid)
    }
}

#[cfg(unix)]
fn runtime_process_group_alive(process_group_id: i64) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(format!("-{}", process_group_id.abs()))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn runtime_process_group_alive(_process_group_id: i64) -> bool {
    false
}

fn runtime_pid_alive(root_pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/proc").join(root_pid.to_string()).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Command::new("kill")
            .arg("-0")
            .arg(root_pid.to_string())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

fn runtime_process_payload(event: &RuntimeProcessEvent) -> serde_json::Value {
    let attribution = &event.attribution;
    serde_json::json!({
        "kind": runtime_process_kind(event.kind),
        "run_id": event.run_id.as_str(),
        "agent_run_id": event.agent_run_id.as_str(),
        "parent_agent_run_id": attribution.parent_agent_run_id.as_ref().map(|agent| agent.as_str()),
        "runtime_kind": attribution.runtime_kind.as_str(),
        "transport_kind": attribution.transport_kind.as_str(),
        "process_model": format!("{:?}", attribution.process_model),
        "root_pid": attribution.root_pid.map(|pid| pid.0),
        "child_pids": attribution.child_pids.iter().map(|pid| pid.0).collect::<Vec<_>>(),
        "process_group_id": attribution.process_group_id.map(|pgid| pgid.0),
        "container_ref": attribution.container_ref.as_ref().map(|value| value.as_str()),
        "vm_ref": attribution.vm_ref.as_ref().map(|value| value.as_str()),
        "memory_group_id": attribution.memory_group_id.as_ref().map(MemoryGroupId::as_str),
        "identity_ref": attribution.identity_ref.as_ref().map(|value| value.as_str()),
    })
}

fn runtime_process_kind(kind: RuntimeProcessEventKind) -> &'static str {
    match kind {
        RuntimeProcessEventKind::Started => "started",
        RuntimeProcessEventKind::Attributed => "attributed",
        RuntimeProcessEventKind::ChildDiscovered => "child_discovered",
        RuntimeProcessEventKind::Terminated => "terminated",
    }
}

fn runtime_process_span_id(run_id: &str, agent_run_id: &str) -> String {
    format!("span:{run_id}:{agent_run_id}:runtime.process")
}

pub(crate) struct LocalGitAdapter;

impl LocalGitAdapter {
    pub(crate) fn prepare_worktree(
        &self,
        run_id: &str,
        repo: &str,
        issue: &str,
        config: &VcsConfig,
    ) -> Result<PathBuf, String> {
        self.prepare_worktree_ref(WorktreeRequest {
            run_id: run_id.to_string(),
            repo: repo.to_string(),
            issue: issue.to_string(),
            base_ref: config.base_ref.clone(),
            worktree_root: PathBuf::from(&config.worktree_root),
            branch_template: config.branch_template.clone(),
            fail_on_dirty_source_checkout: config.fail_on_dirty_source_checkout,
            copy_runtime_config_to_worktree: config.copy_runtime_config_to_worktree,
        })
        .map(|worktree| worktree.path)
    }

    pub(crate) fn preflight_source_checkout(&self, config: &VcsConfig) -> Result<(), String> {
        resolve_base_commit(&config.base_ref)?;
        if config.fail_on_dirty_source_checkout {
            ensure_clean_git_checkout()?;
        }
        Ok(())
    }

    fn prepare_worktree_ref(&self, req: WorktreeRequest) -> Result<WorktreeRef, String> {
        let base_ref = req.base_ref.clone();
        let base_commit = resolve_base_commit(&req.base_ref)?;
        let source_checkout_clean_at_prepare =
            git_output(&["status", "--porcelain"])?.trim().is_empty();
        if req.fail_on_dirty_source_checkout {
            ensure_clean_git_checkout()?;
        }
        create_dir(&req.worktree_root)?;
        let worktree = req.worktree_root.join(&req.run_id);
        if worktree.exists() {
            return Err(format!("worktree already exists: {}", worktree.display()));
        }
        let branch_name =
            render_branch_template(&req.branch_template, &req.repo, &req.issue, &req.run_id);
        let status = Command::new("git")
            .arg("worktree")
            .arg("add")
            .arg("-b")
            .arg(&branch_name)
            .arg(&worktree)
            .arg(base_commit.trim())
            .status()
            .map_err(|e| format!("git worktree add: {e}"))?;
        if !status.success() {
            return Err(format!("git worktree add exited with {status}"));
        }
        let overlaid_runtime_config = if req.copy_runtime_config_to_worktree {
            copy_runtime_config_to_worktree(&worktree)?
        } else {
            Vec::new()
        };
        let git_version = git_output(&["--version"]).unwrap_or_else(|_| "unknown".to_string());
        let overlay_metadata = if overlaid_runtime_config.is_empty() {
            String::new()
        } else {
            format!(
                "runtime_config_overlay = [{}]\n",
                overlaid_runtime_config
                    .iter()
                    .map(|item| format!("\"{}\"", toml_escape(item)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let metadata = format!(
            "run_id = \"{}\"\nbase_ref = \"{}\"\nbase_commit = \"{}\"\nworktree_path = \"{}\"\nbranch_name = \"{}\"\ngit_version = \"{}\"\nsource_checkout_clean_at_prepare = {}\n{}",
            toml_escape(&req.run_id),
            toml_escape(base_ref.trim()),
            base_commit.trim(),
            toml_escape(&worktree.display().to_string()),
            toml_escape(&branch_name),
            toml_escape(git_version.trim()),
            source_checkout_clean_at_prepare,
            overlay_metadata
        );
        write_file(worktree.join(".agentactr-run.toml"), &metadata)?;
        Ok(WorktreeRef {
            path: fs::canonicalize(&worktree).unwrap_or(worktree),
            base_commit: base_commit.trim().to_string(),
            run_id: req.run_id,
        })
    }
}

fn copy_runtime_config_to_worktree(worktree: &Path) -> Result<Vec<String>, String> {
    const RUNTIME_CONFIG_FILES: &[&str] = &["agentactr.toml", ".codex/config.toml", "WORKFLOW.md"];

    let mut copied = Vec::new();
    for relative in RUNTIME_CONFIG_FILES {
        let source = Path::new(relative);
        if !source.exists() {
            continue;
        }
        if !source.is_file() {
            return Err(format!(
                "runtime config overlay source {} is not a file",
                source.display()
            ));
        }
        let target = worktree.join(relative);
        if let Some(parent) = target.parent() {
            create_dir(parent)?;
        }
        fs::copy(source, &target).map_err(|e| {
            format!(
                "copy runtime config {} to {}: {e}",
                source.display(),
                target.display()
            )
        })?;
        copied.push((*relative).to_string());
    }
    Ok(copied)
}

impl VersionControl for LocalGitAdapter {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport {
            adapter_kind: "version_control".to_string(),
            adapter_name: "agentactr-cli-local-git".to_string(),
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            product_name: "git".to_string(),
            product_version: git_output(&["--version"]).unwrap_or_else(|_| "unknown".to_string()),
            api_version: "git-cli".to_string(),
            capability_digest: "detect,status,worktree-add-detach,diff,merge-plan-read-only"
                .to_string(),
            degraded_features: vec!["commit".to_string()],
            required_actions: vec![
                "keep commit and merge behind SDK use cases before enabling finalization"
                    .to_string(),
            ],
            warnings: Vec::new(),
        }
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            adapter_kind: "version_control".to_string(),
            supported_features: vec![
                "source_checkout_preflight".to_string(),
                "isolated_git_worktree".to_string(),
                "base_commit_recording".to_string(),
                "runtime_config_overlay".to_string(),
                "workspace_diff_artifact".to_string(),
                "merge_plan_read_only".to_string(),
            ],
            degraded_features: vec![
                "commit".to_string(),
                "cross_issue_overlap_detection".to_string(),
            ],
            required_actions: vec![
                "keep commit and merge behind SDK use cases before enabling finalization"
                    .to_string(),
            ],
        }
    }

    fn detect(&self, _root: &Path) -> Result<VcsCapabilities, String> {
        git_output(&["status", "--porcelain"])?;
        Ok(VcsCapabilities)
    }

    fn prepare_workspace(&self, req: WorktreeRequest) -> Result<WorktreeRef, String> {
        if req.run_id.trim().is_empty() {
            return Err("worktree request requires run_id".to_string());
        }
        self.prepare_worktree_ref(req)
    }

    fn diff(&self, worktree: &WorktreeRef) -> Result<WorkspaceDiff, String> {
        if worktree.run_id.trim().is_empty() {
            return Err("workspace diff requires run_id".to_string());
        }
        if !worktree.path.is_dir() {
            return Err(format!(
                "workspace diff worktree is missing or not a directory: {}",
                worktree.path.display()
            ));
        }
        let current_commit = git_output_in_worktree(&worktree.path, &["rev-parse", "HEAD"])?;
        let mut patch = git_output_in_worktree_raw(
            &worktree.path,
            &["diff", "--binary", &worktree.base_commit, "--"],
        )?;
        let status = git_output_in_worktree(&worktree.path, &["status", "--porcelain"])?;
        let touched_files = parse_git_status_paths(&status);
        let untracked_files = git_output_in_worktree(
            &worktree.path,
            &["ls-files", "--others", "--exclude-standard"],
        )?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
        for untracked in &untracked_files {
            if !patch.is_empty() && !patch.ends_with('\n') {
                patch.push('\n');
            }
            patch.push_str(&git_new_file_patch(&worktree.path, untracked)?);
        }
        Ok(WorkspaceDiff {
            run_id: worktree.run_id.clone(),
            worktree: worktree.path.clone(),
            base_commit: worktree.base_commit.clone(),
            current_commit,
            patch,
            is_empty: touched_files.is_empty(),
            touched_files,
            untracked_files,
        })
    }

    fn commit(&self, _req: CommitRequest) -> Result<CommitRef, String> {
        Err("commit is not implemented in this milestone".to_string())
    }

    fn merge_plan(&self, req: MergePlanRequest) -> Result<MergePlan, String> {
        if req.worktree.run_id.trim().is_empty() {
            return Err("merge plan requires run_id".to_string());
        }
        if !req.worktree.path.is_dir() {
            return Err(format!(
                "merge plan worktree is missing or not a directory: {}",
                req.worktree.path.display()
            ));
        }
        let current_commit = git_output_in_worktree(&req.worktree.path, &["rev-parse", "HEAD"])?;
        let base_rev = format!("{}^{{commit}}", req.base_ref);
        let base_ref_current_commit =
            git_output_in_worktree(&req.worktree.path, &["rev-parse", "--verify", &base_rev])?;
        let base_ref_drifted = base_ref_current_commit.trim() != req.worktree.base_commit.trim();
        let head_contains_base_ref = git_status_in_worktree(
            &req.worktree.path,
            &[
                "merge-base",
                "--is-ancestor",
                &base_ref_current_commit,
                &current_commit,
            ],
        )?;
        let status = git_output_in_worktree(&req.worktree.path, &["status", "--porcelain"])?;
        let touched_files = parse_git_status_paths(&status);
        let merge_enabled = req.merge_mode != "disabled";
        let workspace_diff_exists = req
            .workspace_diff_artifact
            .as_ref()
            .map(|path| path.is_file())
            .unwrap_or(false);
        let mut blockers = Vec::new();
        if !merge_enabled {
            blockers.push("merge.mode is disabled".to_string());
        }
        if base_ref_drifted {
            blockers.push(format!(
                "base ref {} advanced from {} to {}",
                req.base_ref, req.worktree.base_commit, base_ref_current_commit
            ));
        }
        if !head_contains_base_ref {
            blockers.push(format!(
                "worktree HEAD {} does not contain current base ref {}",
                current_commit, base_ref_current_commit
            ));
        }
        if !workspace_diff_exists {
            blockers.push("workspace diff artifact is missing".to_string());
        }
        let warnings = vec![
            "cross-issue overlap detection is not implemented in this milestone".to_string(),
            "commit and GitHub finalization remain disabled unless implemented separately"
                .to_string(),
        ];
        let recommendation = if blockers.is_empty() {
            "merge_candidate"
        } else {
            "do_not_merge"
        }
        .to_string();
        Ok(MergePlan {
            run_id: req.worktree.run_id,
            worktree: req.worktree.path,
            base_ref: req.base_ref,
            base_commit: req.worktree.base_commit,
            current_commit,
            base_ref_current_commit,
            base_ref_drifted,
            head_contains_base_ref,
            merge_mode: req.merge_mode,
            merge_enabled,
            workspace_diff_artifact: req.workspace_diff_artifact,
            workspace_diff_exists,
            touched_files,
            blockers,
            warnings,
            recommendation,
        })
    }
}

fn render_branch_template(template: &str, repo: &str, issue: &str, run_id: &str) -> String {
    let repo_slug = repo
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    template
        .replace("{repo_slug}", &repo_slug)
        .replace("{issue_number}", issue)
        .replace("{run_id}", run_id)
}

pub(crate) struct GithubRestAdapter {
    artifact_dir: PathBuf,
    token_env: String,
    api_version: String,
    standard_label_policy: String,
    project_automation: String,
    project_owner: String,
    project_number: u32,
    project_title: String,
    project_priority_field: String,
    project_size_field: String,
}

impl GithubRestAdapter {
    pub(crate) fn new(artifact_dir: impl Into<PathBuf>, config: &TrackerConfig) -> Self {
        Self {
            artifact_dir: artifact_dir.into(),
            token_env: config.token_env.clone(),
            api_version: config.github_api_version.clone(),
            standard_label_policy: "ensure_on_issue_create".to_string(),
            project_automation: "disabled".to_string(),
            project_owner: "auto".to_string(),
            project_number: 0,
            project_title: "Agentactr".to_string(),
            project_priority_field: "Priority".to_string(),
            project_size_field: "Size".to_string(),
        }
    }

    pub(crate) fn new_with_github(
        artifact_dir: impl Into<PathBuf>,
        tracker: &TrackerConfig,
        github: &GithubConfig,
    ) -> Self {
        Self {
            artifact_dir: artifact_dir.into(),
            token_env: tracker.token_env.clone(),
            api_version: tracker.github_api_version.clone(),
            standard_label_policy: github.standard_label_policy.clone(),
            project_automation: github.project_automation.clone(),
            project_owner: github.project_owner.clone(),
            project_number: github.project_number,
            project_title: github.project_title.clone(),
            project_priority_field: github.project_priority_field.clone(),
            project_size_field: github.project_size_field.clone(),
        }
    }

    pub(crate) fn check_label_exists(&self, repo: &str, label: &str) -> Result<(), String> {
        validate_github_repo(repo)?;
        let token = github_token_from_env(&self.token_env)?;
        let url = format!(
            "https://api.github.com/repos/{repo}/labels/{}",
            github_path_segment_encode(label)
        );
        github_json_request(
            reqwest::Method::GET,
            &url,
            &token,
            &self.api_version,
            None,
            &self.artifact_dir,
            &format!("github_label_{}", safe_artifact_segment(label)),
        )
        .map(|_| ())
    }

    fn project_automation_enabled(&self) -> bool {
        self.project_automation == "ensure_on_issue_create"
    }

    fn capabilities_for_config(&self) -> AdapterCapabilities {
        let mut capabilities = Self::bootstrap_capabilities();
        if self.project_automation_enabled() {
            capabilities
                .supported_features
                .push("github_projects_v2".to_string());
            capabilities
                .degraded_features
                .retain(|feature| feature != "github_projects_v2");
        }
        capabilities.supported_features.sort();
        capabilities.supported_features.dedup();
        capabilities
    }

    fn ensure_standard_issue_labels(
        &self,
        repo: &str,
        labels: &[String],
        token: &str,
    ) -> Result<(), String> {
        if self.standard_label_policy == "disabled" {
            return Ok(());
        }
        if self.standard_label_policy != "ensure_on_issue_create" {
            return Err(format!(
                "unsupported github.standard_label_policy `{}`",
                self.standard_label_policy
            ));
        }
        for label in labels {
            if let Some(spec) = standard_github_label(label) {
                self.ensure_standard_label(repo, spec, token)?;
            }
        }
        Ok(())
    }

    fn ensure_standard_label(
        &self,
        repo: &str,
        spec: &GithubStandardLabel,
        token: &str,
    ) -> Result<(), String> {
        let label_path = github_path_segment_encode(spec.name);
        let artifact_segment = safe_artifact_segment(spec.name);
        let get_url = format!("https://api.github.com/repos/{repo}/labels/{label_path}");
        let (status, headers, body) = github_json_raw_request(
            reqwest::Method::GET,
            &get_url,
            token,
            &self.api_version,
            None,
            &self.artifact_dir,
            &format!("github_standard_label_{artifact_segment}_get"),
        )?;
        if status.is_success() {
            let artifact = self
                .artifact_dir
                .join(format!("github_standard_label_{artifact_segment}.json"));
            write_file(&artifact, &body)?;
            return Ok(());
        }
        if status.as_u16() != 404 {
            return Err(format!(
                "GitHub label preflight failed for `{}`: status={} {} body={}",
                spec.name,
                status.as_u16(),
                github_header_report(&headers),
                body
            ));
        }
        let create_url = format!("https://api.github.com/repos/{repo}/labels");
        let payload = serde_json::json!({
            "name": spec.name,
            "color": spec.color,
            "description": spec.description,
        });
        let created = github_json_request(
            reqwest::Method::POST,
            &create_url,
            token,
            &self.api_version,
            Some(payload),
            &self.artifact_dir,
            &format!("github_standard_label_{artifact_segment}_created"),
        )?;
        write_file(
            self.artifact_dir
                .join(format!("github_standard_label_{artifact_segment}.json")),
            &serde_json::to_string_pretty(&created)
                .map_err(|e| format!("render GitHub standard label artifact: {e}"))?,
        )
    }

    fn apply_project_fields_to_issue(
        &self,
        repo: &str,
        issue_node_id: &str,
        fields: &[IssueProjectFieldValue],
        token: &str,
    ) -> Result<GithubProjectAutomationResult, String> {
        if fields.is_empty() {
            return Ok(GithubProjectAutomationResult {
                applied_fields: Vec::new(),
                artifacts: Vec::new(),
            });
        }
        if !self.project_automation_enabled() {
            return Err(
                "proposal includes project_fields but github.project_automation is disabled"
                    .to_string(),
            );
        }
        let owner_login = github_project_owner_login(repo, &self.project_owner)?;
        let owner = self.fetch_project_owner(&owner_login, token)?;
        let mut project = self.resolve_or_create_project(&owner_login, &owner, token)?;
        let mut artifacts = vec![self.artifact_dir.join("github_project_resolve.json")];
        project = self.ensure_project_fields(project, fields, token)?;
        artifacts.push(self.artifact_dir.join("github_project_fields.json"));
        let item_id = self.add_issue_to_project(&project.id, issue_node_id, token)?;
        artifacts.push(self.artifact_dir.join("github_project_item.json"));
        let mut applied = Vec::new();
        for requested in fields {
            let field = project
                .fields
                .iter()
                .find(|field| field.name == requested.field_name)
                .ok_or_else(|| {
                    format!(
                        "GitHub project field `{}` was not available after ensure",
                        requested.field_name
                    )
                })?;
            let option = field
                .options
                .iter()
                .find(|option| option.name == requested.value)
                .ok_or_else(|| {
                    format!(
                        "GitHub project field `{}` has no option `{}`",
                        requested.field_name, requested.value
                    )
                })?;
            self.update_project_item_single_select(
                &project.id,
                &item_id,
                &field.id,
                &option.id,
                &format!(
                    "github_project_field_{}_{}",
                    safe_artifact_segment(&requested.field_name),
                    safe_artifact_segment(&requested.value)
                ),
                token,
            )?;
            artifacts.push(self.artifact_dir.join(format!(
                "github_project_field_{}_{}.json",
                safe_artifact_segment(&requested.field_name),
                safe_artifact_segment(&requested.value)
            )));
            applied.push(requested.clone());
        }
        Ok(GithubProjectAutomationResult {
            applied_fields: applied,
            artifacts,
        })
    }

    fn fetch_project_owner(
        &self,
        owner_login: &str,
        token: &str,
    ) -> Result<GithubProjectOwner, String> {
        let response = github_graphql_request(
            r#"
query($login: String!) {
  user(login: $login) { id }
  organization(login: $login) { id }
}
"#,
            serde_json::json!({ "login": owner_login }),
            token,
            &self.api_version,
            &self.artifact_dir,
            "github_project_owner",
        )?;
        let owner = response
            .pointer("/data/user")
            .or_else(|| response.pointer("/data/organization"))
            .ok_or_else(|| format!("GitHub Projects owner `{owner_login}` was not found"))?;
        Ok(GithubProjectOwner {
            id: json_str(owner, "/id")
                .ok_or_else(|| format!("GitHub Projects owner `{owner_login}` has no node id"))?,
        })
    }

    fn resolve_or_create_project(
        &self,
        owner_login: &str,
        owner: &GithubProjectOwner,
        token: &str,
    ) -> Result<GithubProject, String> {
        let existing = if self.project_number > 0 {
            self.fetch_project_by_number(owner_login, self.project_number, token)?
        } else {
            self.find_project_by_title(owner_login, &self.project_title, token)?
        };
        if let Some(project) = existing {
            write_file(
                self.artifact_dir.join("github_project_resolve.json"),
                &serde_json::to_string_pretty(&serde_json::json!({
                    "action": "found",
                    "title": project.title,
                    "number": project.number,
                    "id": project.id,
                }))
                .map_err(|e| format!("render GitHub project resolve artifact: {e}"))?,
            )?;
            return Ok(project);
        }
        if self.project_number > 0 {
            return Err(format!(
                "GitHub ProjectV2 number {} was not found for `{owner_login}`",
                self.project_number
            ));
        }
        let response = github_graphql_request(
            r#"
mutation($ownerId: ID!, $title: String!) {
  createProjectV2(input: { ownerId: $ownerId, title: $title }) {
    projectV2 { id title number }
  }
}
"#,
            serde_json::json!({
                "ownerId": owner.id,
                "title": self.project_title,
            }),
            token,
            &self.api_version,
            &self.artifact_dir,
            "github_project_create",
        )?;
        let project = response
            .pointer("/data/createProjectV2/projectV2")
            .ok_or("GitHub createProjectV2 response missing projectV2")?;
        let created = GithubProject {
            id: json_str(project, "/id")
                .ok_or("GitHub createProjectV2 response missing project id")?,
            title: json_str(project, "/title").unwrap_or_else(|| self.project_title.clone()),
            number: project
                .pointer("/number")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default() as u32,
            fields: Vec::new(),
        };
        write_file(
            self.artifact_dir.join("github_project_resolve.json"),
            &serde_json::to_string_pretty(&serde_json::json!({
                "action": "created",
                "title": created.title,
                "number": created.number,
                "id": created.id,
            }))
            .map_err(|e| format!("render GitHub project resolve artifact: {e}"))?,
        )?;
        Ok(created)
    }

    fn fetch_project_by_number(
        &self,
        owner_login: &str,
        number: u32,
        token: &str,
    ) -> Result<Option<GithubProject>, String> {
        let response = github_graphql_request(
            github_project_by_number_query(),
            serde_json::json!({ "login": owner_login, "number": number }),
            token,
            &self.api_version,
            &self.artifact_dir,
            "github_project_fetch",
        )?;
        Ok(response
            .pointer("/data/user/projectV2")
            .or_else(|| response.pointer("/data/organization/projectV2"))
            .and_then(github_project_from_json))
    }

    fn find_project_by_title(
        &self,
        owner_login: &str,
        title: &str,
        token: &str,
    ) -> Result<Option<GithubProject>, String> {
        let response = github_graphql_request(
            github_project_list_query(),
            serde_json::json!({ "login": owner_login, "query": title }),
            token,
            &self.api_version,
            &self.artifact_dir,
            "github_project_fetch",
        )?;
        let nodes = response
            .pointer("/data/user/projectsV2/nodes")
            .or_else(|| response.pointer("/data/organization/projectsV2/nodes"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(nodes
            .iter()
            .filter_map(github_project_from_json)
            .find(|project| project.title == title))
    }

    fn ensure_project_fields(
        &self,
        mut project: GithubProject,
        requested: &[IssueProjectFieldValue],
        token: &str,
    ) -> Result<GithubProject, String> {
        for field in requested {
            if project
                .fields
                .iter()
                .any(|existing| existing.name == field.field_name)
            {
                continue;
            }
            let options = self.project_field_options(&field.field_name)?;
            let response = github_graphql_request(
                r#"
mutation($projectId: ID!, $name: String!, $options: [ProjectV2SingleSelectFieldOptionInput!]!) {
  createProjectV2Field(input: { projectId: $projectId, dataType: SINGLE_SELECT, name: $name, singleSelectOptions: $options }) {
    projectV2Field {
      ... on ProjectV2SingleSelectField {
        id
        name
        options { id name }
      }
    }
  }
}
"#,
                serde_json::json!({
                    "projectId": project.id,
                    "name": field.field_name,
                    "options": options,
                }),
                token,
                &self.api_version,
                &self.artifact_dir,
                &format!(
                    "github_project_create_field_{}",
                    safe_artifact_segment(&field.field_name)
                ),
            )?;
            let created = response
                .pointer("/data/createProjectV2Field/projectV2Field")
                .and_then(github_project_field_from_json)
                .ok_or_else(|| {
                    format!(
                        "GitHub createProjectV2Field did not return `{}`",
                        field.field_name
                    )
                })?;
            project.fields.push(created);
        }
        write_file(
            self.artifact_dir.join("github_project_fields.json"),
            &serde_json::to_string_pretty(&serde_json::json!({
                "project_id": project.id,
                "fields": project.fields.iter().map(|field| serde_json::json!({
                    "id": field.id,
                    "name": field.name,
                    "options": field.options.iter().map(|option| serde_json::json!({
                        "id": option.id,
                        "name": option.name,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }))
            .map_err(|e| format!("render GitHub project fields artifact: {e}"))?,
        )?;
        Ok(project)
    }

    fn add_issue_to_project(
        &self,
        project_id: &str,
        issue_node_id: &str,
        token: &str,
    ) -> Result<String, String> {
        let response = github_graphql_request(
            r#"
mutation($projectId: ID!, $contentId: ID!) {
  addProjectV2ItemById(input: { projectId: $projectId, contentId: $contentId }) {
    item { id }
  }
}
"#,
            serde_json::json!({
                "projectId": project_id,
                "contentId": issue_node_id,
            }),
            token,
            &self.api_version,
            &self.artifact_dir,
            "github_project_item",
        )?;
        response
            .pointer("/data/addProjectV2ItemById/item/id")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .ok_or("GitHub addProjectV2ItemById response missing item id".to_string())
    }

    fn update_project_item_single_select(
        &self,
        project_id: &str,
        item_id: &str,
        field_id: &str,
        option_id: &str,
        artifact_label: &str,
        token: &str,
    ) -> Result<(), String> {
        github_graphql_request(
            r#"
mutation($projectId: ID!, $itemId: ID!, $fieldId: ID!, $optionId: String!) {
  updateProjectV2ItemFieldValue(input: {
    projectId: $projectId,
    itemId: $itemId,
    fieldId: $fieldId,
    value: { singleSelectOptionId: $optionId }
  }) {
    projectV2Item { id }
  }
}
"#,
            serde_json::json!({
                "projectId": project_id,
                "itemId": item_id,
                "fieldId": field_id,
                "optionId": option_id,
            }),
            token,
            &self.api_version,
            &self.artifact_dir,
            artifact_label,
        )
        .map(|_| ())
    }

    fn project_field_options(&self, field_name: &str) -> Result<Vec<serde_json::Value>, String> {
        if field_name == self.project_priority_field {
            return project_field_options("Priority");
        }
        if field_name == self.project_size_field {
            return project_field_options("Size");
        }
        project_field_options(field_name)
    }
}

impl GithubRestAdapter {
    pub(crate) fn fetch_issue_json(
        &self,
        repo: &str,
        issue: &str,
        artifact_dir: &Path,
    ) -> Result<String, String> {
        validate_github_repo(repo)?;
        validate_issue_number(issue)?;
        let token = github_token_from_env(&self.token_env)?;
        let url = format!("https://api.github.com/repos/{repo}/issues/{issue}");
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("agentactr-cli/0.1")
            .build()
            .map_err(|e| format!("build GitHub HTTP client: {e}"))?;
        let mut rate_events = Vec::new();
        let mut rate_trace_events = Vec::new();
        let max_attempts = 5;
        let mut response = None;
        for attempt in 0..max_attempts {
            let attempt_response = client
                .get(&url)
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
                .header("X-GitHub-Api-Version", &self.api_version)
                .send()
                .map_err(|e| format!("GitHub issue request failed: {e}"))?;
            let status = attempt_response.status();
            let headers = attempt_response.headers().clone();
            let body = attempt_response
                .text()
                .map_err(|e| format!("read GitHub issue response: {e}"))?;
            if let Some(decision) = github_retry_decision(status, attempt, &headers, &body) {
                let event = format!(
                    "attempt={} status={} wait_seconds={} reason={}",
                    attempt + 1,
                    status.as_u16(),
                    decision.wait.as_secs(),
                    decision.reason
                );
                warn!("{event}");
                rate_events.push(event);
                rate_trace_events.push(github_rate_limit_trace_payload(
                    attempt + 1,
                    status.as_u16(),
                    &decision,
                    &headers,
                ));
                if attempt + 1 >= max_attempts {
                    response = Some((status, headers, body));
                    break;
                }
                if decision.wait > Duration::from_secs(300) {
                    write_file(
                        artifact_dir.join("github_issue.headers"),
                        &github_header_report(&headers),
                    )?;
                    write_github_rate_limit_artifacts(
                        artifact_dir,
                        &rate_events,
                        &rate_trace_events,
                    )?;
                    return Err(format!(
                        "GitHub rate limited; retry after {} seconds ({})",
                        decision.wait.as_secs(),
                        decision.reason
                    ));
                }
                thread::sleep(decision.wait);
                continue;
            }
            response = Some((status, headers, body));
            break;
        }
        let (status, headers, body) =
            response.ok_or("GitHub issue request produced no response")?;
        let header_report = github_header_report(&headers);
        write_file(artifact_dir.join("github_issue.headers"), &header_report)?;
        write_github_rate_limit_artifacts(artifact_dir, &rate_events, &rate_trace_events)?;
        if !status.is_success() {
            return Err(format!(
                "GitHub issue fetch failed: status={} {}",
                status.as_u16(),
                header_report
            ));
        }
        write_file(artifact_dir.join("github_issue.json"), &body)?;
        Ok(body)
    }

    fn fetch_issue(&self, repo: &str, issue: &str) -> Result<Issue, String> {
        let raw_json = self.fetch_issue_json(repo, issue, &self.artifact_dir)?;
        let issue_artifact = self.artifact_dir.join("github_issue.json");
        let parsed = serde_json::from_str::<serde_json::Value>(&raw_json)
            .map_err(|e| format!("parse GitHub issue JSON: {e}"))?;
        Ok(Issue {
            id: format!("{repo}#{issue}"),
            repo: repo.to_string(),
            number: issue.parse::<u64>().unwrap_or_default(),
            title: json_str(&parsed, "/title").unwrap_or_default(),
            body: json_str(&parsed, "/body").unwrap_or_default(),
            state: json_str(&parsed, "/state").unwrap_or_default(),
            author: json_str(&parsed, "/user/login").unwrap_or_default(),
            labels: github_label_names(&parsed),
            created_at: json_str(&parsed, "/created_at"),
            updated_at: json_str(&parsed, "/updated_at"),
            is_pull_request: parsed.get("pull_request").is_some(),
            html_url: json_str(&parsed, "/html_url"),
            source_artifact: Some(issue_artifact),
        })
    }

    fn apply_issue_labels(
        &self,
        repo: &str,
        issue_number: u64,
        add_labels: &[String],
        remove_labels: &[String],
        close_state_reason: Option<&str>,
        artifact_label: &str,
    ) -> Result<GithubIssueMutationResult, String> {
        validate_github_repo(repo)?;
        let token = github_token_from_env(&self.token_env)?;
        let add_labels = normalized_label_set(add_labels);
        let remove_labels = normalized_label_set(remove_labels);
        if !add_labels.is_empty() {
            let url = format!("https://api.github.com/repos/{repo}/issues/{issue_number}/labels");
            github_json_request(
                reqwest::Method::POST,
                &url,
                &token,
                &self.api_version,
                Some(serde_json::json!({ "labels": add_labels })),
                &self.artifact_dir,
                &format!("{artifact_label}_add_labels"),
            )?;
        }
        for label in &remove_labels {
            let url = format!(
                "https://api.github.com/repos/{repo}/issues/{issue_number}/labels/{}",
                github_path_segment_encode(label)
            );
            if let Err(err) = github_json_request(
                reqwest::Method::DELETE,
                &url,
                &token,
                &self.api_version,
                None,
                &self.artifact_dir,
                &format!(
                    "{artifact_label}_remove_label_{}",
                    safe_artifact_segment(label)
                ),
            ) {
                let final_issue = self.fetch_issue(repo, &issue_number.to_string())?;
                if final_issue.labels.contains(label) || !err.contains("status=404") {
                    return Err(err);
                }
            }
        }
        let response = if let Some(reason) = close_state_reason {
            let url = format!("https://api.github.com/repos/{repo}/issues/{issue_number}");
            github_json_request(
                reqwest::Method::PATCH,
                &url,
                &token,
                &self.api_version,
                Some(serde_json::json!({
                    "state": "closed",
                    "state_reason": reason,
                })),
                &self.artifact_dir,
                artifact_label,
            )?
        } else {
            let final_issue = self.fetch_issue(repo, &issue_number.to_string())?;
            serde_json::json!({
                "labels": final_issue
                    .labels
                    .iter()
                    .map(|label| serde_json::json!({ "name": label }))
                    .collect::<Vec<_>>(),
                "state": final_issue.state,
                "state_reason": serde_json::Value::Null,
            })
        };
        let response_path = self.artifact_dir.join(format!("{artifact_label}.json"));
        write_file(
            &response_path,
            &serde_json::to_string_pretty(&response)
                .map_err(|e| format!("render GitHub issue lifecycle artifact: {e}"))?,
        )?;
        Ok(GithubIssueMutationResult {
            labels: github_label_names(&response),
            state: json_str(&response, "/state").unwrap_or_default(),
            state_reason: json_str(&response, "/state_reason"),
            artifact: response_path,
        })
    }

    fn upsert_lifecycle_comment(
        &self,
        repo: &str,
        issue_number: u64,
        _kind: IssueCommentKind,
        marker_identity: &str,
        body: &str,
        update_existing: bool,
    ) -> Result<CommentRef, String> {
        validate_github_repo(repo)?;
        let token = github_token_from_env(&self.token_env)?;
        let comments = self.list_issue_comments(repo, issue_number)?;
        if let Some(existing) = comments.iter().find(|comment| {
            json_str(comment, "/body").is_some_and(|text| text.contains(marker_identity))
        }) {
            let provider_id = existing
                .pointer("/id")
                .and_then(serde_json::Value::as_u64)
                .ok_or("GitHub comment marker matched comment without numeric id")?;
            if !update_existing {
                return Ok(comment_ref_from_json(
                    existing,
                    self.artifact_dir.join("github_lifecycle_comment.json"),
                    "existing",
                ));
            }
            let url = format!("https://api.github.com/repos/{repo}/issues/comments/{provider_id}");
            let response = github_json_request(
                reqwest::Method::PATCH,
                &url,
                &token,
                &self.api_version,
                Some(serde_json::json!({ "body": body })),
                &self.artifact_dir,
                "github_lifecycle_comment",
            )?;
            write_file(
                self.artifact_dir.join("github_lifecycle_comment.json"),
                &serde_json::to_string_pretty(&response)
                    .map_err(|e| format!("render GitHub lifecycle comment artifact: {e}"))?,
            )?;
            return Ok(comment_ref_from_json(
                &response,
                self.artifact_dir.join("github_lifecycle_comment.json"),
                "updated",
            ));
        }
        let url = format!("https://api.github.com/repos/{repo}/issues/{issue_number}/comments");
        let response = github_json_request(
            reqwest::Method::POST,
            &url,
            &token,
            &self.api_version,
            Some(serde_json::json!({ "body": body })),
            &self.artifact_dir,
            "github_lifecycle_comment",
        )?;
        write_file(
            self.artifact_dir.join("github_lifecycle_comment.json"),
            &serde_json::to_string_pretty(&response)
                .map_err(|e| format!("render GitHub lifecycle comment artifact: {e}"))?,
        )?;
        Ok(comment_ref_from_json(
            &response,
            self.artifact_dir.join("github_lifecycle_comment.json"),
            "created",
        ))
    }

    fn list_issue_comments(
        &self,
        repo: &str,
        issue_number: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let token = github_token_from_env(&self.token_env)?;
        let mut comments = Vec::new();
        let mut page = 1_u32;
        loop {
            let url = format!(
                "https://api.github.com/repos/{repo}/issues/{issue_number}/comments?per_page=100&page={page}"
            );
            let label = format!("github_lifecycle_comments_page_{page}");
            let response = github_json_request(
                reqwest::Method::GET,
                &url,
                &token,
                &self.api_version,
                None,
                &self.artifact_dir,
                &label,
            )?;
            let page_items = response
                .as_array()
                .cloned()
                .ok_or("GitHub comments response was not an array".to_string())?;
            let page_len = page_items.len();
            comments.extend(page_items);
            if page_len < 100 {
                break;
            }
            page = page.saturating_add(1);
        }
        write_file(
            self.artifact_dir.join("github_lifecycle_comments.json"),
            &serde_json::to_string_pretty(&comments)
                .map_err(|e| format!("render GitHub lifecycle comments artifact: {e}"))?,
        )?;
        Ok(comments)
    }
}

impl IssueTracker for GithubRestAdapter {
    fn version_report(&self) -> AdapterVersionReport {
        let mut degraded_features = vec!["issue_field_values".to_string()];
        if !self.project_automation_enabled() {
            degraded_features.push("github_projects_v2".to_string());
        }
        AdapterVersionReport {
            adapter_kind: "issue_tracker".to_string(),
            adapter_name: "agentactr-cli-github-reqwest".to_string(),
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            product_name: "github-rest".to_string(),
            product_version: "api.github.com".to_string(),
            api_version: self.api_version.clone(),
            capability_digest: "issue-read-create-link-lifecycle".to_string(),
            degraded_features,
            required_actions: vec![
                "keep issue field values degraded until proposal parser and response verifier round-trip them".to_string(),
            ],
            warnings: vec![
                "GitHub lifecycle mutation is SDK-gated and honors github.finalization policy".to_string(),
            ],
        }
    }

    fn capabilities(&self) -> AdapterCapabilities {
        self.capabilities_for_config()
    }

    fn fetch_candidates(&self, _q: CandidateQuery) -> Result<Vec<Issue>, String> {
        self.fetch_issue_candidates(_q)
    }

    fn fetch_by_ids(&self, ids: &[IssueId]) -> Result<Vec<Issue>, String> {
        let mut issues = Vec::new();
        for id in ids {
            let Some((repo, issue)) = id.0.rsplit_once('#') else {
                return Err(format!("issue id must use OWNER/REPO#NUMBER: {}", id.0));
            };
            issues.push(self.fetch_issue(repo, issue)?);
        }
        Ok(issues)
    }

    fn claim(&self, req: ClaimRequest) -> Result<ClaimResult, String> {
        let issue = self.fetch_issue(&req.repo, &req.issue_number.to_string())?;
        if issue.is_pull_request && !req.allow_pull_request {
            return Err(
                "GitHub lifecycle mutation rejects pull-request shaped issue records by default"
                    .to_string(),
            );
        }
        if issue.state != "open" {
            return Err(format!(
                "GitHub lifecycle claim requires open issue; found state={}",
                issue.state
            ));
        }
        if issue
            .labels
            .iter()
            .any(|label| req.ignore_labels.iter().any(|ignore| ignore == label))
        {
            return Err("GitHub lifecycle claim rejected ignored issue".to_string());
        }
        let claim_comments = self.list_issue_comments(&req.repo, req.issue_number)?;
        let mut previous_lease = None;
        let now = iso_timestamp_from_epoch_millis(current_epoch_millis());
        for marker in github_claim_markers_from_comments(&claim_comments) {
            if marker.run_id == req.run_id {
                previous_lease = Some(format!(
                    "same_run owner_id={} token={} expires_at={}",
                    marker.owner_id, marker.fencing_token, marker.expires_at
                ));
                continue;
            }
            if marker.expires_at > now {
                return Ok(ClaimResult {
                    accepted: false,
                    fencing_token: req.fencing_token,
                    previous_lease: Some(format!(
                        "foreign_active run_id={} token={} expires_at={}",
                        marker.run_id, marker.fencing_token, marker.expires_at
                    )),
                    applied_labels: issue.labels,
                    marker_comment: None,
                    source_artifacts: vec![
                        self.artifact_dir.join("github_issue.json"),
                        self.artifact_dir.join("github_lifecycle_comments.json"),
                    ],
                    verification_status: "blocked_by_foreign_lease".to_string(),
                    detail: "non-expired foreign GitHub lifecycle lease exists".to_string(),
                });
            }
            previous_lease = Some(format!(
                "foreign_expired run_id={} owner_id={} token={} expires_at={}",
                marker.run_id, marker.owner_id, marker.fencing_token, marker.expires_at
            ));
        }
        let body = github_claim_marker_body(&req);
        let marker = github_claim_marker(&req.run_id);
        let comment = self.upsert_lifecycle_comment(
            &req.repo,
            req.issue_number,
            IssueCommentKind::Claim,
            &marker,
            &body,
            true,
        )?;
        let label_result = self.apply_issue_labels(
            &req.repo,
            req.issue_number,
            &[req.claim_label.clone(), req.running_label.clone()],
            &[],
            None,
            "github_claim_issue",
        )?;
        let verified_issue = self.fetch_issue(&req.repo, &req.issue_number.to_string())?;
        let verified_comments = self.list_issue_comments(&req.repo, req.issue_number)?;
        let verified_marker = github_claim_markers_from_comments(&verified_comments)
            .into_iter()
            .any(|claim| {
                claim.schema_version == "1"
                    && claim.run_id == req.run_id
                    && claim.owner_id == req.owner_id
                    && claim.fencing_token == req.fencing_token
                    && claim.expires_at == req.lease_expires_at
            });
        let accepted = verified_issue.labels.contains(&req.claim_label)
            && verified_issue.labels.contains(&req.running_label)
            && verified_marker
            && comment.provider_id != "missing";
        Ok(ClaimResult {
            accepted,
            fencing_token: req.fencing_token,
            previous_lease,
            applied_labels: verified_issue.labels,
            marker_comment: Some(comment),
            source_artifacts: vec![
                self.artifact_dir.join("github_issue.json"),
                label_result.artifact,
                self.artifact_dir.join("github_lifecycle_comment.json"),
                self.artifact_dir.join("github_lifecycle_comments.json"),
            ],
            verification_status: if accepted { "verified" } else { "mismatch" }.to_string(),
            detail: if accepted {
                "claim marker and claim/running labels verified".to_string()
            } else {
                "claim verification failed".to_string()
            },
        })
    }

    fn release(&self, req: ReleaseRequest) -> Result<ReleaseResult, String> {
        let mut comment_refs = Vec::new();
        if let Some(comment) = req.final_comment.as_ref() {
            comment_refs.push(self.comment(comment.clone())?);
        }
        let state_reason = req.close_state_reason.clone();
        let mutation = self.apply_issue_labels(
            &req.repo,
            req.issue_number,
            &req.add_labels,
            &req.remove_labels,
            state_reason.as_deref(),
            "github_release_issue",
        )?;
        let final_issue = self.fetch_issue(&req.repo, &req.issue_number.to_string())?;
        let mut mismatch_details = Vec::new();
        for label in &req.add_labels {
            if !mutation.labels.contains(label) {
                mismatch_details.push(format!("expected added label `{label}` is absent"));
            }
        }
        for label in &req.remove_labels {
            if mutation.labels.contains(label) {
                mismatch_details.push(format!("expected removed label `{label}` is still present"));
            }
        }
        if req.close_state_reason.is_some() && final_issue.state != "closed" {
            mismatch_details.push(format!(
                "expected closed issue state, found `{}`",
                final_issue.state
            ));
        }
        if req.close_state_reason.is_some() && mutation.state != "closed" {
            mismatch_details.push(format!(
                "expected closed issue response state, found `{}`",
                mutation.state
            ));
        }
        if req.close_state_reason.is_some() && mutation.state_reason != req.close_state_reason {
            mismatch_details.push(format!(
                "expected state_reason `{:?}`, found `{:?}`",
                req.close_state_reason, mutation.state_reason
            ));
        }
        let verification_status = if mismatch_details.is_empty() {
            "verified"
        } else {
            "mismatch"
        }
        .to_string();
        if verification_status != "verified" {
            return Err(format!(
                "GitHub release verification failed: {}",
                mismatch_details.join("; ")
            ));
        }
        Ok(ReleaseResult {
            applied_labels: mutation.labels,
            removed_labels: req.remove_labels,
            final_issue_state: final_issue.state,
            state_reason: mutation.state_reason,
            comment_refs,
            source_artifacts: vec![
                mutation.artifact,
                self.artifact_dir.join("github_issue.json"),
            ],
            verification_status,
            mismatch_details,
        })
    }

    fn comment(&self, req: CommentRequest) -> Result<CommentRef, String> {
        let marker = github_lifecycle_comment_marker(&req);
        let marker_identity = github_lifecycle_comment_marker_identity(&req);
        let body = format!("{}\n\n{}", req.body.trim_end(), marker);
        self.upsert_lifecycle_comment(
            &req.repo,
            req.issue_number,
            req.kind,
            &marker_identity,
            &body,
            req.update_existing,
        )
    }

    fn create_issue(&self, req: IssueCreateRequest) -> Result<IssueCreateResult, String> {
        validate_github_repo(&req.proposal.repo)?;
        if !req.proposal.issue_field_values.is_empty() {
            return Err(
                "GitHub issue_field_values are not supported by the REST create issue endpoint; use project_fields or remove unsupported issue field values before submission"
                    .to_string(),
            );
        }
        let token = github_token_from_env(&self.token_env)?;
        self.ensure_standard_issue_labels(&req.proposal.repo, &req.proposal.labels, &token)?;
        let url = format!("https://api.github.com/repos/{}/issues", req.proposal.repo);
        let body = format!("{}\n\n{}", req.proposal.body.trim_end(), req.body_marker);
        let milestone = github_issue_create_milestone(req.proposal.milestone.as_deref())?;
        let payload = github_create_issue_payload(
            &req.proposal.title,
            &body,
            &req.proposal.labels,
            &req.proposal.assignees,
            milestone,
            req.proposal.issue_type.as_deref(),
        );
        let response = github_json_request(
            reqwest::Method::POST,
            &url,
            &token,
            &self.api_version,
            Some(payload),
            &self.artifact_dir,
            "github_created_issue",
        )?;
        let issue_artifact = self.artifact_dir.join("github_created_issue.json");
        write_file(
            &issue_artifact,
            &serde_json::to_string_pretty(&response)
                .map_err(|e| format!("render created issue artifact: {e}"))?,
        )?;
        let project_result = if req.proposal.project_fields.is_empty() {
            GithubProjectAutomationResult {
                applied_fields: Vec::new(),
                artifacts: Vec::new(),
            }
        } else {
            let issue_node_id = json_str(&response, "/node_id")
                .ok_or("GitHub issue creation response did not include node_id for ProjectV2")?;
            self.apply_project_fields_to_issue(
                &req.proposal.repo,
                &issue_node_id,
                &req.proposal.project_fields,
                &token,
            )?
        };
        if !project_result.artifacts.is_empty() {
            write_file(
                self.artifact_dir.join("github_project_automation.json"),
                &serde_json::to_string_pretty(&serde_json::json!({
                    "applied_fields": project_result.applied_fields.iter().map(|field| serde_json::json!({
                        "field_name": field.field_name,
                        "value": field.value,
                    })).collect::<Vec<_>>(),
                    "artifacts": project_result.artifacts.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
                }))
                .map_err(|e| format!("render GitHub project automation artifact: {e}"))?,
            )?;
        }
        Ok(issue_create_result_from_response(
            req,
            &response,
            issue_artifact,
            project_result.applied_fields,
        ))
    }

    fn link_issue(&self, req: IssueLinkRequest) -> Result<IssueLinkResult, String> {
        let token = github_token_from_env(&self.token_env)?;
        validate_github_repo(&req.repo)?;
        let url = format!(
            "https://api.github.com/repos/{}/issues/{}/sub_issues",
            req.repo, req.parent_issue
        );
        let payload = serde_json::json!({ "sub_issue_id": req.child_issue_id });
        let response = github_json_request(
            reqwest::Method::POST,
            &url,
            &token,
            &self.api_version,
            Some(payload),
            &self.artifact_dir,
            "github_linked_sub_issue",
        )?;
        write_file(
            self.artifact_dir.join("github_linked_sub_issue.json"),
            &serde_json::to_string_pretty(&response)
                .map_err(|e| format!("render linked sub-issue artifact: {e}"))?,
        )?;
        Ok(IssueLinkResult {
            parent_issue: req.parent_issue,
            child_issue_number: req.child_issue_number,
            linked: true,
            detail: format!(
                "linked issue #{} as sub-issue of #{}",
                req.child_issue_number, req.parent_issue
            ),
        })
    }
}

impl GithubRestAdapter {
    pub(crate) fn bootstrap_capabilities() -> AdapterCapabilities {
        AdapterCapabilities {
            adapter_kind: "issue_tracker".to_string(),
            supported_features: vec![
                "issue_read".to_string(),
                "candidate_polling".to_string(),
                "rate_limit_artifacts".to_string(),
                "issue_create".to_string(),
                "issue_link".to_string(),
                "issue_labels".to_string(),
                "issue_assignees".to_string(),
                "issue_milestone".to_string(),
                "issue_type".to_string(),
                "standard_label_ensure".to_string(),
                "claim_mutation".to_string(),
                "comment_create".to_string(),
                "comment_update".to_string(),
                "label_set".to_string(),
                "issue_close".to_string(),
                "state_reason".to_string(),
            ],
            degraded_features: vec![
                "issue_field_values".to_string(),
                "github_projects_v2".to_string(),
            ],
            required_actions: Vec::new(),
        }
    }

    fn fetch_issue_candidates(&self, query: CandidateQuery) -> Result<Vec<Issue>, String> {
        let token = github_token_from_env(&self.token_env)?;
        validate_github_repo(&query.repo)?;
        create_dir(&self.artifact_dir)?;
        let mut issues = Vec::new();
        let mut page = query.page.unwrap_or(1).max(1);
        let per_page = query.per_page.clamp(1, 100);
        let limit = query.limit.max(1);
        let search_endpoint = query.text_query.is_some();
        let mut search_total_count = None;
        let mut search_incomplete_results = None;
        loop {
            let page_s = page.to_string();
            let per_page_s = per_page.to_string();
            let url = if let Some(search) = query.text_query.as_deref() {
                let terms = github_issue_search_terms(&query, search);
                reqwest::Url::parse_with_params(
                    "https://api.github.com/search/issues",
                    &[
                        ("q", terms.as_str()),
                        ("sort", query.sort.as_github_value()),
                        ("order", query.direction.as_github_value()),
                        ("page", page_s.as_str()),
                        ("per_page", per_page_s.as_str()),
                    ],
                )
                .map_err(|e| format!("build GitHub issue search URL: {e}"))?
            } else {
                github_issue_list_url(&query, page_s.as_str(), per_page_s.as_str())?
            };
            let label = format!("github_issue_candidates_page_{page}");
            let response = github_json_request(
                reqwest::Method::GET,
                url.as_str(),
                &token,
                &self.api_version,
                None,
                &self.artifact_dir,
                &label,
            )?;
            if search_endpoint {
                capture_github_issue_search_metadata(
                    &response,
                    &mut search_total_count,
                    &mut search_incomplete_results,
                );
            }
            let page_items = if search_endpoint {
                response
                    .pointer("/items")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            } else {
                response.as_array().cloned().unwrap_or_default()
            };
            if page_items.is_empty() {
                break;
            }
            for item in &page_items {
                if item.get("pull_request").is_some() && !query.include_pull_requests {
                    continue;
                }
                issues.push(github_issue_from_value(&query.repo, item, None));
                if issues.len() >= limit as usize {
                    break;
                }
            }
            if issues.len() >= limit as usize || page_items.len() < per_page as usize {
                break;
            }
            page = page.saturating_add(1);
        }
        let summary = github_issue_candidates_summary(
            &query,
            issues.len(),
            limit,
            per_page,
            search_total_count,
            search_incomplete_results,
        );
        write_file(
            self.artifact_dir
                .join("github_issue_candidates_summary.json"),
            &serde_json::to_string_pretty(&summary)
                .map_err(|e| format!("render issue candidate summary: {e}"))?,
        )?;
        Ok(issues)
    }

    pub(crate) fn recover_created_issue_by_marker(
        &self,
        req: &IssueCreateRequest,
    ) -> Result<Option<IssueCreateResult>, String> {
        let token = github_token_from_env(&self.token_env)?;
        validate_github_repo(&req.proposal.repo)?;
        let query = format!(
            "repo:{} type:issue in:body \"{}\"",
            req.proposal.repo, req.body_marker
        );
        let url = reqwest::Url::parse_with_params(
            "https://api.github.com/search/issues",
            &[("q", query.as_str()), ("per_page", "1")],
        )
        .map_err(|e| format!("build GitHub issue marker search URL: {e}"))?;
        let response = github_json_request(
            reqwest::Method::GET,
            url.as_str(),
            &token,
            &self.api_version,
            None,
            &self.artifact_dir,
            "github_issue_marker_search",
        )?;
        write_file(
            self.artifact_dir.join("github_issue_marker_search.json"),
            &serde_json::to_string_pretty(&response)
                .map_err(|e| format!("render issue marker search artifact: {e}"))?,
        )?;
        if response
            .pointer("/total_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            > 1
        {
            return Err(
                "issue marker recovery found multiple candidate issues; inspect GitHub before resuming"
                    .to_string(),
            );
        }
        let Some(issue) = response
            .pointer("/items")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.first())
        else {
            return Ok(None);
        };
        if let Some(body) = json_str(issue, "/body") {
            if !body.contains(&req.body_marker) {
                return Err(
                    "issue marker recovery returned a result whose body does not contain the exact marker"
                        .to_string(),
                );
            }
        }
        let artifact = self.artifact_dir.join("github_recovered_issue.json");
        write_file(
            &artifact,
            &serde_json::to_string_pretty(issue)
                .map_err(|e| format!("render recovered issue artifact: {e}"))?,
        )?;
        Ok(Some(issue_create_result_from_response(
            req.clone(),
            issue,
            artifact,
            Vec::new(),
        )))
    }
}

fn github_issue_search_terms(query: &CandidateQuery, search: &str) -> String {
    let mut terms = format!("repo:{}", query.repo);
    if !query.include_pull_requests {
        terms.push_str(" is:issue");
    }
    match query.state {
        agentactr_sdk::CandidateState::Open => terms.push_str(" state:open"),
        agentactr_sdk::CandidateState::Closed => terms.push_str(" state:closed"),
        agentactr_sdk::CandidateState::All => {}
    }
    for label in &query.labels {
        terms.push_str(&format!(" label:\"{}\"", label.replace('"', "")));
    }
    if let Some(assignee) = query.assignee.as_deref() {
        terms.push_str(&format!(" assignee:{}", assignee.replace('"', "")));
    }
    if let Some(author) = query.author.as_deref() {
        terms.push_str(&format!(" author:{}", author.replace('"', "")));
    }
    if let Some(since) = query.since.as_deref() {
        terms.push_str(&format!(" updated:>={}", since.replace('"', "")));
    }
    if !search.trim().is_empty() {
        terms.push(' ');
        terms.push_str(search);
    }
    terms
}

fn github_issue_list_url(
    query: &CandidateQuery,
    page: &str,
    per_page: &str,
) -> Result<reqwest::Url, String> {
    let labels = query
        .labels
        .iter()
        .map(|label| label.trim())
        .filter(|label| !label.is_empty())
        .collect::<Vec<_>>()
        .join(",");
    let mut params = vec![
        ("state", query.state.as_github_value().to_string()),
        ("sort", query.sort.as_github_value().to_string()),
        ("direction", query.direction.as_github_value().to_string()),
        ("page", page.to_string()),
        ("per_page", per_page.to_string()),
    ];
    if !labels.is_empty() {
        params.push(("labels", labels));
    }
    if let Some(assignee) = query
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        params.push(("assignee", assignee.to_string()));
    }
    if let Some(author) = query
        .author
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        params.push(("creator", author.to_string()));
    }
    if let Some(since) = query
        .since
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        params.push(("since", since.to_string()));
    }
    reqwest::Url::parse_with_params(
        &format!("https://api.github.com/repos/{}/issues", query.repo),
        &params,
    )
    .map_err(|e| format!("build GitHub issue list URL: {e}"))
}

fn capture_github_issue_search_metadata(
    response: &serde_json::Value,
    total_count: &mut Option<u64>,
    incomplete_results: &mut Option<bool>,
) {
    if total_count.is_none() {
        *total_count = response
            .pointer("/total_count")
            .and_then(serde_json::Value::as_u64);
    }
    let page_incomplete = response
        .pointer("/incomplete_results")
        .and_then(serde_json::Value::as_bool);
    match (*incomplete_results, page_incomplete) {
        (_, Some(true)) => *incomplete_results = Some(true),
        (None, Some(false)) => *incomplete_results = Some(false),
        _ => {}
    }
}

fn github_issue_candidates_summary(
    query: &CandidateQuery,
    count: usize,
    limit: u32,
    per_page: u32,
    search_total_count: Option<u64>,
    search_incomplete_results: Option<bool>,
) -> serde_json::Value {
    let search_endpoint = query.text_query.is_some();
    serde_json::json!({
        "repo": &query.repo,
        "count": count,
        "limit": limit,
        "endpoint": if search_endpoint { "search/issues" } else { "repos/issues" },
        "search_endpoint": search_endpoint,
        "include_pull_requests": query.include_pull_requests,
        "query": {
            "state": query.state.as_github_value(),
            "labels": &query.labels,
            "assignee": &query.assignee,
            "author": &query.author,
            "since": &query.since,
            "text_query": &query.text_query,
            "sort": query.sort.as_github_value(),
            "direction": query.direction.as_github_value(),
            "page": query.page,
            "per_page": per_page,
        },
        "search": if search_endpoint {
            serde_json::json!({
                "total_count": search_total_count,
                "incomplete_results": search_incomplete_results,
                "partial_results": search_incomplete_results.unwrap_or(false),
            })
        } else {
            serde_json::Value::Null
        },
    })
}

fn github_issue_create_milestone(value: Option<&str>) -> Result<Option<u64>, String> {
    let Some(raw) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let number = raw.parse::<u64>().map_err(|_| {
        "GitHub issue milestone must be the positive decimal milestone number, not a title"
            .to_string()
    })?;
    if number == 0 || number.to_string() != raw {
        return Err(
            "GitHub issue milestone must be the positive decimal milestone number, not a title"
                .to_string(),
        );
    }
    Ok(Some(number))
}

fn issue_create_result_from_response(
    req: IssueCreateRequest,
    response: &serde_json::Value,
    issue_artifact: PathBuf,
    applied_project_fields: Vec<IssueProjectFieldValue>,
) -> IssueCreateResult {
    let issue_number = response
        .pointer("/number")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let tracker_issue_id = response.pointer("/id").and_then(serde_json::Value::as_u64);
    let issue = Issue {
        id: format!("{}#{}", req.proposal.repo, issue_number),
        repo: req.proposal.repo.clone(),
        number: issue_number,
        title: json_str(response, "/title").unwrap_or_default(),
        body: json_str(response, "/body").unwrap_or_default(),
        state: json_str(response, "/state").unwrap_or_default(),
        author: json_str(response, "/user/login").unwrap_or_default(),
        labels: github_label_names(response),
        created_at: json_str(response, "/created_at"),
        updated_at: json_str(response, "/updated_at"),
        is_pull_request: response.get("pull_request").is_some(),
        html_url: json_str(response, "/html_url"),
        source_artifact: Some(issue_artifact),
    };
    IssueCreateResult {
        tracker_issue_id,
        requested_metadata: IssueRequestedMetadata {
            labels: req.proposal.labels,
            assignees: req.proposal.assignees,
            milestone: req.proposal.milestone,
            issue_type: req.proposal.issue_type,
            issue_field_values: req.proposal.issue_field_values,
            project_fields: req.proposal.project_fields,
        },
        applied_metadata: IssueAppliedMetadata {
            labels: issue.labels.clone(),
            assignees: github_assignee_logins(response),
            milestone: github_milestone_value(response),
            issue_type: json_str(response, "/type/name"),
            issue_field_values: Vec::new(),
            project_fields: applied_project_fields,
        },
        issue,
    }
}

fn github_issue_from_value(
    repo: &str,
    value: &serde_json::Value,
    source_artifact: Option<PathBuf>,
) -> Issue {
    let number = value
        .pointer("/number")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    Issue {
        id: format!("{repo}#{number}"),
        repo: repo.to_string(),
        number,
        title: json_str(value, "/title").unwrap_or_default(),
        body: json_str(value, "/body").unwrap_or_default(),
        state: json_str(value, "/state").unwrap_or_default(),
        author: json_str(value, "/user/login").unwrap_or_default(),
        labels: github_label_names(value),
        created_at: json_str(value, "/created_at"),
        updated_at: json_str(value, "/updated_at"),
        is_pull_request: value.get("pull_request").is_some(),
        html_url: json_str(value, "/html_url"),
        source_artifact,
    }
}

fn github_header_report(headers: &reqwest::header::HeaderMap) -> String {
    let interesting = [
        "x-ratelimit-limit",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
        "x-ratelimit-used",
        "retry-after",
        "deprecation",
        "sunset",
        "x-github-request-id",
    ];
    interesting
        .iter()
        .filter_map(|name| {
            headers
                .get(*name)
                .and_then(|value| value.to_str().ok())
                .map(|value| format!("{name}: {value}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_github_rate_limit_artifacts(
    artifact_dir: &Path,
    rate_events: &[String],
    rate_trace_events: &[serde_json::Value],
) -> Result<(), String> {
    if rate_events.is_empty() {
        return Ok(());
    }
    write_file(
        artifact_dir.join("github_issue.rate_limit.log"),
        &format!("{}\n", rate_events.join("\n")),
    )?;
    write_file(
        artifact_dir.join("github_rate_limit_events.jsonl"),
        &format!(
            "{}\n",
            rate_trace_events
                .iter()
                .map(serde_json::Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
}

fn retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

struct GithubRetryDecision {
    wait: Duration,
    reason: &'static str,
}

fn github_rate_limit_trace_payload(
    attempt: usize,
    status: u16,
    decision: &GithubRetryDecision,
    headers: &reqwest::header::HeaderMap,
) -> serde_json::Value {
    serde_json::json!({
        "attempt": attempt,
        "status": status,
        "wait_seconds": decision.wait.as_secs(),
        "reason": decision.reason,
        "rate_limit": {
            "limit": header_u64(headers, "x-ratelimit-limit"),
            "remaining": header_u64(headers, "x-ratelimit-remaining"),
            "used": header_u64(headers, "x-ratelimit-used"),
            "reset": header_u64(headers, "x-ratelimit-reset"),
            "resource": header_str(headers, "x-ratelimit-resource"),
            "retry_after": header_u64(headers, "retry-after"),
        },
        "github_request_id": header_str(headers, "x-github-request-id"),
    })
}

fn github_retry_decision(
    status: reqwest::StatusCode,
    attempt: usize,
    headers: &reqwest::header::HeaderMap,
    body: &str,
) -> Option<GithubRetryDecision> {
    if !matches!(status.as_u16(), 403 | 429) {
        return None;
    }
    if let Some(seconds) = retry_after_seconds(headers) {
        return Some(GithubRetryDecision {
            wait: Duration::from_secs(seconds),
            reason: "retry-after",
        });
    }
    if rate_limit_remaining(headers) == Some(0) {
        return Some(GithubRetryDecision {
            wait: reset_wait(headers).unwrap_or_else(|| Duration::from_secs(60)),
            reason: "x-ratelimit-reset",
        });
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || (status == reqwest::StatusCode::FORBIDDEN
            && response_indicates_secondary_rate_limit(body))
    {
        return Some(GithubRetryDecision {
            wait: secondary_limit_backoff(attempt),
            reason: "secondary-rate-limit-fallback",
        });
    }
    None
}

fn rate_limit_remaining(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    header_u64(headers, "x-ratelimit-remaining")
}

fn response_indicates_secondary_rate_limit(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("secondary rate limit")
        || body.contains("secondary limit")
        || body.contains("abuse detection")
        || body.contains("abuse-rate-limits")
}

fn header_u64(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn header_str(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
}

fn reset_wait(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let reset_epoch = headers
        .get("x-ratelimit-reset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(reset_epoch.saturating_sub(now)))
}

fn secondary_limit_backoff(attempt: usize) -> Duration {
    let multiplier = 1_u64.checked_shl(attempt as u32).unwrap_or(16).min(16);
    Duration::from_secs(60 * multiplier)
}

fn json_str(value: &serde_json::Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

fn normalized_label_set(labels: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for label in labels {
        if !normalized.contains(label) {
            normalized.push(label.clone());
        }
    }
    normalized.sort();
    normalized
}

fn github_create_issue_payload(
    title: &str,
    body: &str,
    labels: &[String],
    assignees: &[String],
    milestone: Option<u64>,
    issue_type: Option<&str>,
) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "title".to_string(),
        serde_json::Value::String(title.to_string()),
    );
    payload.insert(
        "body".to_string(),
        serde_json::Value::String(body.to_string()),
    );
    if !labels.is_empty() {
        payload.insert("labels".to_string(), serde_json::json!(labels));
    }
    if !assignees.is_empty() {
        payload.insert("assignees".to_string(), serde_json::json!(assignees));
    }
    if let Some(milestone) = milestone {
        payload.insert("milestone".to_string(), serde_json::json!(milestone));
    }
    if let Some(issue_type) = issue_type {
        payload.insert(
            "type".to_string(),
            serde_json::Value::String(issue_type.to_string()),
        );
    }
    serde_json::Value::Object(payload)
}

fn github_json_request(
    method: reqwest::Method,
    url: &str,
    token: &str,
    api_version: &str,
    payload: Option<serde_json::Value>,
    artifact_dir: &Path,
    artifact_label: &str,
) -> Result<serde_json::Value, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("agentactr-cli/0.1")
        .build()
        .map_err(|e| format!("build GitHub HTTP client: {e}"))?;
    let payload_body = payload
        .map(|payload| serde_json::to_string(&payload))
        .transpose()
        .map_err(|e| format!("render GitHub JSON: {e}"))?;
    let mut rate_events = Vec::new();
    let mut rate_trace_events = Vec::new();
    let max_attempts = 5;
    let mut final_response = None;
    for attempt in 0..max_attempts {
        let mut request = client
            .request(method.clone(), url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .header("X-GitHub-Api-Version", api_version);
        if let Some(payload_body) = &payload_body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(payload_body.clone());
        }
        let response = request
            .send()
            .map_err(|e| format!("GitHub request failed: {e}"))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .text()
            .map_err(|e| format!("read GitHub response: {e}"))?;
        if let Some(decision) = github_retry_decision(status, attempt, &headers, &body) {
            let event = format!(
                "attempt={} status={} wait_seconds={} reason={} operation={}",
                attempt + 1,
                status.as_u16(),
                decision.wait.as_secs(),
                decision.reason,
                artifact_label
            );
            warn!("{event}");
            rate_events.push(event);
            rate_trace_events.push(github_rate_limit_trace_payload(
                attempt + 1,
                status.as_u16(),
                &decision,
                &headers,
            ));
            if attempt + 1 >= max_attempts {
                final_response = Some((status, headers, body));
                break;
            }
            if decision.wait > Duration::from_secs(300) {
                write_file(
                    artifact_dir.join(format!("{artifact_label}.headers")),
                    &github_header_report(&headers),
                )?;
                write_github_rate_limit_artifacts(artifact_dir, &rate_events, &rate_trace_events)?;
                return Err(format!(
                    "GitHub rate limited; retry after {} seconds ({})",
                    decision.wait.as_secs(),
                    decision.reason
                ));
            }
            thread::sleep(decision.wait);
            continue;
        }
        final_response = Some((status, headers, body));
        break;
    }
    let (status, headers, body) = final_response.ok_or("GitHub request produced no response")?;
    write_file(
        artifact_dir.join(format!("{artifact_label}.headers")),
        &github_header_report(&headers),
    )?;
    write_github_rate_limit_artifacts(artifact_dir, &rate_events, &rate_trace_events)?;
    if !status.is_success() {
        return Err(format!(
            "GitHub request failed: status={} {} body={}",
            status.as_u16(),
            github_header_report(&headers),
            body
        ));
    }
    serde_json::from_str(&body).map_err(|e| format!("parse GitHub JSON response: {e}"))
}

fn github_json_raw_request(
    method: reqwest::Method,
    url: &str,
    token: &str,
    api_version: &str,
    payload: Option<serde_json::Value>,
    artifact_dir: &Path,
    artifact_label: &str,
) -> Result<(reqwest::StatusCode, reqwest::header::HeaderMap, String), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("agentactr-cli/0.1")
        .build()
        .map_err(|e| format!("build GitHub HTTP client: {e}"))?;
    let payload_body = payload
        .map(|payload| serde_json::to_string(&payload))
        .transpose()
        .map_err(|e| format!("render GitHub JSON: {e}"))?;
    let mut request = client
        .request(method, url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .header("X-GitHub-Api-Version", api_version);
    if let Some(payload_body) = payload_body {
        request = request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload_body);
    }
    let response = request
        .send()
        .map_err(|e| format!("GitHub request failed: {e}"))?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .text()
        .map_err(|e| format!("read GitHub response: {e}"))?;
    write_file(
        artifact_dir.join(format!("{artifact_label}.headers")),
        &github_header_report(&headers),
    )?;
    write_file(artifact_dir.join(format!("{artifact_label}.json")), &body)?;
    Ok((status, headers, body))
}

fn github_graphql_request(
    query: &str,
    variables: serde_json::Value,
    token: &str,
    api_version: &str,
    artifact_dir: &Path,
    artifact_label: &str,
) -> Result<serde_json::Value, String> {
    let payload = serde_json::json!({
        "query": query,
        "variables": variables,
    });
    let response = github_json_request(
        reqwest::Method::POST,
        "https://api.github.com/graphql",
        token,
        api_version,
        Some(payload),
        artifact_dir,
        artifact_label,
    )?;
    if let Some(errors) = response.get("errors").and_then(serde_json::Value::as_array) {
        if !errors.is_empty() {
            return Err(format!(
                "GitHub GraphQL request `{artifact_label}` returned errors: {}",
                serde_json::to_string(errors)
                    .unwrap_or_else(|_| "<unrenderable graphql errors>".to_string())
            ));
        }
    }
    write_file(
        artifact_dir.join(format!("{artifact_label}.json")),
        &serde_json::to_string_pretty(&response)
            .map_err(|e| format!("render GitHub GraphQL artifact: {e}"))?,
    )?;
    Ok(response)
}

fn standard_github_label(name: &str) -> Option<&'static GithubStandardLabel> {
    GITHUB_STANDARD_LABELS
        .iter()
        .find(|label| label.name == name)
}

fn github_project_owner_login(repo: &str, configured_owner: &str) -> Result<String, String> {
    if configured_owner != "auto" {
        return Ok(configured_owner.to_string());
    }
    repo.split_once('/')
        .map(|(owner, _)| owner.to_string())
        .ok_or("repo must use OWNER/REPO".to_string())
}

fn github_project_by_number_query() -> &'static str {
    r#"
query($login: String!, $number: Int!) {
  user(login: $login) { projectV2(number: $number) { ...ProjectShape } }
  organization(login: $login) { projectV2(number: $number) { ...ProjectShape } }
}
fragment ProjectShape on ProjectV2 {
  id
  title
  number
  fields(first: 50) {
    nodes {
      __typename
      ... on ProjectV2SingleSelectField {
        id
        name
        options { id name }
      }
      ... on ProjectV2Field {
        id
        name
      }
    }
  }
}
"#
}

fn github_project_list_query() -> &'static str {
    r#"
query($login: String!, $query: String!) {
  user(login: $login) { projectsV2(first: 20, query: $query) { nodes { ...ProjectShape } } }
  organization(login: $login) { projectsV2(first: 20, query: $query) { nodes { ...ProjectShape } } }
}
fragment ProjectShape on ProjectV2 {
  id
  title
  number
  fields(first: 50) {
    nodes {
      __typename
      ... on ProjectV2SingleSelectField {
        id
        name
        options { id name }
      }
      ... on ProjectV2Field {
        id
        name
      }
    }
  }
}
"#
}

fn github_project_from_json(value: &serde_json::Value) -> Option<GithubProject> {
    if value.is_null() {
        return None;
    }
    Some(GithubProject {
        id: json_str(value, "/id")?,
        title: json_str(value, "/title")?,
        number: value.pointer("/number")?.as_u64()? as u32,
        fields: value
            .pointer("/fields/nodes")
            .and_then(serde_json::Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(github_project_field_from_json)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    })
}

fn github_project_field_from_json(value: &serde_json::Value) -> Option<GithubProjectField> {
    let id = json_str(value, "/id")?;
    let name = json_str(value, "/name")?;
    let options = value
        .pointer("/options")
        .and_then(serde_json::Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    Some(GithubProjectFieldOption {
                        id: json_str(option, "/id")?,
                        name: json_str(option, "/name")?,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(GithubProjectField { id, name, options })
}

fn project_field_options(field_name: &str) -> Result<Vec<serde_json::Value>, String> {
    let values = match field_name {
        "Priority" => vec![
            ("P0", "RED", "Critical priority"),
            ("P1", "ORANGE", "High priority"),
            ("P2", "YELLOW", "Normal priority"),
        ],
        "Size" => vec![
            ("XS", "GRAY", "Extra small change"),
            ("S", "BLUE", "Small change"),
            ("M", "GREEN", "Medium change"),
            ("L", "ORANGE", "Large change"),
            ("XL", "RED", "Extra large change"),
        ],
        _ => {
            return Err(format!(
            "GitHub ProjectV2 auto-maintenance supports only Priority and Size, got `{field_name}`"
        ))
        }
    };
    Ok(values
        .into_iter()
        .map(|(name, color, description)| {
            serde_json::json!({
                "name": name,
                "color": color,
                "description": description,
            })
        })
        .collect())
}

fn github_label_names(value: &serde_json::Value) -> Vec<String> {
    value
        .pointer("/labels")
        .and_then(serde_json::Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| json_str(label, "/name"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn github_assignee_logins(value: &serde_json::Value) -> Vec<String> {
    value
        .pointer("/assignees")
        .and_then(serde_json::Value::as_array)
        .map(|assignees| {
            assignees
                .iter()
                .filter_map(|assignee| json_str(assignee, "/login"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn github_milestone_value(value: &serde_json::Value) -> Option<String> {
    value
        .pointer("/milestone/number")
        .and_then(serde_json::Value::as_u64)
        .map(|number| number.to_string())
        .or_else(|| json_str(value, "/milestone/title"))
}

fn github_claim_marker(run_id: &str) -> String {
    format!("<!-- agentactr:lifecycle-claim schema_version=1 run_id={run_id}")
}

fn github_claim_marker_body(req: &ClaimRequest) -> String {
    format!(
        "<!-- agentactr:lifecycle-claim schema_version=1 run_id={} owner_id={} fencing_token={} expires_at={} -->\nagentactr claim lease for run `{}`.",
        req.run_id, req.owner_id, req.fencing_token, req.lease_expires_at, req.run_id
    )
}

fn github_lifecycle_comment_marker(req: &CommentRequest) -> String {
    let digest = sha256_hex_bytes(req.body.as_bytes());
    format!(
        "<!-- agentactr:lifecycle-comment schema_version=1 run_id={} kind={} key={} digest={} -->",
        req.run_id,
        req.kind.as_str(),
        req.idempotency_key,
        digest
    )
}

fn github_lifecycle_comment_marker_identity(req: &CommentRequest) -> String {
    format!(
        "<!-- agentactr:lifecycle-comment schema_version=1 run_id={} kind={} key={}",
        req.run_id,
        req.kind.as_str(),
        req.idempotency_key
    )
}

#[cfg(test)]
fn github_lifecycle_comment_marker_from_body(body: &str) -> Option<GithubLifecycleCommentMarker> {
    let start = body.find("<!-- agentactr:lifecycle-comment ")?;
    let rest = &body[start + "<!-- agentactr:lifecycle-comment ".len()..];
    let end = rest.find("-->")?;
    let marker = &rest[..end];
    let mut schema_version = None;
    let mut run_id = None;
    let mut kind = None;
    let mut key = None;
    let mut digest = None;
    for part in marker.split_whitespace() {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        match name {
            "schema_version" => schema_version = Some(value.to_string()),
            "run_id" => run_id = Some(value.to_string()),
            "kind" => kind = Some(value.to_string()),
            "key" => key = Some(value.to_string()),
            "digest" => digest = Some(value.to_string()),
            _ => {}
        }
    }
    Some(GithubLifecycleCommentMarker {
        schema_version: schema_version?,
        run_id: run_id?,
        kind: kind?,
        key: key?,
        digest: digest?,
    })
}

fn github_claim_markers_from_comments(comments: &[serde_json::Value]) -> Vec<GithubClaimMarker> {
    comments
        .iter()
        .filter_map(|comment| json_str(comment, "/body"))
        .filter_map(|body| github_claim_marker_from_body(&body))
        .collect()
}

fn github_claim_marker_from_body(body: &str) -> Option<GithubClaimMarker> {
    let start = body.find("<!-- agentactr:lifecycle-claim ")?;
    let rest = &body[start + "<!-- agentactr:lifecycle-claim ".len()..];
    let end = rest.find("-->")?;
    let marker = &rest[..end];
    let mut schema_version = None;
    let mut run_id = None;
    let mut owner_id = None;
    let mut fencing_token = None;
    let mut expires_at = None;
    for part in marker.split_whitespace() {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key {
            "schema_version" => schema_version = Some(value.to_string()),
            "run_id" => run_id = Some(value.to_string()),
            "owner_id" => owner_id = Some(value.to_string()),
            "fencing_token" => fencing_token = Some(value.to_string()),
            "expires_at" => expires_at = Some(value.to_string()),
            _ => {}
        }
    }
    Some(GithubClaimMarker {
        schema_version: schema_version?,
        run_id: run_id?,
        owner_id: owner_id?,
        fencing_token: fencing_token?,
        expires_at: expires_at?,
    })
}

fn comment_ref_from_json(
    value: &serde_json::Value,
    artifact_path: PathBuf,
    created_or_updated: &str,
) -> CommentRef {
    CommentRef {
        provider_id: value
            .pointer("/id")
            .and_then(serde_json::Value::as_u64)
            .map(|id| id.to_string())
            .unwrap_or_else(|| "missing".to_string()),
        html_url: json_str(value, "/html_url"),
        artifact_path: Some(artifact_path),
        created_or_updated: created_or_updated.to_string(),
    }
}

fn github_path_segment_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect::<String>()
}

fn safe_artifact_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn create_dir(path: impl AsRef<Path>) -> Result<(), String> {
    fs::create_dir_all(path.as_ref())
        .map_err(|e| format!("create {}: {e}", path.as_ref().display()))
}

fn write_file(path: impl AsRef<Path>, content: &str) -> Result<(), String> {
    fs::write(path.as_ref(), content).map_err(|e| format!("write {}: {e}", path.as_ref().display()))
}

pub(crate) fn validate_github_repo(repo: &str) -> Result<(), String> {
    let Some((owner, name)) = repo.split_once('/') else {
        return Err("repo must use OWNER/REPO".to_string());
    };
    if valid_github_segment(owner) && valid_github_segment(name) {
        Ok(())
    } else {
        Err("repo contains unsupported characters".to_string())
    }
}

pub(crate) fn validate_issue_number(issue: &str) -> Result<(), String> {
    if !issue.is_empty() && issue.chars().all(|c| c.is_ascii_digit()) {
        Ok(())
    } else {
        Err("issue must be a numeric GitHub issue id".to_string())
    }
}

fn github_token_from_env(configured_env: &str) -> Result<String, String> {
    env::var(configured_env)
        .or_else(|_| env::var("GITHUB_TOKEN"))
        .or_else(|_| env::var("GH_TOKEN"))
        .map_err(|_| {
            format!("missing GitHub token; set GITHUB_TOKEN, GH_TOKEN, or {configured_env}")
        })
}

fn valid_github_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn ensure_clean_git_checkout() -> Result<(), String> {
    let output = git_output(&["status", "--porcelain"])?;
    if output.trim().is_empty() {
        Ok(())
    } else {
        Err(
            "source checkout is dirty; commit/stash changes before creating an issue worktree"
                .to_string(),
        )
    }
}

fn resolve_base_commit(base_ref: &str) -> Result<String, String> {
    let rev = format!("{base_ref}^{{commit}}");
    git_output(&["rev-parse", "--verify", &rev])
        .map_err(|e| format!("resolve vcs.base_ref `{base_ref}` to an immutable commit: {e}"))
}

fn git_output(args: &[&str]) -> Result<String, String> {
    command_output("git", args)
}

fn git_output_in_worktree(worktree: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .map_err(|e| format!("git -C {} {}: {e}", worktree.display(), args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!(
            "git -C {} {} failed: {}",
            worktree.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn git_output_in_worktree_raw(worktree: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .map_err(|e| format!("git -C {} {}: {e}", worktree.display(), args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format!(
            "git -C {} {} failed: {}",
            worktree.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn git_new_file_patch(worktree: &Path, repo_relative_path: &str) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["diff", "--binary", "--no-index", "--", "/dev/null"])
        .arg(repo_relative_path)
        .output()
        .map_err(|e| {
            format!(
                "git -C {} diff --binary --no-index -- /dev/null {}: {e}",
                worktree.display(),
                repo_relative_path
            )
        })?;
    let code = output.status.code().unwrap_or_default();
    if output.status.success() || code == 1 {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format!(
            "git -C {} diff --binary --no-index -- /dev/null {} failed: {}",
            worktree.display(),
            repo_relative_path,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn git_status_in_worktree(worktree: &Path, args: &[&str]) -> Result<bool, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .map_err(|e| format!("git -C {} {}: {e}", worktree.display(), args.join(" ")))?;
    if output.status.success() {
        Ok(true)
    } else if output.status.code() == Some(1) {
        Ok(false)
    } else {
        Err(format!(
            "git -C {} {} failed: {}",
            worktree.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn parse_git_status_paths(status: &str) -> Vec<String> {
    status
        .lines()
        .filter_map(|line| {
            if line.len() < 3 {
                return None;
            }
            let path = if line.as_bytes().get(2) == Some(&b' ') {
                line[3..].trim()
            } else {
                line.get(2..).unwrap_or(line).trim_start()
            };
            let normalized = path
                .rsplit_once(" -> ")
                .map(|(_, new_path)| new_path)
                .unwrap_or(path);
            if normalized == ".agentactr-run.toml" {
                None
            } else {
                Some(normalized.to_string())
            }
        })
        .collect()
}

fn command_output(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program} {}: {e}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn github_claim_marker_parser_extracts_fencing_token_and_expiry() {
        let body = "<!-- agentactr:lifecycle-claim schema_version=1 run_id=run-1 owner_id=owner fencing_token=token-1 expires_at=2026-05-15T10:00:00.000Z -->\nbody";

        let marker = github_claim_marker_from_body(body).expect("claim marker should parse");

        assert_eq!(marker.schema_version, "1");
        assert_eq!(marker.run_id, "run-1");
        assert_eq!(marker.owner_id, "owner");
        assert_eq!(marker.fencing_token, "token-1");
        assert_eq!(marker.expires_at, "2026-05-15T10:00:00.000Z");
    }

    #[test]
    fn github_claim_marker_parser_ignores_unrelated_comments() {
        assert!(github_claim_marker_from_body("plain comment").is_none());
    }

    #[test]
    fn github_claim_marker_parser_requires_owner_and_schema() {
        assert!(github_claim_marker_from_body(
            "<!-- agentactr:lifecycle-claim run_id=run-1 fencing_token=t expires_at=2026-05-15T10:00:00.000Z -->"
        )
        .is_none());
        assert!(github_claim_marker_from_body(
            "<!-- agentactr:lifecycle-claim schema_version=1 run_id=run-1 fencing_token=t expires_at=2026-05-15T10:00:00.000Z -->"
        )
        .is_none());
    }

    #[test]
    fn lifecycle_comment_identity_is_stable_across_body_digest_changes() {
        let mut req = CommentRequest {
            repo: "OWNER/REPO".to_string(),
            issue_number: 42,
            run_id: "run-1".to_string(),
            kind: IssueCommentKind::ReviewRequired,
            idempotency_key: "run-1:42:review-required".to_string(),
            body: "first summary".to_string(),
            update_existing: true,
        };
        let first_marker = github_lifecycle_comment_marker(&req);
        let identity = github_lifecycle_comment_marker_identity(&req);
        req.body = "updated summary".to_string();
        let second_marker = github_lifecycle_comment_marker(&req);

        assert_ne!(first_marker, second_marker);
        assert!(first_marker.contains(&identity));
        assert!(second_marker.contains(&identity));
        let parsed = github_lifecycle_comment_marker_from_body(&format!("body\n\n{second_marker}"))
            .expect("comment marker should parse");
        assert_eq!(parsed.schema_version, "1");
        assert_eq!(parsed.run_id, "run-1");
        assert_eq!(parsed.kind, "review_required");
        assert_eq!(parsed.key, "run-1:42:review-required");
    }

    #[test]
    fn lifecycle_label_mutation_deduplicates_requested_labels_without_snapshot_merge() {
        let normalized = normalized_label_set(&[
            "agentactr:running".to_string(),
            "bug".to_string(),
            "agentactr:running".to_string(),
        ]);

        assert_eq!(
            normalized,
            vec!["agentactr:running".to_string(), "bug".to_string(),]
        );
    }

    #[test]
    fn github_issue_create_payload_omits_unsupported_issue_field_values() {
        let payload = github_create_issue_payload(
            "title",
            "body",
            &["bug".to_string()],
            &["octocat".to_string()],
            Some(7),
            Some("Task"),
        );

        assert_eq!(payload["title"], "title");
        assert_eq!(payload["labels"][0], "bug");
        assert_eq!(payload["assignees"][0], "octocat");
        assert_eq!(payload["milestone"], 7);
        assert_eq!(payload["type"], "Task");
        assert!(payload.get("issue_field_values").is_none());
    }

    #[test]
    fn github_issue_create_rejects_issue_field_values_before_token_lookup() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("GITHUB_TOKEN");
        env::remove_var("GH_TOKEN");
        env::remove_var("CUSTOM_GITHUB_TOKEN");
        let mut tracker_config = AgentactrConfig::strict_defaults("OWNER/REPO").tracker;
        tracker_config.token_env = "CUSTOM_GITHUB_TOKEN".to_string();
        tracker_config.github_api_version = "2026-03-10".to_string();
        let adapter = GithubRestAdapter::new(
            env::temp_dir().join(format!(
                "agentactr-github-create-field-test-{}",
                std::process::id()
            )),
            &tracker_config,
        );
        let req = IssueCreateRequest {
            proposal: agentactr_sdk::IssueProposal {
                proposal_id: agentactr_sdk::IssueProposalId::new("proposal-1"),
                repo: "OWNER/REPO".to_string(),
                parent_issue: None,
                title: "title".to_string(),
                body: "body".to_string(),
                labels: Vec::new(),
                assignees: Vec::new(),
                milestone: None,
                issue_type: None,
                issue_field_values: vec![agentactr_sdk::IssueFieldValue {
                    field_id: 1,
                    value: "High".to_string(),
                    value_type: Some("single_select".to_string()),
                }],
                project_fields: Vec::new(),
                digest: "digest".to_string(),
                dedupe: agentactr_sdk::IssueDedupeStatus::Unique,
                framework: None,
                related_issues: Vec::new(),
                provenance: Vec::new(),
            },
            body_marker: "<!-- marker -->".to_string(),
        };

        let err = adapter.create_issue(req).unwrap_err();

        assert!(err.contains("issue_field_values are not supported"));
        assert!(!err.contains("missing GitHub token"));
    }

    #[test]
    fn branch_template_uses_effective_repo_issue_and_run() {
        let branch = render_branch_template(
            "agentactr/{repo_slug}/issue-{issue_number}/{run_id}",
            "OWNER/agentactrSDK",
            "42",
            "issue-42-123",
        );

        assert_eq!(branch, "agentactr/OWNER-agentactrSDK/issue-42/issue-42-123");
    }

    #[test]
    fn prepare_worktree_overlays_uncommitted_runtime_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        let original_dir = env::current_dir().unwrap();
        let root = temp_root("runtime-config-overlay");
        fs::create_dir_all(root.join(".codex")).unwrap();
        run_git(&root, &["init"]);
        run_git(
            &root,
            &["config", "user.email", "agentactr@example.invalid"],
        );
        run_git(&root, &["config", "user.name", "agentactr test"]);
        fs::write(root.join("README.md"), "demo\n").unwrap();
        run_git(&root, &["add", "README.md"]);
        run_git(&root, &["commit", "-m", "initial"]);
        fs::write(
            root.join(".codex/config.toml"),
            "approval_policy = \"never\"\nsandbox_mode = \"workspace-write\"\n",
        )
        .unwrap();
        fs::write(
            root.join("agentactr.toml"),
            "[tracker]\nrepo = \"OWNER/REPO\"\n",
        )
        .unwrap();

        env::set_current_dir(&root).unwrap();
        let result = LocalGitAdapter.prepare_worktree_ref(WorktreeRequest {
            run_id: "issue-42-local".to_string(),
            repo: "OWNER/REPO".to_string(),
            issue: "42".to_string(),
            base_ref: "HEAD".to_string(),
            worktree_root: PathBuf::from(".agentactr/worktrees"),
            branch_template: "agentactr/{run_id}".to_string(),
            fail_on_dirty_source_checkout: false,
            copy_runtime_config_to_worktree: true,
        });
        env::set_current_dir(original_dir).unwrap();

        let worktree = result.unwrap();
        let codex_config = fs::read_to_string(worktree.path.join(".codex/config.toml")).unwrap();
        let agentactr_config = fs::read_to_string(worktree.path.join("agentactr.toml")).unwrap();
        let metadata = fs::read_to_string(worktree.path.join(".agentactr-run.toml")).unwrap();
        assert!(codex_config.contains("approval_policy = \"never\""));
        assert!(!codex_config.contains("[profiles.agentactr]"));
        assert!(agentactr_config.contains("OWNER/REPO"));
        assert!(metadata.contains("runtime_config_overlay"));
        assert!(metadata.contains(".codex/config.toml"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn github_retry_ignores_plain_forbidden_responses() {
        let headers = HeaderMap::new();

        let decision = github_retry_decision(reqwest::StatusCode::FORBIDDEN, 0, &headers, "");

        assert!(decision.is_none());
    }

    #[test]
    fn github_retry_ignores_forbidden_with_rate_headers_without_secondary_body() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("42"));
        headers.insert("x-ratelimit-resource", HeaderValue::from_static("core"));

        let decision = github_retry_decision(
            reqwest::StatusCode::FORBIDDEN,
            0,
            &headers,
            r#"{"message":"Resource not accessible by integration"}"#,
        );

        assert!(decision.is_none());
    }

    #[test]
    fn github_retry_uses_primary_rate_limit_reset_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("0"));

        let decision = github_retry_decision(reqwest::StatusCode::FORBIDDEN, 0, &headers, "")
            .expect("primary rate limit should retry");

        assert_eq!(decision.reason, "x-ratelimit-reset");
    }

    #[test]
    fn github_retry_uses_secondary_fallback_for_429() {
        let headers = HeaderMap::new();

        let decision =
            github_retry_decision(reqwest::StatusCode::TOO_MANY_REQUESTS, 1, &headers, "")
                .expect("429 should use secondary fallback");

        assert_eq!(decision.reason, "secondary-rate-limit-fallback");
        assert_eq!(decision.wait, Duration::from_secs(120));
    }

    #[test]
    fn github_retry_uses_secondary_fallback_for_forbidden_with_secondary_body() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("42"));
        headers.insert("x-ratelimit-resource", HeaderValue::from_static("core"));

        let decision = github_retry_decision(
            reqwest::StatusCode::FORBIDDEN,
            0,
            &headers,
            r#"{"message":"You have exceeded a secondary rate limit."}"#,
        )
        .expect("403 secondary limit should use documented fallback");

        assert_eq!(decision.reason, "secondary-rate-limit-fallback");
        assert_eq!(decision.wait, Duration::from_secs(60));
    }

    #[test]
    fn github_search_terms_exclude_pull_requests_by_default() {
        let query = CandidateQuery {
            repo: "OWNER/REPO".to_string(),
            text_query: Some("cache bug".to_string()),
            labels: vec!["bug".to_string()],
            ..CandidateQuery::default()
        };

        let terms = github_issue_search_terms(&query, "cache bug");

        assert!(terms.contains("repo:OWNER/REPO"));
        assert!(terms.contains("is:issue"));
        assert!(terms.contains("label:\"bug\""));
        assert!(terms.contains("cache bug"));
    }

    #[test]
    fn github_search_terms_allow_pull_requests_when_requested() {
        let query = CandidateQuery {
            repo: "OWNER/REPO".to_string(),
            text_query: Some("router".to_string()),
            include_pull_requests: true,
            ..CandidateQuery::default()
        };

        let terms = github_issue_search_terms(&query, "router");

        assert!(terms.contains("repo:OWNER/REPO"));
        assert!(!terms.contains("is:issue"));
        assert!(!terms.contains("type:issue"));
        assert!(terms.contains("router"));
    }

    #[test]
    fn github_issue_list_url_omits_unset_optional_filters() {
        let query = CandidateQuery {
            repo: "OWNER/REPO".to_string(),
            assignee: None,
            author: None,
            since: None,
            labels: Vec::new(),
            ..CandidateQuery::default()
        };

        let url = github_issue_list_url(&query, "1", "50").unwrap();
        let rendered = url.as_str();

        assert!(rendered.contains("state=open"));
        assert!(rendered.contains("sort=updated"));
        assert!(!rendered.contains("assignee="));
        assert!(!rendered.contains("creator="));
        assert!(!rendered.contains("since="));
        assert!(!rendered.contains("labels="));
    }

    #[test]
    fn github_issue_list_url_preserves_explicit_optional_filters() {
        let query = CandidateQuery {
            repo: "OWNER/REPO".to_string(),
            assignee: Some("octocat".to_string()),
            author: Some("mona".to_string()),
            since: Some("2026-05-14T00:00:00Z".to_string()),
            labels: vec!["bug".to_string(), "needs review".to_string()],
            ..CandidateQuery::default()
        };

        let url = github_issue_list_url(&query, "2", "25").unwrap();
        let rendered = url.as_str();

        assert!(rendered.contains("assignee=octocat"));
        assert!(rendered.contains("creator=mona"));
        assert!(rendered.contains("since=2026-05-14T00%3A00%3A00Z"));
        assert!(rendered.contains("labels=bug%2Cneeds+review"));
        assert!(rendered.contains("page=2"));
        assert!(rendered.contains("per_page=25"));
    }

    #[test]
    fn github_candidate_summary_records_search_completeness_metadata() {
        let query = CandidateQuery {
            repo: "OWNER/REPO".to_string(),
            text_query: Some("memory pressure".to_string()),
            ..CandidateQuery::default()
        };
        let mut total_count = None;
        let mut incomplete_results = None;
        capture_github_issue_search_metadata(
            &serde_json::json!({
                "total_count": 123,
                "incomplete_results": true,
                "items": [],
            }),
            &mut total_count,
            &mut incomplete_results,
        );

        let summary =
            github_issue_candidates_summary(&query, 50, 50, 50, total_count, incomplete_results);

        assert_eq!(summary["endpoint"], "search/issues");
        assert_eq!(summary["search"]["total_count"], 123);
        assert_eq!(summary["search"]["incomplete_results"], true);
        assert_eq!(summary["search"]["partial_results"], true);
    }

    #[test]
    fn github_issue_create_milestone_accepts_canonical_number() {
        assert_eq!(github_issue_create_milestone(Some("12")).unwrap(), Some(12));
        assert_eq!(github_issue_create_milestone(None).unwrap(), None);
        assert_eq!(github_issue_create_milestone(Some("   ")).unwrap(), None);
    }

    #[test]
    fn github_issue_create_milestone_rejects_titles_and_noncanonical_numbers() {
        let title_err = github_issue_create_milestone(Some("v1")).unwrap_err();
        let leading_zero_err = github_issue_create_milestone(Some("012")).unwrap_err();
        let zero_err = github_issue_create_milestone(Some("0")).unwrap_err();

        assert!(title_err.contains("milestone number"));
        assert!(leading_zero_err.contains("milestone number"));
        assert!(zero_err.contains("milestone number"));
    }

    #[test]
    fn standard_github_label_set_is_limited_to_known_representative_labels() {
        let names = GITHUB_STANDARD_LABELS
            .iter()
            .map(|label| label.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "bug",
                "dependencies",
                "documentation",
                "duplicate",
                "enhancement",
                "go",
                "good first issue",
                "help wanted",
                "invalid",
                "python:uv",
                "question",
                "tool",
                "wontfix"
            ]
        );
        assert!(standard_github_label("bug").is_some());
        assert!(standard_github_label("agentactr:running").is_none());
    }

    #[test]
    fn project_field_options_cover_priority_and_size_only() {
        let priority = project_field_options("Priority").unwrap();
        let size = project_field_options("Size").unwrap();
        let unsupported = project_field_options("Status").unwrap_err();

        assert_eq!(priority.len(), 3);
        assert_eq!(priority[0]["name"], "P0");
        assert_eq!(size.len(), 5);
        assert_eq!(size[4]["name"], "XL");
        assert!(unsupported.contains("Priority and Size"));
    }

    #[test]
    fn github_project_parses_single_select_fields() {
        let value = serde_json::json!({
            "id": "PVT_kwDO",
            "title": "Agentactr",
            "number": 7,
            "fields": {
                "nodes": [
                    {
                        "__typename": "ProjectV2SingleSelectField",
                        "id": "PVTSSF_lADO",
                        "name": "Priority",
                        "options": [
                            { "id": "p0", "name": "P0" },
                            { "id": "p1", "name": "P1" }
                        ]
                    }
                ]
            }
        });

        let project = github_project_from_json(&value).unwrap();

        assert_eq!(project.title, "Agentactr");
        assert_eq!(project.number, 7);
        assert_eq!(project.fields[0].name, "Priority");
        assert_eq!(project.fields[0].options[1].name, "P1");
    }

    #[test]
    fn github_rate_limit_artifacts_are_written_for_early_returns() {
        let root = env::temp_dir().join(format!(
            "agentactr-github-rate-artifacts-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let events =
            vec!["attempt=1 status=403 wait_seconds=3600 reason=x-ratelimit-reset".to_string()];
        let trace_events = vec![serde_json::json!({
            "attempt": 1,
            "status": 403,
            "wait_seconds": 3600,
            "reason": "x-ratelimit-reset",
        })];

        write_github_rate_limit_artifacts(&root, &events, &trace_events).unwrap();

        let log = fs::read_to_string(root.join("github_issue.rate_limit.log")).unwrap();
        let jsonl = fs::read_to_string(root.join("github_rate_limit_events.jsonl")).unwrap();
        let _ = fs::remove_dir_all(&root);
        assert!(log.contains("wait_seconds=3600"));
        assert!(jsonl.contains(r#""reason":"x-ratelimit-reset""#));
    }

    #[test]
    fn runtime_process_observe_persists_trace_and_artifact_events() {
        let root = env::temp_dir().join(format!(
            "agentactr-runtime-process-observe-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let artifact_dir = root.join("artifacts");
        let trace_path = root.join("trace").join("events.jsonl");
        let supervisor = CliCodexMemorySupervisor::new(
            std::collections::HashMap::new(),
            artifact_dir.clone(),
            trace_path.clone(),
            "OWNER/REPO".to_string(),
            "42".to_string(),
        );
        let attribution = agentactr_sdk::RuntimeProcessAttribution::new(
            agentactr_sdk::RunId::new("run-1"),
            agentactr_sdk::AgentRunId::new("agent-1"),
            agentactr_sdk::RuntimeKind::new("codex"),
            agentactr_sdk::RuntimeTransportKind::new("cli_json"),
            agentactr_sdk::RuntimeProcessModel::OneShotProcess,
        )
        .with_parent_agent_run_id(agentactr_sdk::AgentRunId::new("agent-parent"))
        .with_root_pid(agentactr_sdk::ProcessId(123))
        .with_process_group_id(agentactr_sdk::ProcessGroupId(123))
        .with_memory_group_id(MemoryGroupId::new("memory-1"));
        let event = RuntimeProcessEvent::new(RuntimeProcessEventKind::Started, attribution);

        supervisor.observe(&event).unwrap();

        let artifact =
            fs::read_to_string(artifact_dir.join("runtime_process_events.jsonl")).unwrap();
        let trace = fs::read_to_string(&trace_path).unwrap();
        let trace_event: serde_json::Value =
            serde_json::from_str(trace.lines().next().unwrap()).unwrap();
        let _ = fs::remove_dir_all(root);

        assert!(artifact.contains(r#""kind":"started""#));
        assert!(artifact.contains(r#""memory_group_id":"memory-1""#));
        assert!(artifact.contains(r#""parent_agent_run_id":"agent-parent""#));
        assert!(trace_event["ts"].as_str().unwrap().ends_with('Z'));
        assert!(trace_event["ts_unix_ms"].is_number());
        assert!(trace.contains(r#""event_type":"runtime.process.started""#));
        assert!(trace.contains(r#""issue_id":"github:OWNER/REPO#42""#));
        assert!(trace.contains(r#""parent_agent_run_id":"agent-parent""#));
        assert!(trace.contains(r#""span_id":"span:run-1:agent-1:runtime.process""#));
        assert!(trace.contains(r#""parent_span_id":"span:run-1:agent-parent:runtime.process""#));
    }

    #[test]
    fn github_rate_limit_payload_preserves_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("5000"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        headers.insert("x-ratelimit-used", HeaderValue::from_static("5000"));
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("12345"));
        headers.insert("x-ratelimit-resource", HeaderValue::from_static("core"));
        headers.insert("x-github-request-id", HeaderValue::from_static("ABC:123"));
        let decision = GithubRetryDecision {
            wait: Duration::from_secs(60),
            reason: "x-ratelimit-reset",
        };

        let payload = github_rate_limit_trace_payload(2, 403, &decision, &headers);

        assert_eq!(payload["attempt"], 2);
        assert_eq!(payload["status"], 403);
        assert_eq!(payload["wait_seconds"], 60);
        assert_eq!(payload["reason"], "x-ratelimit-reset");
        assert_eq!(payload["rate_limit"]["remaining"], 0);
        assert_eq!(payload["rate_limit"]["resource"], "core");
        assert_eq!(payload["github_request_id"], "ABC:123");
    }

    #[test]
    fn github_token_prefers_configured_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = save_env(&["GITHUB_TOKEN", "GH_TOKEN", "AGENTACTR_TEST_GITHUB_TOKEN"]);
        env::set_var("GITHUB_TOKEN", "generic-github-token");
        env::set_var("GH_TOKEN", "generic-gh-token");
        env::set_var("AGENTACTR_TEST_GITHUB_TOKEN", "configured-token");

        let token = github_token_from_env("AGENTACTR_TEST_GITHUB_TOKEN").unwrap();

        restore_env(saved);
        assert_eq!(token, "configured-token");
    }

    #[test]
    fn github_token_falls_back_to_generic_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = save_env(&["GITHUB_TOKEN", "GH_TOKEN", "AGENTACTR_TEST_GITHUB_TOKEN"]);
        env::set_var("GITHUB_TOKEN", "generic-github-token");
        env::remove_var("GH_TOKEN");
        env::remove_var("AGENTACTR_TEST_GITHUB_TOKEN");

        let token = github_token_from_env("AGENTACTR_TEST_GITHUB_TOKEN").unwrap();

        restore_env(saved);
        assert_eq!(token, "generic-github-token");
    }

    fn save_env(names: &[&str]) -> Vec<(String, Option<OsString>)> {
        names
            .iter()
            .map(|name| ((*name).to_string(), env::var_os(name)))
            .collect()
    }

    fn restore_env(saved: Vec<(String, Option<OsString>)>) {
        for (name, value) in saved {
            if let Some(value) = value {
                env::set_var(&name, value);
            } else {
                env::remove_var(&name);
            }
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let epoch_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = env::temp_dir().join(format!(
            "agentactr-adapters-{name}-{epoch_nanos}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn run_git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "git {} exited {status}", args.join(" "));
    }
}
