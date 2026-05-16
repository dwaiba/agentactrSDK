use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) struct ArtifactIntegrityContext<'a> {
    pub(crate) run_id: &'a str,
    pub(crate) artifact_dir: &'a Path,
}

pub(crate) fn collect_artifact_integrity(
    context: &ArtifactIntegrityContext<'_>,
) -> Result<serde_json::Value, String> {
    let writer_prompt = verify_writer_prompt_artifact(context)?;
    let child_handoffs = verify_spawn_handoff_artifacts(context)?;
    let workspace_diff = verify_workspace_diff_artifact(context)?;
    let merge_plan = verify_merge_plan_artifact(context)?;
    let verified = writer_prompt
        .get("verified")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        && child_handoffs.iter().all(|child| {
            child
                .get("verified")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        && workspace_diff
            .get("verified")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        && merge_plan
            .get("verified")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
    Ok(serde_json::json!({
        "schema_version": "0.1",
        "run_id": context.run_id,
        "status": if verified { "verified" } else { "incomplete_or_mismatch" },
        "verified": verified,
        "writer_prompt": writer_prompt,
        "child_handoffs": child_handoffs,
        "workspace_diff": workspace_diff,
        "merge_plan": merge_plan,
    }))
}

fn verify_writer_prompt_artifact(
    context: &ArtifactIntegrityContext<'_>,
) -> Result<serde_json::Value, String> {
    let metadata_path = context.artifact_dir.join("codex.prompt.metadata.json");
    if !metadata_path.exists() {
        return Ok(serde_json::json!({
            "status": "missing_metadata",
            "verified": false,
            "metadata": metadata_path.display().to_string(),
        }));
    }
    let metadata = read_json_file(&metadata_path)?;
    let artifact = metadata
        .get("prompt_artifact")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| context.artifact_dir.join("codex.prompt.txt"));
    Ok(verify_artifact_digest(
        context.artifact_dir,
        &artifact,
        metadata
            .get("artifact_sha256")
            .and_then(serde_json::Value::as_str),
        serde_json::json!({
            "digest_field": "artifact_sha256",
            "metadata": metadata_path.display().to_string(),
            "bytes": metadata.get("bytes").cloned().unwrap_or(serde_json::Value::Null),
            "chars": metadata.get("chars").cloned().unwrap_or(serde_json::Value::Null),
            "redaction": metadata.get("redaction").cloned().unwrap_or(serde_json::Value::Null),
            "visibility_mode": metadata.get("visibility_mode").cloned().unwrap_or(serde_json::Value::Null),
        }),
    ))
}

fn verify_spawn_handoff_artifacts(
    context: &ArtifactIntegrityContext<'_>,
) -> Result<Vec<serde_json::Value>, String> {
    let manifest_path = context.artifact_dir.join("spawn_handoffs.json");
    if !manifest_path.exists() {
        return Ok(Vec::new());
    }
    let manifest = read_json_file(&manifest_path)?;
    let Some(children) = manifest
        .get("children")
        .and_then(serde_json::Value::as_array)
    else {
        return Err(format!(
            "spawn handoff manifest {} is missing array `children`",
            manifest_path.display()
        ));
    };
    let mut out = Vec::new();
    for child in children {
        let handoff = child
            .get("handoff")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| context.artifact_dir.join("handoff.md"));
        let prompt = child
            .get("prompt_artifact")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| context.artifact_dir.join("codex.prompt.txt"));
        let handoff_integrity = verify_artifact_digest(
            context.artifact_dir,
            &handoff,
            child
                .get("handoff_sha256")
                .and_then(serde_json::Value::as_str),
            serde_json::json!({
                "digest_field": "handoff_sha256",
                "bytes": child.get("handoff_bytes").cloned().unwrap_or(serde_json::Value::Null),
                "chars": child.get("handoff_chars").cloned().unwrap_or(serde_json::Value::Null),
                "redaction": child.get("handoff_redaction").cloned().unwrap_or(serde_json::Value::Null),
                "visibility_mode": child.get("handoff_visibility_mode").cloned().unwrap_or(serde_json::Value::Null),
            }),
        );
        let prompt_integrity = verify_artifact_digest(
            context.artifact_dir,
            &prompt,
            child
                .get("prompt_artifact_sha256")
                .and_then(serde_json::Value::as_str),
            serde_json::json!({
                "digest_field": "prompt_artifact_sha256",
                "metadata": child.get("prompt_metadata").cloned().unwrap_or(serde_json::Value::Null),
                "bytes": child.get("prompt_bytes").cloned().unwrap_or(serde_json::Value::Null),
                "chars": child.get("prompt_chars").cloned().unwrap_or(serde_json::Value::Null),
                "redaction": child.get("prompt_redaction").cloned().unwrap_or(serde_json::Value::Null),
                "visibility_mode": child.get("prompt_visibility_mode").cloned().unwrap_or(serde_json::Value::Null),
            }),
        );
        let verified = handoff_integrity
            .get("verified")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            && prompt_integrity
                .get("verified")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
        out.push(serde_json::json!({
            "agent_run_id": child.get("agent_run_id").cloned().unwrap_or(serde_json::Value::Null),
            "role": child.get("role").cloned().unwrap_or(serde_json::Value::Null),
            "verified": verified,
            "handoff": handoff_integrity,
            "prompt": prompt_integrity,
        }));
    }
    Ok(out)
}

fn verify_workspace_diff_artifact(
    context: &ArtifactIntegrityContext<'_>,
) -> Result<serde_json::Value, String> {
    let metadata_path = context.artifact_dir.join("workspace.diff.metadata.json");
    if !metadata_path.exists() {
        return Ok(serde_json::json!({
            "status": "not_recorded",
            "required": false,
            "verified": true,
            "metadata": metadata_path.display().to_string(),
        }));
    }
    let metadata = read_json_file(&metadata_path)?;
    let artifact = metadata
        .get("artifact")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| context.artifact_dir.join("workspace.diff.patch"));
    Ok(verify_artifact_digest(
        context.artifact_dir,
        &artifact,
        metadata
            .get("patch_sha256")
            .and_then(serde_json::Value::as_str),
        serde_json::json!({
            "digest_field": "patch_sha256",
            "metadata": metadata_path.display().to_string(),
            "required": false,
            "patch_bytes": metadata.get("patch_bytes").cloned().unwrap_or(serde_json::Value::Null),
            "touched_file_count": metadata.get("touched_file_count").cloned().unwrap_or(serde_json::Value::Null),
            "untracked_file_count": metadata.get("untracked_file_count").cloned().unwrap_or(serde_json::Value::Null),
            "includes_untracked_file_bodies": metadata.get("includes_untracked_file_bodies").cloned().unwrap_or(serde_json::Value::Null),
        }),
    ))
}

fn verify_merge_plan_artifact(
    context: &ArtifactIntegrityContext<'_>,
) -> Result<serde_json::Value, String> {
    let metadata_path = context.artifact_dir.join("merge_plan.metadata.json");
    if !metadata_path.exists() {
        return Ok(serde_json::json!({
            "status": "not_recorded",
            "required": false,
            "verified": true,
            "metadata": metadata_path.display().to_string(),
        }));
    }
    let metadata = read_json_file(&metadata_path)?;
    let artifact = metadata
        .get("artifact")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| context.artifact_dir.join("merge_plan.json"));
    Ok(verify_artifact_digest(
        context.artifact_dir,
        &artifact,
        metadata
            .get("artifact_sha256")
            .and_then(serde_json::Value::as_str),
        serde_json::json!({
            "digest_field": "artifact_sha256",
            "metadata": metadata_path.display().to_string(),
            "required": false,
            "artifact_bytes": metadata.get("artifact_bytes").cloned().unwrap_or(serde_json::Value::Null),
            "artifact_chars": metadata.get("artifact_chars").cloned().unwrap_or(serde_json::Value::Null),
            "recommendation": metadata.get("recommendation").cloned().unwrap_or(serde_json::Value::Null),
            "blocker_count": metadata.get("blocker_count").cloned().unwrap_or(serde_json::Value::Null),
            "warning_count": metadata.get("warning_count").cloned().unwrap_or(serde_json::Value::Null),
            "merge_mode": metadata.get("merge_mode").cloned().unwrap_or(serde_json::Value::Null),
            "merge_enabled": metadata.get("merge_enabled").cloned().unwrap_or(serde_json::Value::Null),
            "workspace_diff_exists": metadata.get("workspace_diff_exists").cloned().unwrap_or(serde_json::Value::Null),
        }),
    ))
}

pub(crate) fn verify_artifact_digest(
    artifact_root: &Path,
    artifact_path: &Path,
    expected_sha256: Option<&str>,
    mut extra: serde_json::Value,
) -> serde_json::Value {
    let Some(path) = safe_artifact_path(artifact_root, artifact_path) else {
        return merge_artifact_integrity_extra(
            serde_json::json!({
                "path": artifact_path.display().to_string(),
                "status": "path_outside_artifact_root",
                "verified": false,
                "expected_sha256": expected_sha256,
            }),
            extra,
        );
    };
    let expected = expected_sha256.unwrap_or("");
    let base = match fs::read(&path) {
        Ok(bytes) => {
            let actual = format!("sha256:{}", sha256_hex_bytes(&bytes));
            serde_json::json!({
                "path": path.display().to_string(),
                "status": if !expected.is_empty() && expected == actual { "verified" } else { "mismatch_or_missing_expected_digest" },
                "verified": !expected.is_empty() && expected == actual,
                "expected_sha256": expected_sha256,
                "actual_sha256": actual,
                "actual_bytes": bytes.len(),
            })
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => serde_json::json!({
            "path": path.display().to_string(),
            "status": "missing_artifact",
            "verified": false,
            "expected_sha256": expected_sha256,
        }),
        Err(err) => serde_json::json!({
            "path": path.display().to_string(),
            "status": "read_failed",
            "verified": false,
            "expected_sha256": expected_sha256,
            "error": err.to_string(),
        }),
    };
    merge_artifact_integrity_extra(base, extra.take())
}

fn merge_artifact_integrity_extra(
    mut base: serde_json::Value,
    extra: serde_json::Value,
) -> serde_json::Value {
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    base
}

fn safe_artifact_path(artifact_root: &Path, path: &Path) -> Option<PathBuf> {
    let root = super::normalize_path_lexically(artifact_root)?;
    let candidate = if path.is_absolute() {
        super::normalize_path_lexically(path)?
    } else {
        super::normalize_path_lexically(&artifact_root.join(path))?
    };
    if !candidate.starts_with(&root) {
        return None;
    }
    if candidate.exists() {
        let canonical_root = artifact_root.canonicalize().ok()?;
        let canonical_candidate = candidate.canonicalize().ok()?;
        canonical_candidate
            .starts_with(canonical_root)
            .then_some(canonical_candidate)
    } else {
        Some(candidate)
    }
}

fn read_json_file(path: &Path) -> Result<serde_json::Value, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("parse {}: {e}", path.display()))
}

pub(crate) fn sha256_hex_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}
