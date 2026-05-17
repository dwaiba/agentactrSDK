use crate::adapters::{validate_github_repo, validate_issue_number};
use crate::trace_command::{latest_run_status, read_trace_records};
use crate::vcs_adapter::LocalGitAdapter;
use crate::{
    append_run_event, collect_vcs_inventory, collect_vcs_status, collect_workspace_diff,
    flag_value, git_output_in_dir, has_flag, load_agentactr_config, load_run_artifact_context,
    new_run_id, not_implemented, print_vcs_inventory, print_vcs_inventory_json, print_vcs_show,
    print_vcs_show_json, print_vcs_status, record_workspace_diff_artifacts, run_trace_path,
    validate_run_worktree_scope, RunEventContext,
};
use std::env;
use std::path::Path;

pub(crate) fn cmd_vcs(args: &mut [String]) -> Result<(), String> {
    if args.get(1).map(String::as_str) == Some("list") {
        let config = load_agentactr_config(None)?;
        let entries = collect_vcs_inventory(&config)?;
        match args.get(2).map(String::as_str) {
            None => {
                print_vcs_inventory(&entries);
                return Ok(());
            }
            Some("--json") => {
                print_vcs_inventory_json(&entries)?;
                return Ok(());
            }
            _ => return Err("usage: agentactr vcs list [--json]".to_string()),
        }
    }
    if args.get(1).map(String::as_str) == Some("show") {
        let run_id = args
            .get(2)
            .ok_or("usage: agentactr vcs show RUN_ID [--json]")?;
        let config = load_agentactr_config(None)?;
        let mut context = load_run_artifact_context(&config, run_id)?;
        context.worktree = validate_run_worktree_scope(&config, &context)?;
        let status = collect_vcs_status(&context)?;
        let trace_records = read_trace_records(&run_trace_path(&config)?)?;
        let last_run_status = latest_run_status(&trace_records, run_id);
        match args.get(3).map(String::as_str) {
            None => {
                print_vcs_show(&config, &status, &last_run_status)?;
                return Ok(());
            }
            Some("--json") => {
                print_vcs_show_json(&config, &status, &last_run_status)?;
                return Ok(());
            }
            _ => return Err("usage: agentactr vcs show RUN_ID [--json]".to_string()),
        }
    }
    if args.get(1).map(String::as_str) == Some("status") {
        let run_id = args.get(2).ok_or("usage: agentactr vcs status RUN_ID")?;
        let config = load_agentactr_config(None)?;
        let mut context = load_run_artifact_context(&config, run_id)?;
        context.worktree = validate_run_worktree_scope(&config, &context)?;
        let status = collect_vcs_status(&context)?;
        print_vcs_status(&status);
        append_run_event(
            &RunEventContext::root(&config, &context.run_id, &context.repo, &context.issue),
            "vcs.status.read",
            status.to_json(),
        )?;
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("diff") {
        let run_id = args
            .get(2)
            .ok_or("usage: agentactr vcs diff RUN_ID [--output PATH]")?;
        if args.len() != 3
            && !(args.len() == 5 && args.get(3).map(String::as_str) == Some("--output"))
        {
            return Err("usage: agentactr vcs diff RUN_ID [--output PATH]".to_string());
        }
        let output_path = flag_value(args, "--output");
        let config = load_agentactr_config(None)?;
        let mut context = load_run_artifact_context(&config, run_id)?;
        context.worktree = validate_run_worktree_scope(&config, &context)?;
        let diff = collect_workspace_diff(&context)?;
        let (patch_path, metadata_path) =
            record_workspace_diff_artifacts(&config, &context, &diff, output_path.as_deref())?;
        println!("workspace_diff={}", patch_path.display());
        println!("workspace_diff_metadata={}", metadata_path.display());
        println!("run_id={}", diff.run_id);
        println!("base_commit={}", diff.base_commit);
        println!("current_commit={}", diff.current_commit);
        println!("patch_bytes={}", diff.patch.len());
        println!("touched_file_count={}", diff.touched_files.len());
        println!("untracked_file_count={}", diff.untracked_files.len());
        println!("is_empty={}", diff.is_empty);
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("apply") {
        return cmd_vcs_apply(args);
    }
    if let Some(command @ ("commit" | "cleanup")) = args.get(1).map(String::as_str) {
        return not_implemented(&format!("vcs {command}"));
    }
    if args.get(1).map(String::as_str) != Some("prepare") {
        return Err(
            "usage: agentactr vcs prepare --issue 123 [--repo OWNER/REPO] | vcs list [--json] | vcs show RUN_ID [--json] | vcs status RUN_ID | vcs diff RUN_ID [--output PATH] | vcs apply RUN_ID --check|--yes [--3way] [--allow-dirty]"
                .to_string(),
        );
    }
    let issue = flag_value(args, "--issue").ok_or("missing --issue")?;
    let repo_override = flag_value(args, "--repo");
    let config = load_agentactr_config(repo_override.as_deref())?;
    validate_github_repo(&config.tracker.repo)?;
    validate_issue_number(&issue)?;
    let run_id = new_run_id(&issue);
    let worktree =
        LocalGitAdapter.prepare_worktree(&run_id, &config.tracker.repo, &issue, &config.vcs)?;
    println!("run id: {run_id}");
    println!("worktree: {}", worktree.display());
    Ok(())
}

fn cmd_vcs_apply(args: &mut [String]) -> Result<(), String> {
    validate_vcs_apply_args(args)?;
    let run_id = args
        .get(2)
        .ok_or("usage: agentactr vcs apply RUN_ID --check|--yes [--3way] [--allow-dirty]")?;
    let check = has_flag(args, "--check");
    let yes = has_flag(args, "--yes");
    if check == yes {
        return Err(
            "usage: agentactr vcs apply RUN_ID --check|--yes [--3way] [--allow-dirty]".to_string(),
        );
    }
    let config = load_agentactr_config(None)?;
    let mut context = load_run_artifact_context(&config, run_id)?;
    context.worktree = validate_run_worktree_scope(&config, &context)?;
    let diff = collect_workspace_diff(&context)?;
    let (patch_path, metadata_path) =
        record_workspace_diff_artifacts(&config, &context, &diff, None)?;
    let source_root = env::current_dir().map_err(|e| format!("read current directory: {e}"))?;
    let result = apply_recorded_patch(
        &source_root,
        &patch_path,
        check,
        yes,
        has_flag(args, "--3way") || has_flag(args, "--three-way"),
        has_flag(args, "--allow-dirty"),
    )?;
    println!("run_id={}", context.run_id);
    println!("workspace_diff={}", patch_path.display());
    println!("workspace_diff_metadata={}", metadata_path.display());
    println!("source_checkout={}", source_root.display());
    println!("apply_mode={}", if check { "check" } else { "apply" });
    println!("three_way={}", result.three_way);
    println!("applied={}", result.applied);
    println!("status={}", result.status);
    Ok(())
}

const VCS_APPLY_USAGE: &str =
    "usage: agentactr vcs apply RUN_ID --check|--yes [--3way] [--allow-dirty]";
const VCS_APPLY_BOOL_FLAGS: &[&str] =
    &["--check", "--yes", "--3way", "--three-way", "--allow-dirty"];

fn validate_vcs_apply_args(args: &[String]) -> Result<(), String> {
    if args.get(1).map(String::as_str) != Some("apply") {
        return Err(VCS_APPLY_USAGE.to_string());
    }
    let Some(run_id) = args.get(2) else {
        return Err(VCS_APPLY_USAGE.to_string());
    };
    if run_id.starts_with("--") {
        return Err(format!(
            "agentactr vcs apply requires RUN_ID before flags; got `{run_id}`; {VCS_APPLY_USAGE}"
        ));
    }
    let mut index = 3;
    while index < args.len() {
        let arg = &args[index];
        if VCS_APPLY_BOOL_FLAGS.contains(&arg.as_str()) {
            index += 1;
            continue;
        }
        if arg.starts_with("--") {
            return Err(format!(
                "unknown agentactr vcs apply flag `{arg}`; {VCS_APPLY_USAGE}"
            ));
        }
        return Err(format!(
            "unexpected agentactr vcs apply argument `{arg}`; {VCS_APPLY_USAGE}"
        ));
    }
    Ok(())
}

pub(crate) struct PatchApplyResult {
    pub(crate) applied: bool,
    pub(crate) three_way: bool,
    pub(crate) status: String,
}

pub(crate) fn apply_recorded_patch(
    source_root: &Path,
    patch_path: &Path,
    check: bool,
    yes: bool,
    three_way: bool,
    allow_dirty: bool,
) -> Result<PatchApplyResult, String> {
    if yes && !allow_dirty {
        let status = git_output_in_dir(source_root, &["status", "--porcelain"])?;
        if !status.trim().is_empty() {
            return Err(
                "source checkout is dirty; commit/stash changes or pass --allow-dirty".to_string(),
            );
        }
    }
    let mut args = Vec::new();
    args.push("apply");
    if three_way {
        args.push("--3way");
    }
    if check {
        args.push("--check");
    }
    args.push("--");
    let patch_arg = patch_path
        .to_str()
        .ok_or_else(|| format!("patch path is not valid UTF-8: {}", patch_path.display()))?;
    args.push(patch_arg);
    run_git_apply(source_root, &args)?;
    Ok(PatchApplyResult {
        applied: yes,
        three_way,
        status: if check {
            "patch_applies_cleanly".to_string()
        } else {
            "patch_applied".to_string()
        },
    })
}

fn run_git_apply(source_root: &Path, args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(source_root)
        .args(args)
        .output()
        .map_err(|e| format!("git -C {} {}: {e}", source_root.display(), args.join(" ")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git -C {} {} failed with {}: {}",
            source_root.display(),
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcs_apply_args_reject_unknown_flags_before_side_effects() {
        let args = vec![
            "vcs".to_string(),
            "apply".to_string(),
            "run-1".to_string(),
            "--check".to_string(),
            "--bogus".to_string(),
        ];

        let err = validate_vcs_apply_args(&args).unwrap_err();

        assert!(err.contains("unknown agentactr vcs apply flag `--bogus`"));
    }

    #[test]
    fn vcs_apply_args_reject_missing_run_id_and_stray_values() {
        let missing = vec![
            "vcs".to_string(),
            "apply".to_string(),
            "--check".to_string(),
        ];
        let err = validate_vcs_apply_args(&missing).unwrap_err();
        assert!(err.contains("requires RUN_ID before flags"));

        let stray = vec![
            "vcs".to_string(),
            "apply".to_string(),
            "run-1".to_string(),
            "--check".to_string(),
            "extra".to_string(),
        ];
        let err = validate_vcs_apply_args(&stray).unwrap_err();
        assert!(err.contains("unexpected agentactr vcs apply argument `extra`"));
    }
}
