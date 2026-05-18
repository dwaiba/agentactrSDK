use crate::terminal::{self, ColorMode};
use crate::trace_command::{
    latest_run_status, read_trace_records, summarize_trace_runs, TraceRecord,
};
use crate::{load_agentactr_config, load_run_artifact_context, run_trace_path, validate_run_id};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

pub(crate) fn cmd_tui(args: &mut [String]) -> Result<(), String> {
    let no_color = args.iter().any(|arg| arg == "--no-color");
    let color_override = no_color.then_some(ColorMode::Never);
    match args.get(1).map(String::as_str) {
        Some("run") => {
            let run_id = args.get(2).ok_or(
                "usage: agentactr tui run RUN_ID [--refresh 1s] [--snapshot] [--no-color]",
            )?;
            validate_run_id(run_id)?;
            render_tui_command(run_id, args, color_override)
        }
        Some("latest") => {
            let config = load_agentactr_config(None)?;
            let trace_path = run_trace_path(&config)?;
            let records = read_trace_records(&trace_path)?;
            let run_id = latest_tui_run_id(&records, &trace_path)?;
            render_tui_command(&run_id, args, color_override)
        }
        _ => Err(
            "usage: agentactr tui run RUN_ID [--refresh 1s] [--snapshot] | agentactr tui latest [--refresh 1s]"
                .to_string(),
        ),
    }
}

fn latest_tui_run_id(records: &[TraceRecord], trace_path: &Path) -> Result<String, String> {
    let Some(summary) = summarize_trace_runs(records).into_iter().next() else {
        return Err(format!(
            "no runs found in {}; pass RUN_ID with `agentactr tui run RUN_ID`",
            trace_path.display()
        ));
    };
    Ok(summary.run_id)
}

fn render_tui_command(
    run_id: &str,
    args: &[String],
    color_override: Option<ColorMode>,
) -> Result<(), String> {
    let snapshot = args.iter().any(|arg| arg == "--snapshot");
    let color = terminal::color_enabled(false, color_override);
    let refresh = refresh_interval(args)?;
    if snapshot || refresh.is_none() {
        print!("{}", render_snapshot(run_id, color)?);
        return Ok(());
    }
    let interval = refresh.expect("checked is_some above");
    loop {
        print!("\x1b[2J\x1b[H{}", render_snapshot(run_id, color)?);
        thread::sleep(interval);
    }
}

fn refresh_interval(args: &[String]) -> Result<Option<Duration>, String> {
    let Some(index) = args.iter().position(|arg| arg == "--refresh") else {
        return Ok(None);
    };
    let Some(value) = args.get(index + 1) else {
        return Err("--refresh requires a duration such as 1s or 500ms".to_string());
    };
    if let Some(ms) = value.strip_suffix("ms") {
        let ms = ms
            .parse::<u64>()
            .map_err(|_| format!("invalid --refresh duration `{value}`"))?;
        return Ok(Some(Duration::from_millis(ms.max(100))));
    }
    if let Some(seconds) = value.strip_suffix('s') {
        let seconds = seconds
            .parse::<u64>()
            .map_err(|_| format!("invalid --refresh duration `{value}`"))?;
        return Ok(Some(Duration::from_secs(seconds.max(1))));
    }
    Err(format!(
        "invalid --refresh duration `{value}`; use a value such as 1s or 500ms"
    ))
}

fn render_snapshot(run_id: &str, color: bool) -> Result<String, String> {
    let config = load_agentactr_config(None)?;
    let context = load_run_artifact_context(&config, run_id)?;
    let trace_path = run_trace_path(&config)?;
    let records = read_trace_records(&trace_path)?;
    let run_records = records
        .iter()
        .filter(|record| record.run_id == run_id)
        .collect::<Vec<_>>();
    let status = latest_run_status(&records, run_id);
    let agent_graph_path = context.artifact_dir.join("agent_graph.json");
    let spawn_handoffs_path = context.artifact_dir.join("spawn_handoffs.json");
    let quality_report_path = context.artifact_dir.join("quality_report.txt");
    let quality_status_path = agentactr_sdk::quality_status_path(&quality_report_path);
    let legacy_quality_status_path = context.artifact_dir.join("quality_status.json");
    let runtime_events_path = context.artifact_dir.join("runtime_process_events.jsonl");
    let lifecycle_events_path = context.artifact_dir.join("github_lifecycle_events.jsonl");
    let finalization_path = context.artifact_dir.join("finalization_status.json");
    let graph = read_json_optional(&agent_graph_path)?;
    let spawn = read_json_optional(&spawn_handoffs_path)?;
    let quality = read_quality_status_optional(&quality_status_path, &legacy_quality_status_path)?;
    let quality_status_display_path = quality
        .as_ref()
        .map(|(path, _)| path.as_path())
        .unwrap_or(quality_status_path.as_path());
    let quality_report = read_text_optional(&quality_report_path)?;
    let runtime_events = read_jsonl_optional(&runtime_events_path)?;
    let lifecycle_events = read_jsonl_optional(&lifecycle_events_path)?;
    let finalization = read_json_optional(&finalization_path)?;
    let quality_summary = summarize_quality(
        quality.as_ref().map(|(_, value)| value),
        quality_report.as_deref(),
    );
    let lifecycle_summary = summarize_lifecycle(&lifecycle_events, finalization.as_ref());

    let mut out = String::new();
    out.push_str(&format!(
        "{} {}\n",
        terminal::cyan("Agentactr Run TUI", color),
        terminal::dim("(read-only snapshot)", color)
    ));
    out.push_str(&format!("run_id={run_id}\n"));
    out.push_str(&format!("status={}\n", color_state(&status, color)));
    out.push_str(&format!(
        "artifact_dir={}\n",
        context.artifact_dir.display()
    ));
    out.push('\n');
    out.push_str("Agent Graph\n");
    if let Some(graph) = graph.as_ref() {
        let nodes = graph
            .get("nodes")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        let edges = graph
            .get("edges")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        out.push_str(&format!("- nodes={nodes} edges={edges}\n"));
        for node in graph_nodes(graph, &run_records, &runtime_events)
            .into_iter()
            .take(12)
        {
            out.push_str(&format!("- {}\n", node));
        }
    } else {
        out.push_str(&format!(
            "- {}\n",
            terminal::yellow(
                "agent_graph.json unavailable; graph pane is diagnostic-only",
                color
            )
        ));
    }
    out.push('\n');
    out.push_str("Selected Agent Details\n");
    out.push_str(&format!(
        "- spawn_handoffs={}\n",
        artifact_state(&spawn_handoffs_path, spawn.is_some(), color)
    ));
    if let Some(spawn_summary) = summarize_spawn(spawn.as_ref()) {
        out.push_str(&format!("- spawn={spawn_summary}\n"));
    }
    out.push_str(&format!("- trace_events={}\n", run_records.len()));
    out.push_str(&format!(
        "- runtime_process_events={}\n",
        artifact_count_state(&runtime_events_path, runtime_events.len(), color)
    ));
    out.push('\n');
    out.push_str("Quality / Lifecycle\n");
    out.push_str(&format!(
        "- quality={}\n",
        artifact_state(quality_status_display_path, quality.is_some(), color)
    ));
    out.push_str(&format!(
        "- quality_status={}\n",
        color_state(quality_summary.status.as_str(), color)
    ));
    for gate in quality_summary.gates.iter().take(8) {
        out.push_str(&format!(
            "- gate={} status={} required={} command={}\n",
            gate.name,
            color_state(gate.status.as_str(), color),
            gate.required.as_deref().unwrap_or("unknown"),
            gate.command.as_deref().unwrap_or("not-recorded")
        ));
    }
    if let Some(reason) = quality_summary.failed_reason.as_deref() {
        out.push_str(&format!("- quality_failure={reason}\n"));
    }
    out.push_str(&format!(
        "- finalization={}\n",
        artifact_state(&finalization_path, finalization.is_some(), color)
    ));
    out.push_str(&format!(
        "- lifecycle_events={}\n",
        artifact_count_state(&lifecycle_events_path, lifecycle_events.len(), color)
    ));
    out.push_str(&format!(
        "- lifecycle_status={}\n",
        color_state(lifecycle_summary.status.as_str(), color)
    ));
    for event in lifecycle_summary.events.iter().rev().take(5).rev() {
        out.push_str(&format!(
            "- lifecycle_event={} ts={} detail={}\n",
            event.event_type,
            event.ts.as_deref().unwrap_or("unknown"),
            event.detail.as_deref().unwrap_or("none")
        ));
    }
    out.push_str(&format!(
        "- context_manifest={}\n",
        artifact_state(
            &context.manifest_path,
            context.manifest_path.exists(),
            color
        )
    ));
    Ok(out)
}

fn read_text_optional(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|e| format!("read {}: {e}", path.display()))
}

fn read_json_optional(path: &Path) -> Result<Option<serde_json::Value>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("parse {}: {e}", path.display()))
}

fn read_quality_status_optional(
    preferred_path: &Path,
    legacy_path: &Path,
) -> Result<Option<(std::path::PathBuf, serde_json::Value)>, String> {
    if let Some(value) = read_json_optional(preferred_path)? {
        return Ok(Some((preferred_path.to_path_buf(), value)));
    }
    if let Some(value) = read_json_optional(legacy_path)? {
        return Ok(Some((legacy_path.to_path_buf(), value)));
    }
    Ok(None)
}

fn read_jsonl_optional(path: &Path) -> Result<Vec<serde_json::Value>, String> {
    let Some(text) = read_text_optional(path)? else {
        return Ok(Vec::new());
    };
    let mut events = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        events.push(
            serde_json::from_str::<serde_json::Value>(line)
                .map_err(|e| format!("parse {} line {}: {e}", path.display(), index + 1))?,
        );
    }
    Ok(events)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AgentState {
    state: String,
    last_event: Option<String>,
    last_ts: Option<String>,
    activity: Option<String>,
    failed: bool,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            state: "pending".to_string(),
            last_event: None,
            last_ts: None,
            activity: None,
            failed: false,
        }
    }
}

fn graph_nodes(
    graph: &serde_json::Value,
    trace_records: &[&TraceRecord],
    runtime_events: &[serde_json::Value],
) -> Vec<String> {
    let states = derive_agent_states(trace_records, runtime_events);
    graph
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|node| {
            let id = node
                .get("id")
                .or_else(|| node.get("node_id"))
                .or_else(|| node.get("agent_run_id"))
                .and_then(serde_json::Value::as_str)?;
            let kind = node
                .get("kind")
                .or_else(|| node.get("type"))
                .or_else(|| node.get("runtime"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("node");
            let role = node
                .get("role")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(kind);
            let write_scope = node
                .get("write_scope")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let state = states.get(id).cloned().unwrap_or_default();
            Some(format!(
                "{id} role={role} state={} scope={write_scope} last_event={} activity={}",
                state.state,
                state.last_event.as_deref().unwrap_or("none"),
                state
                    .activity
                    .as_deref()
                    .unwrap_or("waiting_for_runtime_event")
            ))
        })
        .collect()
}

fn derive_agent_states(
    trace_records: &[&TraceRecord],
    runtime_events: &[serde_json::Value],
) -> BTreeMap<String, AgentState> {
    let mut states = BTreeMap::<String, AgentState>::new();
    for record in trace_records {
        let Some(agent) = record.agent_run_id.as_deref() else {
            continue;
        };
        let state = states.entry(agent.to_string()).or_default();
        apply_agent_event(
            state,
            &record.event_type,
            record.ts.as_deref(),
            record.value.get("payload"),
        );
    }
    for event in runtime_events {
        let Some(agent) = event
            .get("agent_run_id")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let kind = event
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .map(|kind| format!("runtime.process.{kind}"))
            .unwrap_or_else(|| "runtime.process.observed".to_string());
        let state = states.entry(agent.to_string()).or_default();
        apply_agent_event(
            state,
            &kind,
            event.get("ts").and_then(serde_json::Value::as_str),
            Some(event),
        );
    }
    states
}

fn apply_agent_event(
    state: &mut AgentState,
    event_type: &str,
    ts: Option<&str>,
    payload: Option<&serde_json::Value>,
) {
    state.last_event = Some(event_type.to_string());
    state.last_ts = ts
        .map(ToString::to_string)
        .or_else(|| state.last_ts.clone());
    if event_type == "error" || event_type.ends_with(".failed") {
        state.failed = true;
        state.state = "failed".to_string();
    } else if event_type.contains("blocked") {
        state.state = "blocked".to_string();
    } else if state.failed {
        state.state = "failed".to_string();
    } else if event_type == "runtime.process.started"
        || event_type == "runtime.process.attributed"
        || event_type == "runtime.process.child_discovered"
        || event_type == "agent.started"
    {
        state.state = "active".to_string();
    } else if event_type == "runtime.process.terminated"
        || event_type == "agent.completed"
        || event_type == "agent.finished"
    {
        state.state = "complete".to_string();
    } else if event_type.contains("review_required") || event_type.contains("review") {
        state.state = "review".to_string();
    }
    state.activity = runtime_activity(payload).or_else(|| {
        Some(
            event_type
                .trim_start_matches("runtime.process.")
                .to_string(),
        )
    });
}

fn runtime_activity(payload: Option<&serde_json::Value>) -> Option<String> {
    let payload = payload?;
    let root_pid = payload.get("root_pid").and_then(serde_json::Value::as_i64);
    let process_group_id = payload
        .get("process_group_id")
        .and_then(serde_json::Value::as_i64);
    let memory_group_id = payload
        .get("memory_group_id")
        .and_then(serde_json::Value::as_str);
    let mut parts = Vec::new();
    if let Some(pid) = root_pid {
        parts.push(format!("pid={pid}"));
    }
    if let Some(pgid) = process_group_id {
        parts.push(format!("pgid={pgid}"));
    }
    if let Some(memory) = memory_group_id {
        parts.push(format!("memory={memory}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

#[derive(Debug, Default)]
struct QualitySummary {
    status: String,
    failed_reason: Option<String>,
    gates: Vec<QualityGateDisplay>,
}

#[derive(Debug, Default)]
struct QualityGateDisplay {
    name: String,
    status: String,
    required: Option<String>,
    command: Option<String>,
}

fn summarize_quality(status: Option<&serde_json::Value>, report: Option<&str>) -> QualitySummary {
    let success = status
        .and_then(|value| value.get("success"))
        .and_then(serde_json::Value::as_bool);
    let failed_reason = status
        .and_then(|value| value.get("failed_reason"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    let gates = report.map(parse_quality_gates).unwrap_or_default();
    QualitySummary {
        status: match success {
            Some(true) => "success".to_string(),
            Some(false) => "failed".to_string(),
            None if !gates.is_empty() => "report_only".to_string(),
            None => "unknown".to_string(),
        },
        failed_reason,
        gates,
    }
}

fn parse_quality_gates(report: &str) -> Vec<QualityGateDisplay> {
    let mut gates = Vec::new();
    let mut current: Option<QualityGateDisplay> = None;
    for line in report.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            if let Some(gate) = current.take() {
                gates.push(gate);
            }
            current = Some(QualityGateDisplay {
                name: name.trim().to_string(),
                status: "unknown".to_string(),
                required: None,
                command: None,
            });
            continue;
        }
        let Some(gate) = current.as_mut() else {
            continue;
        };
        if let Some(status) = line.strip_prefix("status=") {
            gate.status = status.trim().to_string();
        } else if let Some(required) = line.strip_prefix("required=") {
            gate.required = Some(required.trim().to_string());
        } else if let Some(command) = line.strip_prefix("command=") {
            gate.command = Some(command.trim().to_string());
        }
    }
    if let Some(gate) = current {
        gates.push(gate);
    }
    gates
}

#[derive(Debug, Default)]
struct LifecycleSummary {
    status: String,
    events: Vec<LifecycleEventDisplay>,
}

#[derive(Debug, Default)]
struct LifecycleEventDisplay {
    event_type: String,
    ts: Option<String>,
    detail: Option<String>,
}

fn summarize_lifecycle(
    events: &[serde_json::Value],
    finalization: Option<&serde_json::Value>,
) -> LifecycleSummary {
    let mut rendered = Vec::new();
    for event in events {
        let event_type = event
            .get("event_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let ts = event
            .get("ts")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        let payload = event.get("payload").unwrap_or(&serde_json::Value::Null);
        let detail = lifecycle_detail(payload);
        rendered.push(LifecycleEventDisplay {
            event_type,
            ts,
            detail,
        });
    }
    let status = finalization
        .and_then(|value| value.get("status"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            events
                .iter()
                .rev()
                .find_map(|event| {
                    event
                        .pointer("/payload/status")
                        .and_then(serde_json::Value::as_str)
                })
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string());
    LifecycleSummary {
        status,
        events: rendered,
    }
}

fn lifecycle_detail(payload: &serde_json::Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(status) = payload.get("status").and_then(serde_json::Value::as_str) {
        parts.push(format!("status={status}"));
    }
    if let Some(accepted) = payload.get("accepted").and_then(serde_json::Value::as_bool) {
        parts.push(format!("accepted={accepted}"));
    }
    if let Some(verification) = payload
        .get("verification_status")
        .or_else(|| payload.pointer("/release/verification_status"))
        .and_then(serde_json::Value::as_str)
    {
        parts.push(format!("verification={verification}"));
    }
    if let Some(decision) = payload.get("decision").and_then(serde_json::Value::as_str) {
        parts.push(format!("decision={decision}"));
    }
    if let Some(detail) = payload.get("detail").and_then(serde_json::Value::as_str) {
        parts.push(format!("detail={detail}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn summarize_spawn(spawn: Option<&serde_json::Value>) -> Option<String> {
    let spawn = spawn?;
    let children = spawn
        .get("children")
        .or_else(|| spawn.get("handoffs"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let status = spawn
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("recorded");
    Some(format!("status={status} children={children}"))
}

fn artifact_state(path: &Path, exists: bool, color: bool) -> String {
    if exists {
        format!("{} {}", terminal::green("available", color), path.display())
    } else {
        format!("{} {}", terminal::dim("missing", color), path.display())
    }
}

fn artifact_count_state(path: &Path, count: usize, color: bool) -> String {
    if count > 0 {
        format!(
            "{} count={} {}",
            terminal::green("available", color),
            count,
            path.display()
        )
    } else if path.exists() {
        format!(
            "{} count=0 {}",
            terminal::yellow("empty", color),
            path.display()
        )
    } else {
        format!(
            "{} count=0 {}",
            terminal::dim("missing", color),
            path.display()
        )
    }
}

fn color_state(status: &str, color: bool) -> String {
    match status {
        "completed" | "complete" | "finalized" | "success" => terminal::green(status, color),
        "failed" | "blocked" => terminal::red(status, color),
        "running" | "active" | "report_only" => terminal::cyan(status, color),
        "review_required" | "review" => terminal::magenta(status, color),
        "unknown" | "pending" | "skipped" | "disabled" => terminal::dim(status, color),
        _ => terminal::yellow(status, color),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn trace_record(agent: &str, event_type: &str, payload: serde_json::Value) -> TraceRecord {
        run_trace_record("run-1", agent, event_type, 1, payload)
    }

    fn run_trace_record(
        run_id: &str,
        agent: &str,
        event_type: &str,
        ts_unix_ms: u128,
        payload: serde_json::Value,
    ) -> TraceRecord {
        TraceRecord {
            line_number: 1,
            value: serde_json::json!({
                "run_id": run_id,
                "agent_run_id": agent,
                "event_type": event_type,
                "ts_unix_ms": ts_unix_ms,
                "payload": payload,
            }),
            run_id: run_id.to_string(),
            issue_id: Some("github:OWNER/REPO#1".to_string()),
            agent_run_id: Some(agent.to_string()),
            event_type: event_type.to_string(),
            ts: Some("2026-05-17T00:00:00.000Z".to_string()),
            ts_unix_ms: Some(ts_unix_ms),
        }
    }

    #[test]
    fn tui_quality_summary_parses_status_and_gate_results() {
        let status = serde_json::json!({
            "schema_version": "0.1",
            "success": false,
            "failed_reason": "strict quality gate failed: clippy"
        });
        let report = "\
stack=rust

## fmt
command=cargo fmt --all -- --check
status=exit 0
required=true

## domain:clippy
domain=language.rust
command=cargo clippy --workspace
status=exit 101
required=true
";

        let summary = summarize_quality(Some(&status), Some(report));

        assert_eq!(summary.status, "failed");
        assert_eq!(
            summary.failed_reason.as_deref(),
            Some("strict quality gate failed: clippy")
        );
        assert_eq!(summary.gates.len(), 2);
        assert_eq!(summary.gates[0].name, "fmt");
        assert_eq!(summary.gates[0].status, "exit 0");
        assert_eq!(
            summary.gates[1].command.as_deref(),
            Some("cargo clippy --workspace")
        );
    }

    #[test]
    fn tui_lifecycle_summary_parses_events_and_finalization_status() {
        let events = vec![
            serde_json::json!({
                "ts": "2026-05-17T00:00:00.000Z",
                "event_type": "github.lifecycle.claimed",
                "payload": {
                    "accepted": true,
                    "verification_status": "verified",
                    "detail": "claim accepted"
                }
            }),
            serde_json::json!({
                "ts": "2026-05-17T00:01:00.000Z",
                "event_type": "github.lifecycle.finalization",
                "payload": {
                    "status": "review_required",
                    "release": {"verification_status": "verified"}
                }
            }),
        ];
        let finalization = serde_json::json!({"status": "review_required"});

        let summary = summarize_lifecycle(&events, Some(&finalization));

        assert_eq!(summary.status, "review_required");
        assert_eq!(summary.events.len(), 2);
        assert_eq!(
            summary.events[0].detail.as_deref(),
            Some("accepted=true verification=verified detail=claim accepted")
        );
        assert_eq!(
            summary.events[1].detail.as_deref(),
            Some("status=review_required verification=verified")
        );
    }

    #[test]
    fn tui_graph_nodes_render_agent_state_and_activity_from_events() {
        let graph = serde_json::json!({
            "nodes": [
                {
                    "agent_run_id": "agent-writer",
                    "role": "Implementer",
                    "runtime": "codex",
                    "write_scope": "repo"
                },
                {
                    "agent_run_id": "agent-helper",
                    "role": "Researcher",
                    "runtime": "codex",
                    "write_scope": "read_only"
                }
            ]
        });
        let writer = trace_record(
            "agent-writer",
            "runtime.process.started",
            serde_json::json!({
                "root_pid": 100,
                "process_group_id": 100,
                "memory_group_id": "agent-writer"
            }),
        );
        let helper = trace_record(
            "agent-helper",
            "runtime.process.terminated",
            serde_json::json!({
                "root_pid": 101,
                "process_group_id": 101,
                "memory_group_id": "agent-helper"
            }),
        );
        let records = vec![&writer, &helper];

        let nodes = graph_nodes(&graph, &records, &[]);

        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].contains("agent-writer role=Implementer state=active scope=repo"));
        assert!(nodes[0].contains("activity=pid=100 pgid=100 memory=agent-writer"));
        assert!(nodes[1].contains("agent-helper role=Researcher state=complete scope=read_only"));
    }

    #[test]
    fn tui_jsonl_reader_handles_missing_empty_and_invalid_artifacts() {
        let root = env::temp_dir().join(format!("agentactr-tui-jsonl-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let missing = root.join("missing.jsonl");
        assert!(read_jsonl_optional(&missing).unwrap().is_empty());

        let events = root.join("events.jsonl");
        fs::write(
            &events,
            format!(
                "{}\n\n{}\n",
                serde_json::json!({"event_type": "one"}),
                serde_json::json!({"event_type": "two"})
            ),
        )
        .unwrap();
        assert_eq!(read_jsonl_optional(&events).unwrap().len(), 2);

        let invalid = root.join("invalid.jsonl");
        fs::write(&invalid, "{not-json}\n").unwrap();
        let err = read_jsonl_optional(&invalid).unwrap_err();
        assert!(err.contains("line 1"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tui_quality_status_reader_reports_actual_legacy_path() {
        let root = env::temp_dir().join(format!(
            "agentactr-tui-quality-status-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let preferred = root.join("quality_report.status.json");
        let legacy = root.join("quality_status.json");
        fs::write(
            &legacy,
            serde_json::json!({
                "schema_version": "0.1",
                "success": true
            })
            .to_string(),
        )
        .unwrap();

        let (path, status) = read_quality_status_optional(&preferred, &legacy)
            .unwrap()
            .unwrap();

        assert_eq!(path, legacy);
        assert_eq!(
            status.get("success").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tui_latest_uses_trace_timestamp_order_and_reports_empty_guidance() {
        let older = run_trace_record(
            "run-older",
            "agent-1",
            "run.status.updated",
            10,
            serde_json::json!({"status": "running"}),
        );
        let newer = run_trace_record(
            "run-newer",
            "agent-2",
            "run.status.updated",
            20,
            serde_json::json!({"status": "completed"}),
        );
        let records = vec![older, newer];
        let trace_path = Path::new("/tmp/agentactr-events.jsonl");

        assert_eq!(
            latest_tui_run_id(&records, trace_path).as_deref(),
            Ok("run-newer")
        );

        let err = latest_tui_run_id(&[], trace_path).unwrap_err();
        assert!(err.contains("no runs found"));
        assert!(err.contains("agentactr tui run RUN_ID"));
    }

    #[test]
    fn tui_no_color_state_contains_no_ansi_escape() {
        assert_eq!(color_state("failed", false), "failed");
        assert_eq!(color_state("active", false), "active");
    }
}
