use agentactr_sdk::{
    AdapterCapabilities, AdapterVersionReport, CommitRef, CommitRequest, MergePlan,
    MergePlanRequest, PortResult, VcsCapabilities, VcsConfig, VersionControl, WorkspaceDiff,
    WorktreeRef, WorktreeRequest,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

    pub(crate) fn prepare_worktree_ref(&self, req: WorktreeRequest) -> Result<WorktreeRef, String> {
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
                "git_worktree_prepare".to_string(),
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

    fn detect(&self, _root: &Path) -> PortResult<VcsCapabilities> {
        git_output(&["status", "--porcelain"])?;
        Ok(VcsCapabilities)
    }

    fn prepare_workspace(&self, req: WorktreeRequest) -> PortResult<WorktreeRef> {
        if req.run_id.trim().is_empty() {
            return Err("worktree request requires run_id".into());
        }
        Ok(self.prepare_worktree_ref(req)?)
    }

    fn diff(&self, worktree: &WorktreeRef) -> PortResult<WorkspaceDiff> {
        if worktree.run_id.trim().is_empty() {
            return Err("workspace diff requires run_id".into());
        }
        if !worktree.path.is_dir() {
            return Err(format!(
                "workspace diff worktree is missing or not a directory: {}",
                worktree.path.display()
            )
            .into());
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

    fn commit(&self, _req: CommitRequest) -> PortResult<CommitRef> {
        Err("commit is not implemented in this milestone".into())
    }

    fn merge_plan(&self, req: MergePlanRequest) -> PortResult<MergePlan> {
        if req.worktree.run_id.trim().is_empty() {
            return Err("merge plan requires run_id".into());
        }
        if !req.worktree.path.is_dir() {
            return Err(format!(
                "merge plan worktree is missing or not a directory: {}",
                req.worktree.path.display()
            )
            .into());
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
            recommendation,
            blockers,
            warnings,
            touched_files,
        })
    }
}

pub(crate) fn render_branch_template(
    template: &str,
    repo: &str,
    issue: &str,
    run_id: &str,
) -> String {
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

fn create_dir(path: impl AsRef<Path>) -> Result<(), String> {
    fs::create_dir_all(path.as_ref())
        .map_err(|e| format!("create {}: {e}", path.as_ref().display()))
}

fn write_file(path: impl AsRef<Path>, content: &str) -> Result<(), String> {
    fs::write(path.as_ref(), content).map_err(|e| format!("write {}: {e}", path.as_ref().display()))
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
