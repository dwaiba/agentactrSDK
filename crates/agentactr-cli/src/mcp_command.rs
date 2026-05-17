use crate::{configured_repo_inspection, load_agentactr_config, validate_run_id};
use agentactr_sdk::{discover_repository, AgentactrConfig};
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::linux_memory::LinuxMemoryController;

pub(crate) const MCP_PROTOCOL_LATEST: &str = "2025-11-25";
pub(crate) const MCP_PROTOCOL_SUPPORTED: &[&str] = &["2025-11-25", "2024-11-05"];

pub(crate) fn cmd_mcp(args: &mut [String]) -> Result<(), String> {
    if args.get(1).map(String::as_str) != Some("serve") {
        return Err("usage: agentactr mcp serve".to_string());
    }
    serve_mcp_stdio()
}

fn serve_mcp_stdio() -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("read MCP request: {e}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(response) = handle_mcp_json_rpc(trimmed) else {
            continue;
        };
        stdout
            .write_all(response.as_bytes())
            .map_err(|e| format!("write MCP response: {e}"))?;
        stdout
            .write_all(b"\n")
            .map_err(|e| format!("write MCP newline: {e}"))?;
        stdout.flush().map_err(|e| format!("flush MCP: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn handle_mcp_json_rpc(request: &str) -> Option<String> {
    handle_mcp_json_rpc_impl(request)
}

#[cfg(not(test))]
fn handle_mcp_json_rpc(request: &str) -> Option<String> {
    handle_mcp_json_rpc_impl(request)
}

fn handle_mcp_json_rpc_impl(request: &str) -> Option<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(request).ok()?;
    let id = parsed.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let method = parsed.get("method").and_then(serde_json::Value::as_str)?;
    if method.starts_with("notifications/") {
        return None;
    }
    let id = id.to_string();
    let result = match method {
        "initialize" => mcp_initialize_result_json(&parsed),
        "ping" => "{}".to_string(),
        "tools/list" => mcp_tools_list_json(),
        "tools/call" => {
            let tool_name = parsed
                .pointer("/params/name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let arguments = parsed
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            mcp_tool_call_result_json(tool_name, &arguments)
        }
        _ => {
            return Some(format!(
                r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32601,"message":"method not found: {}"}}}}"#,
                json_escape(method)
            ));
        }
    };
    Some(format!(
        r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#
    ))
}

fn mcp_initialize_result_json(request: &serde_json::Value) -> String {
    let requested = request
        .pointer("/params/protocolVersion")
        .and_then(serde_json::Value::as_str);
    let protocol_version = requested
        .filter(|version| MCP_PROTOCOL_SUPPORTED.contains(version))
        .unwrap_or(MCP_PROTOCOL_LATEST);
    format!(
        r#"{{"protocolVersion":"{}","capabilities":{{"tools":{{"listChanged":false}}}},"serverInfo":{{"name":"agentactr","version":"0.1.0"}}}}"#,
        json_escape(protocol_version)
    )
}

fn mcp_tools_list_json() -> String {
    let tools = [
        (
            "agentactr.issue.read",
            "Read local issue context captured by agentactr.",
        ),
        ("agentactr.run.status", "Read local agentactr run status."),
        ("agentactr.trace.read", "Read local trace metadata."),
        ("agentactr.artifact.read", "Read local artifact metadata."),
        ("agentactr.vcs.status", "Read local VCS status metadata."),
        (
            "agentactr.quality.report",
            "Read local quality report metadata.",
        ),
        (
            "agentactr.memory.status",
            "Read local memory status metadata.",
        ),
        ("agentactr.policy.read", "Read effective local policy."),
    ];
    let rendered = tools
        .iter()
        .map(|(name, description)| {
            format!(
                r#"{{"name":"{}","description":"{}","inputSchema":{}}}"#,
                json_escape(name),
                json_escape(description),
                mcp_tool_input_schema_json(name)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"tools":[{rendered}]}}"#)
}

fn mcp_tool_input_schema_json(tool_name: &str) -> &'static str {
    match tool_name {
        "agentactr.issue.read" | "agentactr.artifact.read" => {
            r#"{"type":"object","properties":{"run_id":{"type":"string","description":"Run id to scope artifact lookup when AGENTACTR_ARTIFACT_ROOT points at the artifact root."},"artifact_root":{"type":"string","description":"Artifact root or run artifact directory."}},"additionalProperties":false}"#
        }
        "agentactr.trace.read" => {
            r#"{"type":"object","properties":{"trace_path":{"type":"string","description":"Explicit trace JSONL path."}},"additionalProperties":false}"#
        }
        _ => r#"{"type":"object","properties":{},"additionalProperties":false}"#,
    }
}

fn mcp_tool_call_result_json(tool_name: &str, arguments: &serde_json::Value) -> String {
    let result = match tool_name {
        "agentactr.run.status" => {
            Ok("run_status: no active in-process run registry in this milestone".to_string())
        }
        "agentactr.memory.status" => Ok(memory_status_text()),
        "agentactr.policy.read" => Ok("policy: human_intervention=fail_closed by default; codex.approval_policy=never by default; github write MCP tools disabled".to_string()),
        "agentactr.quality.report" => Ok(mcp_quality_report_text()),
        "agentactr.vcs.status" => Ok(mcp_vcs_status_text()),
        "agentactr.artifact.read" => mcp_artifact_text(arguments),
        "agentactr.trace.read" => Ok(mcp_trace_text(arguments)),
        "agentactr.issue.read" => mcp_issue_text(arguments),
        other => Err(format!("unknown agentactr MCP tool: {other}")),
    };
    let (text, is_error) = match result {
        Ok(text) => (text, false),
        Err(err) => (err, true),
    };
    format!(
        r#"{{"content":[{{"type":"text","text":"{}"}}],"isError":{}}}"#,
        json_escape(&text),
        if is_error { "true" } else { "false" }
    )
}

fn memory_status_text() -> String {
    let config = load_agentactr_config(None)
        .map(|config| config.linux_memory)
        .unwrap_or_else(|_| AgentactrConfig::strict_defaults("OWNER/REPO").linux_memory);
    LinuxMemoryController::new(&config).memory_status_text()
}

fn mcp_quality_report_text() -> String {
    let inspection = load_agentactr_config(None)
        .map(|config| configured_repo_inspection(Path::new("."), &config))
        .unwrap_or_else(|_| discover_repository(Path::new(".")));
    let commands = inspection
        .quality_plan
        .iter()
        .map(|cmd| cmd.command.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "detected_stack={} selected_stack={} confidence={} missing_prerequisites={} quality_plan={}",
        inspection.detected_stack.as_str(),
        inspection.primary_stack.as_str(),
        inspection.confidence,
        inspection.missing_prerequisites.join("; "),
        commands
    )
}

fn mcp_vcs_status_text() -> String {
    let output = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let status = String::from_utf8_lossy(&output.stdout);
            if status.trim().is_empty() {
                "git=true dirty=false".to_string()
            } else {
                format!("git=true dirty=true entries={}", status.lines().count())
            }
        }
        Ok(output) => format!(
            "git=false error={}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(err) => format!("git=false error={err}"),
    }
}

fn mcp_artifact_text(arguments: &serde_json::Value) -> Result<String, String> {
    let root = agentactr_artifact_root(arguments)?;
    Ok(list_dir_names(&root, "artifacts"))
}

#[cfg(test)]
pub(crate) fn mcp_trace_text(arguments: &serde_json::Value) -> String {
    mcp_trace_text_impl(arguments)
}

#[cfg(not(test))]
fn mcp_trace_text(arguments: &serde_json::Value) -> String {
    mcp_trace_text_impl(arguments)
}

fn mcp_trace_text_impl(arguments: &serde_json::Value) -> String {
    let path = agentactr_trace_path(arguments);
    if path.exists() {
        match fs::metadata(&path) {
            Ok(meta) => format!("trace_events_path={} bytes={}", path.display(), meta.len()),
            Err(err) => format!("trace_events_path={} metadata_error={err}", path.display()),
        }
    } else {
        format!("trace_events_path={} present=false", path.display())
    }
}

fn mcp_issue_text(arguments: &serde_json::Value) -> Result<String, String> {
    let artifacts = agentactr_artifact_root(arguments)?;
    let direct_issue = artifacts.join("github_issue.json");
    if direct_issue.exists() {
        return fs::read_to_string(&direct_issue)
            .map_err(|e| format!("read {}: {e}", direct_issue.display()));
    }
    Err(format!(
        "no run-scoped github_issue.json artifact found at {}; pass run_id or set AGENTACTR_ARTIFACT_ROOT to the run artifact directory",
        direct_issue.display()
    ))
}

fn agentactr_artifact_root(arguments: &serde_json::Value) -> Result<PathBuf, String> {
    let root = arguments
        .get("artifact_root")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .or_else(|| env::var("AGENTACTR_ARTIFACT_ROOT").ok().map(PathBuf::from))
        .unwrap_or_else(|| Path::new(".agentactr").join("artifacts"));
    if let Some(run_id) = arguments.get("run_id").and_then(serde_json::Value::as_str) {
        validate_run_id(run_id)?;
        if root.file_name().and_then(|name| name.to_str()) == Some(run_id) {
            Ok(root)
        } else {
            Ok(root.join(run_id))
        }
    } else {
        Ok(root)
    }
}

fn agentactr_trace_path(arguments: &serde_json::Value) -> PathBuf {
    arguments
        .get("trace_path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .or_else(|| env::var("AGENTACTR_TRACE_PATH").ok().map(PathBuf::from))
        .or_else(|| {
            env::var("AGENTACTR_REPO_ROOT")
                .ok()
                .map(|root| Path::new(&root).join(".agentactr/runs/events.jsonl"))
        })
        .unwrap_or_else(|| Path::new(".agentactr/runs/events.jsonl").to_path_buf())
}

fn list_dir_names(root: &Path, label: &str) -> String {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return format!("{label}_path={} present=false", root.display()),
    };
    let names = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    format!(
        "{label}_path={} entries={}",
        root.display(),
        names.join(",")
    )
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn mcp_initialize_negotiates_latest_and_legacy_protocols() {
        let latest = handle_mcp_json_rpc(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
        )
        .unwrap();
        assert!(latest.contains(r#""protocolVersion":"2025-11-25""#));

        let legacy = handle_mcp_json_rpc(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        )
        .unwrap();
        assert!(legacy.contains(r#""protocolVersion":"2024-11-05""#));
    }

    #[test]
    fn mcp_issue_read_uses_explicit_artifact_root() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = env::temp_dir().join(format!("agentactr-mcp-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("github_issue.json"),
            r#"{"title":"from-env-root"}"#,
        )
        .unwrap();
        env::set_var("AGENTACTR_ARTIFACT_ROOT", &root);

        let response = handle_mcp_json_rpc(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"agentactr.issue.read","arguments":{}}}"#,
        )
        .unwrap();

        env::remove_var("AGENTACTR_ARTIFACT_ROOT");
        assert!(response.contains("from-env-root"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mcp_issue_read_is_run_scoped_when_run_id_is_supplied() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = env::temp_dir().join(format!(
            "agentactr-mcp-run-scoped-test-{}",
            std::process::id()
        ));
        let old_run = root.join("run-old");
        let new_run = root.join("run-new");
        fs::create_dir_all(&old_run).unwrap();
        fs::create_dir_all(&new_run).unwrap();
        fs::write(old_run.join("github_issue.json"), r#"{"title":"old"}"#).unwrap();
        fs::write(new_run.join("github_issue.json"), r#"{"title":"new"}"#).unwrap();
        env::set_var("AGENTACTR_ARTIFACT_ROOT", &root);

        let scoped = handle_mcp_json_rpc(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"agentactr.issue.read","arguments":{"run_id":"run-old"}}}"#,
        )
        .unwrap();
        let unscoped = handle_mcp_json_rpc(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"agentactr.issue.read","arguments":{}}}"#,
        )
        .unwrap();

        env::remove_var("AGENTACTR_ARTIFACT_ROOT");
        assert!(scoped.contains(r#"\"title\":\"old\""#));
        assert!(!scoped.contains(r#"\"title\":\"new\""#));
        assert!(unscoped.contains("isError\":true"));
        assert!(unscoped.contains("run-scoped"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mcp_artifact_read_rejects_non_segment_run_id() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = env::temp_dir().join(format!(
            "agentactr-mcp-run-scope-reject-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        env::set_var("AGENTACTR_ARTIFACT_ROOT", &root);

        let response = handle_mcp_json_rpc(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"agentactr.artifact.read","arguments":{"run_id":"../../escape"}}}"#,
        )
        .unwrap();

        env::remove_var("AGENTACTR_ARTIFACT_ROOT");
        assert!(response.contains("isError\":true"));
        assert!(response.contains("path separators are not allowed"));
        assert!(!response.contains("entries="));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mcp_trace_read_uses_explicit_trace_path() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = env::temp_dir().join(format!("agentactr-mcp-trace-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let trace_path = root.join("events.jsonl");
        fs::write(&trace_path, "{}\n").unwrap();
        env::set_var("AGENTACTR_TRACE_PATH", &trace_path);

        let text = mcp_trace_text(&serde_json::json!({}));

        env::remove_var("AGENTACTR_TRACE_PATH");
        assert!(text.contains("bytes="));
        assert!(text.contains(&trace_path.display().to_string()));
        let _ = fs::remove_dir_all(root);
    }
}
