use crate::artifacts::{collect_artifact_integrity, ArtifactIntegrityContext};
use crate::trace_command::{compact_json, read_trace_records, TraceRecord};
use crate::{
    collect_vcs_status, create_dir, current_epoch_millis, iso_timestamp_from_epoch_millis,
    load_agentactr_config, load_run_artifact_context, render_vcs_status_text, resolve_config_path,
    run_trace_path, validate_run_id, validate_run_worktree_scope, write_file,
};
use agentactr_sdk::AgentactrConfig;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct DebugBundleReport {
    pub(crate) bundle_dir: PathBuf,
    pub(crate) copied_files: Vec<String>,
    pub(crate) trace_events: usize,
    pub(crate) redacted: bool,
}

pub(crate) fn cmd_debug(args: &mut [String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("bundle") => {
            let run_id = args.get(2).ok_or("usage: agentactr debug bundle RUN_ID")?;
            let config = load_agentactr_config(None)?;
            let report = create_debug_bundle(&config, run_id)?;
            println!("debug_bundle={}", report.bundle_dir.display());
            println!("copied_files={}", report.copied_files.len());
            println!("trace_events={}", report.trace_events);
            println!("redacted={}", report.redacted);
            Ok(())
        }
        _ => Err("usage: agentactr debug bundle RUN_ID".to_string()),
    }
}

pub(crate) fn create_debug_bundle(
    config: &AgentactrConfig,
    run_id: &str,
) -> Result<DebugBundleReport, String> {
    let context = load_run_artifact_context(config, run_id)?;
    let bundle_dir = debug_bundle_dir(config, run_id)?;
    let artifacts_dir = bundle_dir.join("artifacts");
    let traces_dir = bundle_dir.join("traces");
    create_dir(&artifacts_dir)?;
    create_dir(&traces_dir)?;

    let redacted = config.observability.redact_secrets;
    let mut copied_files = Vec::new();
    copy_debug_artifacts(
        &context.artifact_dir,
        &artifacts_dir,
        &bundle_dir,
        redacted,
        &mut copied_files,
    )?;

    let trace_path = run_trace_path(config)?;
    let trace_records = read_trace_records(&trace_path)?;
    let run_records = trace_records
        .iter()
        .filter(|record| record.run_id == run_id)
        .cloned()
        .collect::<Vec<_>>();
    let trace_slice_path = traces_dir.join("events.jsonl");
    write_trace_slice(&trace_slice_path, &run_records, redacted)?;
    copied_files.push(relative_debug_path(&bundle_dir, &trace_slice_path)?);

    let trace_summary_path = traces_dir.join("summary.txt");
    write_file(
        &trace_summary_path,
        &render_trace_summary(&trace_path, run_id, &run_records),
    )?;
    copied_files.push(relative_debug_path(&bundle_dir, &trace_summary_path)?);

    let artifact_integrity = collect_artifact_integrity(&ArtifactIntegrityContext {
        run_id: &context.run_id,
        artifact_dir: &context.artifact_dir,
    })?;
    let artifact_integrity_path = bundle_dir.join("artifact_integrity.json");
    write_file(
        &artifact_integrity_path,
        &serde_json::to_string_pretty(&artifact_integrity)
            .map_err(|e| format!("render artifact integrity: {e}"))?,
    )?;
    copied_files.push(relative_debug_path(&bundle_dir, &artifact_integrity_path)?);

    let vcs_context = validate_run_worktree_scope(config, &context).map(|worktree| {
        let mut context = context.clone();
        context.worktree = worktree;
        context
    });
    match vcs_context.and_then(|context| collect_vcs_status(&context)) {
        Ok(status) => {
            let vcs_json = bundle_dir.join("vcs_status.json");
            write_file(
                &vcs_json,
                &serde_json::to_string_pretty(&status.to_json())
                    .map_err(|e| format!("render VCS status: {e}"))?,
            )?;
            copied_files.push(relative_debug_path(&bundle_dir, &vcs_json)?);
            let vcs_text = bundle_dir.join("vcs_status.txt");
            write_file(&vcs_text, &render_vcs_status_text(&status))?;
            copied_files.push(relative_debug_path(&bundle_dir, &vcs_text)?);
        }
        Err(err) => {
            let path = bundle_dir.join("vcs_status.error.txt");
            write_file(&path, &format!("{err}\n"))?;
            copied_files.push(relative_debug_path(&bundle_dir, &path)?);
        }
    }

    let manifest_path = bundle_dir.join("bundle_manifest.json");
    let manifest = serde_json::json!({
        "schema_version": "0.1",
        "run_id": context.run_id,
        "repo": context.repo,
        "issue": context.issue,
        "generated_at": iso_timestamp_from_epoch_millis(current_epoch_millis()),
        "source_artifact_dir": context.artifact_dir.display().to_string(),
        "source_context_manifest": context.manifest_path.display().to_string(),
        "source_trace_path": trace_path.display().to_string(),
        "redacted": redacted,
        "trace_events": run_records.len(),
        "artifact_integrity": artifact_integrity,
        "files": copied_files,
        "limitations": [
            "bootstrap-local bundle; replay remains a milestone command",
            "merge plan is read-only; commit and GitHub mutations remain disabled",
            "GitHub mutations are not performed by debug bundling"
        ]
    });
    write_file(
        &manifest_path,
        &serde_json::to_string_pretty(&manifest)
            .map_err(|e| format!("render debug bundle manifest: {e}"))?,
    )?;
    let copied_files = manifest
        .get("files")
        .and_then(serde_json::Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(DebugBundleReport {
        bundle_dir,
        copied_files,
        trace_events: run_records.len(),
        redacted,
    })
}

pub(crate) fn debug_bundle_dir(config: &AgentactrConfig, run_id: &str) -> Result<PathBuf, String> {
    validate_run_id(run_id)?;
    Ok(resolve_config_path(&config.observability.debug_bundle_root)?.join(run_id))
}

fn copy_debug_artifacts(
    source: &Path,
    target: &Path,
    bundle_root: &Path,
    redacted: bool,
    copied_files: &mut Vec<String>,
) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source)
        .map_err(|e| format!("inspect debug source {}: {e}", source.display()))?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(format!(
            "run artifact directory is missing or not a directory: {}",
            source.display()
        ));
    }
    for entry in fs::read_dir(source).map_err(|e| format!("read {}: {e}", source.display()))? {
        let entry = entry.map_err(|e| format!("read {} entry: {e}", source.display()))?;
        let path = entry.path();
        let target_path = target.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)
            .map_err(|e| format!("inspect debug artifact {}: {e}", path.display()))?;
        if metadata.file_type().is_symlink() {
            record_debug_symlink(&path, &target_path, bundle_root, copied_files)?;
        } else if metadata.is_dir() {
            create_dir(&target_path)?;
            copy_debug_artifacts(&path, &target_path, bundle_root, redacted, copied_files)?;
        } else if metadata.is_file() {
            copy_debug_file(&path, &target_path, redacted)?;
            copied_files.push(relative_debug_path(bundle_root, &target_path)?);
        }
    }
    Ok(())
}

fn record_debug_symlink(
    source: &Path,
    target_path: &Path,
    bundle_root: &Path,
    copied_files: &mut Vec<String>,
) -> Result<(), String> {
    let metadata_path = target_path.with_file_name(format!(
        "{}.symlink_skipped.json",
        target_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact")
    ));
    let symlink_target = fs::read_link(source)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "<unreadable>".to_string());
    let payload = serde_json::json!({
        "schema_version": "0.1",
        "action": "skipped_symlink",
        "source": source.display().to_string(),
        "symlink_target": symlink_target,
        "reason": "debug bundles do not follow symlinked run artifacts",
    });
    write_file(
        &metadata_path,
        &serde_json::to_string_pretty(&payload)
            .map_err(|e| format!("render skipped symlink metadata: {e}"))?,
    )?;
    copied_files.push(relative_debug_path(bundle_root, &metadata_path)?);
    Ok(())
}

fn copy_debug_file(source: &Path, target: &Path, redacted: bool) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        create_dir(parent)?;
    }
    let bytes =
        fs::read(source).map_err(|e| format!("read debug source {}: {e}", source.display()))?;
    if redacted {
        match String::from_utf8(bytes.clone()) {
            Ok(text) => write_file(target, &redact_debug_text(&text)),
            Err(_) => {
                fs::copy(source, target).map_err(|e| {
                    format!(
                        "copy binary debug artifact {} to {}: {e}",
                        source.display(),
                        target.display()
                    )
                })?;
                Ok(())
            }
        }
    } else {
        fs::write(target, bytes)
            .map_err(|e| format!("write debug artifact {}: {e}", target.display()))
    }
}

fn write_trace_slice(path: &Path, records: &[TraceRecord], redacted: bool) -> Result<(), String> {
    let mut out = String::new();
    for record in records {
        if redacted {
            let mut value = record.value.clone();
            redact_json_value(&mut value);
            out.push_str(&compact_json(&value));
        } else {
            out.push_str(&compact_json(&record.value));
        }
        out.push('\n');
    }
    write_file(path, &out)
}

fn render_trace_summary(trace_path: &Path, run_id: &str, records: &[TraceRecord]) -> String {
    let mut out = String::new();
    out.push_str(&format!("trace_path={}\n", trace_path.display()));
    out.push_str(&format!("run_id={run_id}\n"));
    out.push_str(&format!("events={}\n", records.len()));
    if let Some(issue_id) = records.iter().find_map(|record| record.issue_id.as_deref()) {
        out.push_str(&format!("issue_id={issue_id}\n"));
    }
    if let Some(first) = records.first().and_then(|record| record.ts.as_deref()) {
        out.push_str(&format!("first_ts={first}\n"));
    }
    if let Some(last) = records.last().and_then(|record| record.ts.as_deref()) {
        out.push_str(&format!("last_ts={last}\n"));
    }
    let status = records
        .iter()
        .rev()
        .find(|record| record.event_type == "run.status.updated")
        .and_then(|record| record.value.pointer("/payload/status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    out.push_str(&format!("run_status={status}\n"));
    for record in records {
        out.push_str(&format!(
            "event line={} ts={} agent={} event={}\n",
            record.line_number,
            record.ts.as_deref().unwrap_or("unknown"),
            record.agent_run_id.as_deref().unwrap_or("root"),
            record.event_type
        ));
    }
    out
}

fn relative_debug_path(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map(|path| path.display().to_string())
        .map_err(|e| {
            format!(
                "debug bundle path {} is not under {}: {e}",
                path.display(),
                root.display()
            )
        })
}

fn redact_debug_text(input: &str) -> String {
    input
        .lines()
        .map(redact_debug_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_debug_line(line: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(line) {
        redact_json_value(&mut value);
        return compact_json(&value);
    }
    let lower = line.to_ascii_lowercase();
    let sensitive = [
        "authorization",
        "bearer ",
        "api_key",
        "apikey",
        "access_token",
        "refresh_token",
        "codex_api_key",
        "github_token",
        "gh_token",
        "password",
        "secret",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if !sensitive {
        return line.to_string();
    }
    if let Some((prefix, _)) = line.split_once('=') {
        return format!("{prefix}=<redacted>");
    }
    if let Some((prefix, _)) = line.split_once(':') {
        return format!("{prefix}: <redacted>");
    }
    "<redacted>".to_string()
}

fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if is_sensitive_debug_key(key) {
                    *value = serde_json::Value::String("<redacted>".to_string());
                } else {
                    redact_json_value(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_json_value(value);
            }
        }
        serde_json::Value::String(value) => {
            let lower = value.to_ascii_lowercase();
            if lower.starts_with("bearer ") || lower.starts_with("sk-") {
                *value = "<redacted>".to_string();
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn is_sensitive_debug_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    [
        "authorization",
        "api_key",
        "apikey",
        "access_token",
        "refresh_token",
        "codex_api_key",
        "github_token",
        "gh_token",
        "password",
        "secret",
        "token",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::sha256_hex_bytes;
    use crate::new_run_id;
    use std::{env, fs};

    #[test]
    fn debug_bundle_aggregates_artifacts_and_redacted_trace_without_mutation() {
        let root = env::temp_dir().join(format!(
            "agentactr-debug-bundle-test-{}-{}",
            std::process::id(),
            new_run_id("debug")
        ));
        let artifacts = root.join("artifacts").join("run-1");
        let debug_root = root.join("debug");
        let worktree = root.join("worktree");
        fs::create_dir_all(&artifacts).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            artifacts.join("context_manifest.json"),
            serde_json::json!({
                "run_id": "run-1",
                "repo": "OWNER/REPO",
                "issue": "42",
                "worktree": {
                    "path": worktree.display().to_string(),
                    "base_commit": "abc123",
                    "run_id": "run-1"
                }
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            artifacts.join("github_issue.headers"),
            "authorization: Bearer github-secret\nx-ratelimit-remaining: 42\n",
        )
        .unwrap();
        fs::write(artifacts.join("quality_report.txt"), "quality ok\n").unwrap();
        let diff_patch = "diff --git a/file.txt b/file.txt\nindex abc..def 100644\n";
        fs::write(artifacts.join("workspace.diff.patch"), diff_patch).unwrap();
        fs::write(
            artifacts.join("workspace.diff.metadata.json"),
            serde_json::json!({
                "schema_version": "0.1",
                "artifact": artifacts.join("workspace.diff.patch").display().to_string(),
                "patch_sha256": format!("sha256:{}", sha256_hex_bytes(diff_patch.as_bytes())),
                "patch_bytes": diff_patch.len(),
                "touched_file_count": 1,
                "untracked_file_count": 0,
                "includes_untracked_file_bodies": false
            })
            .to_string(),
        )
        .unwrap();
        fs::create_dir_all(artifacts.join("memory_debug")).unwrap();
        fs::write(artifacts.join("memory_debug").join("reason.txt"), "oom\n").unwrap();
        #[cfg(unix)]
        {
            let outside = root.join("outside-secret.txt");
            fs::write(&outside, "do-not-bundle\n").unwrap();
            std::os::unix::fs::symlink(&outside, artifacts.join("linked-secret.txt")).unwrap();
        }
        let prompt = "codex prompt body\n";
        fs::write(artifacts.join("codex.prompt.txt"), prompt).unwrap();
        fs::write(
            artifacts.join("codex.prompt.metadata.json"),
            serde_json::json!({
                "schema_version": "0.1",
                "prompt_artifact": artifacts.join("codex.prompt.txt").display().to_string(),
                "artifact_sha256": format!("sha256:{}", sha256_hex_bytes(prompt.as_bytes())),
                "bytes": prompt.len(),
                "chars": prompt.chars().count(),
                "redaction": "none",
                "visibility_mode": "full_body_sensitive_artifact"
            })
            .to_string(),
        )
        .unwrap();
        let child_dir = artifacts.join("spawn").join("child-1");
        fs::create_dir_all(&child_dir).unwrap();
        let child_prompt = "child prompt body\n";
        let child_handoff = "child handoff body\n";
        fs::write(child_dir.join("codex.prompt.txt"), child_prompt).unwrap();
        fs::write(child_dir.join("handoff.md"), child_handoff).unwrap();
        fs::write(
            artifacts.join("spawn_handoffs.json"),
            serde_json::json!({
                "schema_version": "0.1",
                "mode": "parallel_read_only_helpers",
                "children": [{
                    "agent_run_id": "agent-child-1",
                    "role": "RepoExplorer",
                    "handoff": child_dir.join("handoff.md").display().to_string(),
                    "handoff_sha256": format!("sha256:{}", sha256_hex_bytes(child_handoff.as_bytes())),
                    "handoff_bytes": child_handoff.len(),
                    "handoff_chars": child_handoff.chars().count(),
                    "handoff_redaction": "none",
                    "handoff_visibility_mode": "reference_only",
                    "prompt_artifact": child_dir.join("codex.prompt.txt").display().to_string(),
                    "prompt_metadata": child_dir.join("codex.prompt.metadata.json").display().to_string(),
                    "prompt_artifact_sha256": format!("sha256:{}", sha256_hex_bytes(child_prompt.as_bytes())),
                    "prompt_bytes": child_prompt.len(),
                    "prompt_chars": child_prompt.chars().count(),
                    "prompt_redaction": "none",
                    "prompt_visibility_mode": "reference_only",
                    "stdout_jsonl": child_dir.join("codex.stdout.jsonl").display().to_string(),
                    "stderr_log": child_dir.join("codex.stderr.log").display().to_string()
                }]
            })
            .to_string(),
        )
        .unwrap();
        let trace_path = root.join("events.jsonl");
        fs::write(
            &trace_path,
            [
                serde_json::json!({
                    "run_id": "run-1",
                    "issue_id": "github:OWNER/REPO#42",
                    "event_type": "run.status.updated",
                    "ts": "2026-01-01T00:00:00.000Z",
                    "ts_unix_ms": 1,
                    "payload": {"status": "started", "api_key": "sk-secret"}
                })
                .to_string(),
                serde_json::json!({
                    "run_id": "run-2",
                    "issue_id": "github:OWNER/REPO#43",
                    "event_type": "run.status.updated",
                    "ts": "2026-01-01T00:00:01.000Z",
                    "ts_unix_ms": 2,
                    "payload": {"status": "started"}
                })
                .to_string(),
            ]
            .join("\n"),
        )
        .unwrap();
        let trace_before = fs::read_to_string(&trace_path).unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();
        config.observability.debug_bundle_root = debug_root.display().to_string();
        config.observability.jsonl = trace_path.display().to_string();
        config.observability.redact_secrets = true;

        let report = create_debug_bundle(&config, "run-1").unwrap();

        let trace_after = fs::read_to_string(&trace_path).unwrap();
        assert_eq!(trace_before, trace_after);
        assert_eq!(report.trace_events, 1);
        let bundle_trace =
            fs::read_to_string(report.bundle_dir.join("traces").join("events.jsonl")).unwrap();
        assert!(bundle_trace.contains("\"run_id\":\"run-1\""));
        assert!(!bundle_trace.contains("run-2"));
        assert!(!bundle_trace.contains("sk-secret"));
        assert!(bundle_trace.contains("<redacted>"));
        let copied_headers = fs::read_to_string(
            report
                .bundle_dir
                .join("artifacts")
                .join("github_issue.headers"),
        )
        .unwrap();
        assert!(copied_headers.contains("authorization: <redacted>"));
        assert!(report
            .bundle_dir
            .join("artifacts")
            .join("memory_debug")
            .join("reason.txt")
            .exists());
        #[cfg(unix)]
        {
            let copied_link_path = report
                .bundle_dir
                .join("artifacts")
                .join("linked-secret.txt");
            assert!(!copied_link_path.exists());
            let skipped = report
                .bundle_dir
                .join("artifacts")
                .join("linked-secret.txt.symlink_skipped.json");
            let skipped = fs::read_to_string(skipped).unwrap();
            assert!(skipped.contains("\"action\": \"skipped_symlink\""));
            assert!(skipped.contains("outside-secret.txt"));
            assert!(!skipped.contains("do-not-bundle"));
        }
        let manifest = fs::read_to_string(report.bundle_dir.join("bundle_manifest.json")).unwrap();
        assert!(manifest.contains("\"redacted\": true"));
        assert!(manifest.contains("traces/events.jsonl"));
        assert!(manifest.contains("\"artifact_integrity\""));
        assert!(manifest.contains("\"verified\": true"));
        assert!(manifest.contains("artifacts/workspace.diff.patch"));
        let integrity =
            fs::read_to_string(report.bundle_dir.join("artifact_integrity.json")).unwrap();
        assert!(integrity.contains("\"status\": \"verified\""));
        assert!(integrity.contains("\"digest_field\": \"handoff_sha256\""));
        assert!(integrity.contains("\"digest_field\": \"prompt_artifact_sha256\""));
        assert!(integrity.contains("\"digest_field\": \"patch_sha256\""));
        assert!(integrity.contains("\"merge_plan\""));
        assert!(integrity.contains("\"not_recorded\""));
        assert!(report
            .bundle_dir
            .join("artifacts")
            .join("workspace.diff.patch")
            .exists());
        assert!(report
            .bundle_dir
            .join("artifacts")
            .join("workspace.diff.metadata.json")
            .exists());
        let vcs_error = fs::read_to_string(report.bundle_dir.join("vcs_status.error.txt")).unwrap();
        assert!(vcs_error.contains("outside configured vcs.worktree_root"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn debug_bundle_dir_rejects_non_segment_run_ids() {
        let root = env::temp_dir().join(format!(
            "agentactr-debug-run-id-segment-test-{}-{}",
            std::process::id(),
            new_run_id("debug")
        ));
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.debug_bundle_root = root.join("debug").display().to_string();

        assert!(debug_bundle_dir(&config, "issue-42-123").is_ok());
        for invalid in ["", "..", "../escape", "nested/run", "nested\\run", "run id"] {
            assert!(
                debug_bundle_dir(&config, invalid).is_err(),
                "debug_bundle_dir accepted invalid RUN_ID {invalid:?}"
            );
        }
        let _ = fs::remove_dir_all(root);
    }
}
