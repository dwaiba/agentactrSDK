use crate::artifacts::{collect_artifact_integrity, ArtifactIntegrityContext};
use crate::{load_agentactr_config, load_run_artifact_context, run_trace_path};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::Path;

pub(crate) fn latest_run_statuses(records: &[TraceRecord]) -> HashMap<String, String> {
    let mut statuses = HashMap::new();
    for record in records {
        if record.event_type == "run.status.updated" {
            if let Some(status) = record
                .value
                .pointer("/payload/status")
                .and_then(serde_json::Value::as_str)
            {
                statuses.insert(record.run_id.clone(), status.to_string());
            }
        }
    }
    statuses
}

pub(crate) fn latest_run_status(records: &[TraceRecord], run_id: &str) -> String {
    records
        .iter()
        .rev()
        .find(|record| record.run_id == run_id && record.event_type == "run.status.updated")
        .and_then(|record| record.value.pointer("/payload/status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

#[derive(Clone, Debug)]
pub(crate) struct TraceRecord {
    pub(crate) line_number: usize,
    pub(crate) value: serde_json::Value,
    pub(crate) run_id: String,
    pub(crate) issue_id: Option<String>,
    pub(crate) agent_run_id: Option<String>,
    pub(crate) event_type: String,
    pub(crate) ts: Option<String>,
    pub(crate) ts_unix_ms: Option<u128>,
}

#[derive(Clone, Debug)]
pub(crate) struct TraceRunSummary {
    pub(crate) run_id: String,
    pub(crate) issue_id: Option<String>,
    pub(crate) event_count: usize,
    pub(crate) first_ts: Option<String>,
    pub(crate) last_ts: Option<String>,
    pub(crate) last_event_type: Option<String>,
    pub(crate) last_ts_unix_ms: Option<u128>,
}

pub(crate) fn cmd_trace(args: &mut [String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("list") => {
            let config = load_agentactr_config(None)?;
            let trace_path = run_trace_path(&config)?;
            let records = read_trace_records(&trace_path)?;
            let summaries = summarize_trace_runs(&records);
            println!("trace_path={}", trace_path.display());
            println!("runs={}", summaries.len());
            for summary in summaries {
                println!(
                    "run_id={} issue_id={} events={} first_ts={} last_ts={} last_event={}",
                    summary.run_id,
                    summary.issue_id.unwrap_or_else(|| "unknown".to_string()),
                    summary.event_count,
                    summary.first_ts.unwrap_or_else(|| "unknown".to_string()),
                    summary.last_ts.unwrap_or_else(|| "unknown".to_string()),
                    summary
                        .last_event_type
                        .unwrap_or_else(|| "unknown".to_string())
                );
            }
            Ok(())
        }
        Some("show") => {
            let run_id = args.get(2).ok_or("usage: agentactr trace show RUN_ID")?;
            let config = load_agentactr_config(None)?;
            let trace_path = run_trace_path(&config)?;
            let records = read_trace_records(&trace_path)?
                .into_iter()
                .filter(|record| record.run_id == *run_id)
                .collect::<Vec<_>>();
            if records.is_empty() {
                return Err(format!(
                    "no trace events found for run `{run_id}` in {}",
                    trace_path.display()
                ));
            }
            let artifact_integrity = load_run_artifact_context(&config, run_id)
                .and_then(|context| {
                    collect_artifact_integrity(&ArtifactIntegrityContext {
                        run_id: &context.run_id,
                        artifact_dir: &context.artifact_dir,
                    })
                })
                .unwrap_or_else(|err| {
                    serde_json::json!({
                        "schema_version": "0.1",
                        "run_id": run_id,
                        "status": "unavailable",
                        "verified": false,
                        "error": err,
                    })
                });
            print_trace_show(&trace_path, run_id, &records, Some(&artifact_integrity));
            Ok(())
        }
        _ => Err("usage: agentactr trace list | trace show RUN_ID".to_string()),
    }
}

pub(crate) fn read_trace_records(trace_path: &Path) -> Result<Vec<TraceRecord>, String> {
    let content = match fs::read_to_string(trace_path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("read trace events {}: {err}", trace_path.display())),
    };
    let mut records = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str::<serde_json::Value>(line).map_err(|e| {
            format!(
                "parse trace event {} line {}: {e}",
                trace_path.display(),
                index + 1
            )
        })?;
        let run_id = value
            .get("run_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                format!(
                    "trace event {} line {} is missing string run_id",
                    trace_path.display(),
                    index + 1
                )
            })?
            .to_string();
        let event_type = value
            .get("event_type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                format!(
                    "trace event {} line {} is missing string event_type",
                    trace_path.display(),
                    index + 1
                )
            })?
            .to_string();
        records.push(TraceRecord {
            line_number: index + 1,
            run_id,
            issue_id: value
                .get("issue_id")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            agent_run_id: value
                .get("agent_run_id")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            event_type,
            ts: value
                .get("ts")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            ts_unix_ms: value
                .get("ts_unix_ms")
                .and_then(serde_json::Value::as_u64)
                .map(u128::from),
            value,
        });
    }
    Ok(records)
}

pub(crate) fn summarize_trace_runs(records: &[TraceRecord]) -> Vec<TraceRunSummary> {
    let mut summaries: BTreeMap<String, TraceRunSummary> = BTreeMap::new();
    for record in records {
        let summary = summaries
            .entry(record.run_id.clone())
            .or_insert_with(|| TraceRunSummary {
                run_id: record.run_id.clone(),
                issue_id: record.issue_id.clone(),
                event_count: 0,
                first_ts: record.ts.clone(),
                last_ts: record.ts.clone(),
                last_event_type: Some(record.event_type.clone()),
                last_ts_unix_ms: record.ts_unix_ms,
            });
        summary.event_count += 1;
        if summary.issue_id.is_none() {
            summary.issue_id = record.issue_id.clone();
        }
        if summary.first_ts.is_none() {
            summary.first_ts = record.ts.clone();
        }
        summary.last_ts = record.ts.clone().or_else(|| summary.last_ts.clone());
        summary.last_event_type = Some(record.event_type.clone());
        summary.last_ts_unix_ms = record.ts_unix_ms.or(summary.last_ts_unix_ms);
    }
    let mut out = summaries.into_values().collect::<Vec<_>>();
    out.sort_by(|left, right| {
        right
            .last_ts_unix_ms
            .cmp(&left.last_ts_unix_ms)
            .then_with(|| left.run_id.cmp(&right.run_id))
    });
    out
}

pub(crate) fn print_trace_show(
    trace_path: &Path,
    run_id: &str,
    records: &[TraceRecord],
    artifact_integrity: Option<&serde_json::Value>,
) {
    let issue_id = records
        .iter()
        .find_map(|record| record.issue_id.clone())
        .unwrap_or_else(|| "unknown".to_string());
    println!("trace_path={}", trace_path.display());
    println!("run_id={run_id}");
    println!("issue_id={issue_id}");
    println!("events={}", records.len());
    if let Some(first) = records.first().and_then(|record| record.ts.as_ref()) {
        println!("first_ts={first}");
    }
    if let Some(last) = records.last().and_then(|record| record.ts.as_ref()) {
        println!("last_ts={last}");
    }
    print_trace_run_status(records);
    print_trace_agent_last_events(records);
    print_trace_failures(records);
    print_trace_runtime_processes(records);
    print_trace_github_rate_limits(records);
    print_trace_artifacts(records);
    print_trace_artifact_integrity(artifact_integrity);
    println!("events:");
    for record in records {
        println!(
            "  line={} ts={} agent={} event={}",
            record.line_number,
            record.ts.as_deref().unwrap_or("unknown"),
            record.agent_run_id.as_deref().unwrap_or("root"),
            record.event_type
        );
    }
}

fn print_trace_artifact_integrity(integrity: Option<&serde_json::Value>) {
    let Some(integrity) = integrity else {
        println!("artifact_integrity=unavailable");
        return;
    };
    let status = integrity
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let verified = integrity
        .get("verified")
        .and_then(serde_json::Value::as_bool)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let children = integrity
        .get("child_handoffs")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    println!("artifact_integrity={status} verified={verified} child_handoffs={children}");
    if let Some(prompt) = integrity.get("writer_prompt") {
        println!(
            "artifact_integrity_writer_prompt status={} expected_sha256={} actual_sha256={}",
            json_display(prompt.get("status")),
            json_display(prompt.get("expected_sha256")),
            json_display(prompt.get("actual_sha256"))
        );
    }
    if let Some(diff) = integrity.get("workspace_diff") {
        println!(
            "artifact_integrity_workspace_diff status={} expected_sha256={} actual_sha256={}",
            json_display(diff.get("status")),
            json_display(diff.get("expected_sha256")),
            json_display(diff.get("actual_sha256"))
        );
    }
    if let Some(plan) = integrity.get("merge_plan") {
        println!(
            "artifact_integrity_merge_plan status={} expected_sha256={} actual_sha256={}",
            json_display(plan.get("status")),
            json_display(plan.get("expected_sha256")),
            json_display(plan.get("actual_sha256"))
        );
    }
    if let Some(children) = integrity
        .get("child_handoffs")
        .and_then(serde_json::Value::as_array)
    {
        for child in children {
            println!(
                "artifact_integrity_child agent={} handoff_status={} prompt_status={}",
                json_display(child.get("agent_run_id")),
                json_display(child.pointer("/handoff/status")),
                json_display(child.pointer("/prompt/status"))
            );
        }
    }
}

fn print_trace_run_status(records: &[TraceRecord]) {
    let status = records
        .iter()
        .rev()
        .find(|record| record.event_type == "run.status.updated")
        .and_then(|record| record.value.pointer("/payload/status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    println!("run_status={status}");
}

fn print_trace_agent_last_events(records: &[TraceRecord]) {
    let mut by_agent = BTreeMap::<String, &TraceRecord>::new();
    for record in records {
        if let Some(agent) = &record.agent_run_id {
            by_agent.insert(agent.clone(), record);
        }
    }
    println!("agents={}", by_agent.len());
    for (agent, record) in by_agent {
        println!(
            "agent={} last_event={} ts={}",
            agent,
            record.event_type,
            record.ts.as_deref().unwrap_or("unknown")
        );
    }
}

fn print_trace_failures(records: &[TraceRecord]) {
    let failures = records
        .iter()
        .filter(|record| record.event_type.ends_with(".failed") || record.event_type == "error")
        .collect::<Vec<_>>();
    println!("failures={}", failures.len());
    for record in failures {
        let error = record
            .value
            .pointer("/payload/error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unspecified");
        println!(
            "failure line={} event={} error={}",
            record.line_number, record.event_type, error
        );
    }
}

fn print_trace_runtime_processes(records: &[TraceRecord]) {
    let process_events = records
        .iter()
        .filter(|record| record.event_type.starts_with("runtime.process."))
        .collect::<Vec<_>>();
    println!("runtime_process_events={}", process_events.len());
    for record in process_events {
        let payload = record
            .value
            .get("payload")
            .unwrap_or(&serde_json::Value::Null);
        println!(
            "runtime_process line={} kind={} agent={} root_pid={} process_group_id={} container_ref={} vm_ref={} memory_group_id={}",
            record.line_number,
            record.event_type,
            record.agent_run_id.as_deref().unwrap_or("unknown"),
            json_display(payload.pointer("/root_pid")),
            json_display(payload.pointer("/process_group_id")),
            json_display(payload.pointer("/container_ref")),
            json_display(payload.pointer("/vm_ref")),
            json_display(payload.pointer("/memory_group_id")),
        );
    }
}

fn print_trace_github_rate_limits(records: &[TraceRecord]) {
    let rate_events = records
        .iter()
        .filter(|record| record.event_type == "github.rate_limit.updated")
        .collect::<Vec<_>>();
    println!("github_rate_limit_events={}", rate_events.len());
    for record in rate_events {
        let payload = record
            .value
            .get("payload")
            .unwrap_or(&serde_json::Value::Null);
        println!(
            "github_rate_limit line={} status={} reason={} retry_after_ms={}",
            record.line_number,
            json_display(payload.get("status")),
            json_display(payload.get("reason")),
            json_display(payload.get("retry_after_ms")),
        );
    }
}

fn print_trace_artifacts(records: &[TraceRecord]) {
    let artifact_events = records
        .iter()
        .filter(|record| {
            matches!(
                record.event_type.as_str(),
                "context.manifest.written"
                    | "adapter.version_reported"
                    | "finalization.deferred"
                    | "quality.rerun.completed"
                    | "quality.rerun.failed"
                    | "vcs.diff.recorded"
                    | "vcs.merge_plan.recorded"
                    | "vcs.status.read"
            )
        })
        .collect::<Vec<_>>();
    println!("artifact_events={}", artifact_events.len());
    for record in artifact_events {
        println!(
            "artifact_event line={} event={} payload={}",
            record.line_number,
            record.event_type,
            compact_json(
                record
                    .value
                    .get("payload")
                    .unwrap_or(&serde_json::Value::Null)
            )
        );
    }
}

pub(crate) fn json_display(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(serde_json::Value::Number(value)) => value.to_string(),
        Some(serde_json::Value::Bool(value)) => value.to_string(),
        Some(serde_json::Value::Null) | None => "none".to_string(),
        Some(value) => compact_json(value),
    }
}

pub(crate) fn compact_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "unrenderable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_run_id;
    use std::env;

    #[test]
    fn trace_records_parse_and_summarize_runs_without_mutating_trace() {
        let root = env::temp_dir().join(format!(
            "agentactr-trace-list-test-{}-{}",
            std::process::id(),
            new_run_id("trace")
        ));
        fs::create_dir_all(&root).unwrap();
        let trace_path = root.join("events.jsonl");
        fs::write(
            &trace_path,
            [
                serde_json::json!({
                    "run_id": "run-1",
                    "issue_id": "github:OWNER/REPO#1",
                    "agent_run_id": null,
                    "event_type": "run.status.updated",
                    "ts": "2026-01-01T00:00:00.000Z",
                    "ts_unix_ms": 1,
                    "payload": {"status": "started"}
                })
                .to_string(),
                serde_json::json!({
                    "run_id": "run-2",
                    "issue_id": "github:OWNER/REPO#2",
                    "agent_run_id": "agent-2",
                    "event_type": "agent.completed",
                    "ts": "2026-01-01T00:00:02.000Z",
                    "ts_unix_ms": 3,
                    "payload": {}
                })
                .to_string(),
                serde_json::json!({
                    "run_id": "run-1",
                    "issue_id": "github:OWNER/REPO#1",
                    "agent_run_id": "agent-1",
                    "event_type": "phase.completed",
                    "ts": "2026-01-01T00:00:01.000Z",
                    "ts_unix_ms": 2,
                    "payload": {"phase": "quality"}
                })
                .to_string(),
            ]
            .join("\n"),
        )
        .unwrap();
        let before = fs::read_to_string(&trace_path).unwrap();

        let records = read_trace_records(&trace_path).unwrap();
        let summaries = summarize_trace_runs(&records);
        print_trace_show(
            &trace_path,
            "run-1",
            &records
                .iter()
                .filter(|record| record.run_id == "run-1")
                .cloned()
                .collect::<Vec<_>>(),
            None,
        );

        let after = fs::read_to_string(&trace_path).unwrap();
        assert_eq!(before, after);
        assert_eq!(records.len(), 3);
        assert_eq!(summaries[0].run_id, "run-2");
        assert_eq!(summaries[1].run_id, "run-1");
        assert_eq!(summaries[1].event_count, 2);
        assert_eq!(
            summaries[1].last_event_type.as_deref(),
            Some("phase.completed")
        );
        let _ = fs::remove_dir_all(root);
    }
}
