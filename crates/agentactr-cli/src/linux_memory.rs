use crate::{current_epoch_millis, iso_timestamp_from_epoch_millis};
use agentactr_sdk::{
    AdapterCapabilities, AdapterVersionReport, AgentRunId, HelperMemoryCandidate,
    LinuxMemoryConfig, MemoryAction, MemoryActionResult, MemoryController, MemoryGovernorPolicy,
    MemoryGroup, MemoryGroupId, MemoryGroupRequest, MemoryLease, MemoryPolicyRef,
    MemoryPressureSnapshot, MemoryPressureTransition, MemorySample, RunResourceGovernor,
    RuntimeProcessEvent, RuntimeProcessSupervisor,
};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DEFAULT_CGROUP_ROOT: &str = "/sys/fs/cgroup/agentactr.slice";

#[derive(Clone, Debug)]
pub(crate) struct LinuxMemoryController {
    config: LinuxMemoryConfig,
    cgroup_root: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct MemoryRunContext {
    pub(crate) enforce: bool,
    pub(crate) run_group: Option<PathBuf>,
    pub(crate) agent_group: Option<PathBuf>,
    pub(crate) agent_group_id: Option<MemoryGroupId>,
    pub(crate) status_artifact: PathBuf,
}

impl MemoryRunContext {
    pub(crate) fn agent_memory_lease(&self) -> Option<MemoryLease> {
        self.agent_group_id.clone().map(|group_id| MemoryLease {
            group_id,
            policy: MemoryPolicyRef::new("linux_memory.agent"),
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedAgentMemory {
    pub(crate) lease: MemoryLease,
    pub(crate) group: PathBuf,
}

#[derive(Clone, Debug)]
struct MemoryPolicyBytes {
    run_high: u64,
    run_max: u64,
    agent_high: u64,
    agent_max: u64,
}

#[derive(Clone, Debug)]
struct HostMemoryStatus {
    platform: &'static str,
    cgroup_v2: bool,
    psi_memory: bool,
    mode: String,
    cgroup_root: PathBuf,
}

pub(crate) struct MemoryMonitor {
    stop: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
    handle: Option<JoinHandle<()>>,
    registry: Option<ActiveMemoryRegistry>,
    registry_key: Option<String>,
}

impl MemoryMonitor {
    pub(crate) fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|failure| failure.clone())
    }

    pub(crate) fn stop(mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| "memory monitor thread panicked".to_string())?;
        }
        if let (Some(registry), Some(key)) = (&self.registry, &self.registry_key) {
            registry.unregister(key);
        }
        if let Some(err) = self.failure() {
            Err(err)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MemoryMonitorSubject {
    pub(crate) group_id: Option<MemoryGroupId>,
    pub(crate) agent_run_id: AgentRunId,
    pub(crate) runtime_event: RuntimeProcessEvent,
    pub(crate) read_only_helper: bool,
    pub(crate) priority: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct MemoryTraceContext {
    pub(crate) trace_path: PathBuf,
    pub(crate) repo: String,
    pub(crate) issue: String,
}

#[derive(Clone, Debug)]
struct ActiveMemoryTarget {
    group_id: Option<MemoryGroupId>,
    agent_run_id: AgentRunId,
    runtime_event: RuntimeProcessEvent,
    cgroup: PathBuf,
    read_only_helper: bool,
    priority: i32,
    started_at_unix_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ActiveMemoryRegistry {
    inner: Arc<Mutex<ActiveMemoryRegistryState>>,
}

#[derive(Clone, Debug, Default)]
struct ActiveMemoryRegistryState {
    targets: HashMap<String, ActiveMemoryTarget>,
    cancelled: HashSet<String>,
}

impl ActiveMemoryRegistry {
    pub(crate) fn register(&self, cgroup: PathBuf, subject: &MemoryMonitorSubject) -> String {
        let key = subject.agent_run_id.as_str().to_string();
        if let Ok(mut state) = self.inner.lock() {
            state.targets.insert(
                key.clone(),
                ActiveMemoryTarget {
                    group_id: subject.group_id.clone(),
                    agent_run_id: subject.agent_run_id.clone(),
                    runtime_event: subject.runtime_event.clone(),
                    cgroup,
                    read_only_helper: subject.read_only_helper,
                    priority: subject.priority,
                    started_at_unix_ms: unix_ms(),
                },
            );
        }
        key
    }

    fn unregister(&self, key: &str) {
        if let Ok(mut state) = self.inner.lock() {
            state.targets.remove(key);
        }
    }

    fn helper_candidates(&self) -> Vec<HelperMemoryCandidate> {
        let Ok(state) = self.inner.lock() else {
            return Vec::new();
        };
        state
            .targets
            .values()
            .map(|target| {
                let memory_pressure_score = sample_cgroup(&target.cgroup)
                    .ok()
                    .and_then(|sample| {
                        sample
                            .get("memory_current_bytes")
                            .and_then(serde_json::Value::as_u64)
                    })
                    .unwrap_or(0);
                HelperMemoryCandidate {
                    agent_run_id: target.agent_run_id.clone(),
                    read_only: target.read_only_helper,
                    priority: target.priority,
                    memory_pressure_score,
                    started_at_unix_ms: target.started_at_unix_ms,
                }
            })
            .collect()
    }

    fn mark_cancelled(&self, agent_run_id: &AgentRunId) -> Option<ActiveMemoryTarget> {
        let Ok(mut state) = self.inner.lock() else {
            return None;
        };
        let key = agent_run_id.as_str().to_string();
        if state.cancelled.contains(&key) {
            return None;
        }
        let target = state.targets.get(&key)?.clone();
        state.cancelled.insert(key);
        Some(target)
    }

    fn target_by_group_id(&self, group_id: &MemoryGroupId) -> Option<ActiveMemoryTarget> {
        let Ok(state) = self.inner.lock() else {
            return None;
        };
        state
            .targets
            .values()
            .find(|target| target.group_id.as_ref() == Some(group_id))
            .cloned()
    }
}

impl LinuxMemoryController {
    pub(crate) fn new(config: &LinuxMemoryConfig) -> Self {
        let cgroup_root = env::var("AGENTACTR_CGROUP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| configured_cgroup_root(config));
        Self {
            config: config.clone(),
            cgroup_root,
        }
    }

    pub(crate) fn status_lines(&self) -> Vec<String> {
        let status = self.host_status();
        let mut lines = vec![
            format!("  platform = {}", status.platform),
            format!("  mode = {}", status.mode),
            format!("  cgroup_v2 = {}", available_text(status.cgroup_v2)),
            format!("  psi_memory = {}", available_text(status.psi_memory)),
        ];
        if cfg!(target_os = "linux") {
            lines.push(format!("  cgroup_root = {}", status.cgroup_root.display()));
            lines.push(format!(
                "  attach_policy = {}",
                if self.should_enforce() {
                    "fail_closed"
                } else {
                    "observe_only"
                }
            ));
        } else {
            lines.push(
                "  attach_policy = observe_elsewhere; enforcement is active only on Linux cgroup v2 hosts"
                    .to_string(),
            );
        }
        lines
    }

    pub(crate) fn memory_status_text(&self) -> String {
        let status = self.host_status();
        format!(
            "platform={} mode={} cgroup_v2={} psi_memory={} cgroup_root={}",
            status.platform,
            status.mode,
            status.cgroup_v2,
            status.psi_memory,
            status.cgroup_root.display()
        )
    }

    pub(crate) fn prepare_run(
        &self,
        run_id: &str,
        agent_run_id: &str,
        artifact_dir: &Path,
    ) -> Result<MemoryRunContext, String> {
        let status_artifact = artifact_dir.join("memory_status.json");
        let policy = self.memory_policy_bytes()?;
        if !self.should_enforce() {
            let context = MemoryRunContext {
                enforce: false,
                run_group: None,
                agent_group: None,
                agent_group_id: None,
                status_artifact,
            };
            self.write_status_artifact(&context, &policy, None)?;
            return Ok(context);
        }

        self.require_linux_memory_host()?;
        if let Some(parent) = self.cgroup_root.parent() {
            enable_memory_controller(parent)?;
        }
        fs::create_dir_all(&self.cgroup_root)
            .map_err(|e| format!("create cgroup root {}: {e}", self.cgroup_root.display()))?;
        enable_memory_controller(&self.cgroup_root)?;

        let run_group = self.cgroup_root.join(safe_cgroup_name(run_id)?);
        fs::create_dir_all(&run_group)
            .map_err(|e| format!("create run cgroup {}: {e}", run_group.display()))?;
        write_cgroup_value(&run_group, "memory.high", policy.run_high)?;
        write_cgroup_value(&run_group, "memory.max", policy.run_max)?;
        enable_memory_controller(&run_group)?;
        let agent_memory = self.prepare_agent_group_in_run(
            &run_group,
            run_id,
            agent_run_id,
            &policy,
            &status_artifact,
        )?;

        let context = MemoryRunContext {
            enforce: true,
            run_group: Some(run_group),
            agent_group: Some(agent_memory.group),
            agent_group_id: Some(agent_memory.lease.group_id),
            status_artifact,
        };
        let sample = context
            .agent_group
            .as_deref()
            .and_then(|group| sample_cgroup(group).ok());
        self.write_status_artifact(&context, &policy, sample.as_ref())?;
        Ok(context)
    }

    pub(crate) fn prepare_child_agent(
        &self,
        context: &MemoryRunContext,
        run_id: &str,
        agent_run_id: &str,
        artifact_dir: &Path,
    ) -> Result<Option<PreparedAgentMemory>, String> {
        if !context.enforce {
            return Ok(None);
        }
        let run_group = context
            .run_group
            .as_deref()
            .ok_or("memory enforcement context is missing a run cgroup")?;
        let status_artifact = artifact_dir.join("memory_status.json");
        let policy = self.memory_policy_bytes()?;
        let prepared = self.prepare_agent_group_in_run(
            run_group,
            run_id,
            agent_run_id,
            &policy,
            &status_artifact,
        )?;
        let child_context = MemoryRunContext {
            enforce: true,
            run_group: Some(run_group.to_path_buf()),
            agent_group: Some(prepared.group.clone()),
            agent_group_id: Some(prepared.lease.group_id.clone()),
            status_artifact,
        };
        let sample = child_context
            .agent_group
            .as_deref()
            .and_then(|group| sample_cgroup(group).ok());
        self.write_status_artifact(&child_context, &policy, sample.as_ref())?;
        Ok(Some(prepared))
    }

    pub(crate) fn sample_agent(&self, context: &MemoryRunContext) -> Result<(), String> {
        let Some(agent_group) = context.agent_group.as_deref() else {
            self.write_status_artifact(context, &self.memory_policy_bytes()?, None)?;
            return Ok(());
        };
        let sample = sample_cgroup(agent_group)?;
        self.write_status_artifact(context, &self.memory_policy_bytes()?, Some(&sample))
    }

    pub(crate) fn trace_payload(&self, context: &MemoryRunContext) -> serde_json::Value {
        json!({
            "mode": self.config.mode,
            "enforce": context.enforce,
            "run_group": context.run_group.as_ref().map(|path| path.display().to_string()),
            "agent_group": context.agent_group.as_ref().map(|path| path.display().to_string()),
            "agent_group_id": context.agent_group_id.as_ref().map(|id| id.as_str()),
            "status_artifact": context.status_artifact.display().to_string(),
            "cgroup_root": self.cgroup_root.display().to_string(),
        })
    }

    fn should_enforce(&self) -> bool {
        self.config.enabled
            && cfg!(target_os = "linux")
            && !matches!(
                self.config.mode.as_str(),
                "observe" | "observe_only" | "disabled" | "off"
            )
    }

    fn require_linux_memory_host(&self) -> Result<(), String> {
        let status = self.host_status();
        if !status.cgroup_v2 && self.config.cgroup_v2_required {
            return Err("Linux memory enforcement requires cgroup v2; set linux_memory.mode = \"observe_only\" only for an explicit degraded run".to_string());
        }
        if !status.psi_memory && self.config.psi_required {
            return Err("Linux memory enforcement requires PSI memory pressure files; set linux_memory.psi_required = false only for an explicit degraded run".to_string());
        }
        Ok(())
    }

    fn memory_policy_bytes(&self) -> Result<MemoryPolicyBytes, String> {
        Ok(MemoryPolicyBytes {
            run_high: parse_memory_bytes(&self.config.per_issue_memory_high)?,
            run_max: parse_memory_bytes(&self.config.per_issue_memory_max)?,
            agent_high: parse_memory_bytes(&self.config.per_agent_memory_high)?,
            agent_max: parse_memory_bytes(&self.config.per_agent_memory_max)?,
        })
    }

    fn prepare_agent_group_in_run(
        &self,
        run_group: &Path,
        run_id: &str,
        agent_run_id: &str,
        policy: &MemoryPolicyBytes,
        status_artifact: &Path,
    ) -> Result<PreparedAgentMemory, String> {
        let agent_group = run_group.join(safe_cgroup_name(agent_run_id)?);
        fs::create_dir_all(&agent_group)
            .map_err(|e| format!("create agent cgroup {}: {e}", agent_group.display()))?;
        write_cgroup_value(&agent_group, "memory.high", policy.agent_high)?;
        write_cgroup_value(&agent_group, "memory.max", policy.agent_max)?;
        let lease = MemoryLease {
            group_id: agent_memory_group_id(run_id, agent_run_id),
            policy: MemoryPolicyRef::new("linux_memory.agent"),
        };
        if let Some(parent) = status_artifact.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        Ok(PreparedAgentMemory {
            lease,
            group: agent_group,
        })
    }

    fn host_status(&self) -> HostMemoryStatus {
        HostMemoryStatus {
            platform: if cfg!(target_os = "linux") {
                "linux"
            } else {
                "non-linux"
            },
            cgroup_v2: Path::new("/sys/fs/cgroup/cgroup.controllers").exists(),
            psi_memory: Path::new("/proc/pressure/memory").exists(),
            mode: self.config.mode.clone(),
            cgroup_root: self.cgroup_root.clone(),
        }
    }

    fn write_status_artifact(
        &self,
        context: &MemoryRunContext,
        policy: &MemoryPolicyBytes,
        sample: Option<&serde_json::Value>,
    ) -> Result<(), String> {
        let payload = json!({
            "schema_version": "0.1",
            "enabled": self.config.enabled,
            "mode": self.config.mode,
            "enforce": context.enforce,
            "cgroup_root": self.cgroup_root.display().to_string(),
            "root_group": self.config.root_group,
            "run_group": context.run_group.as_ref().map(|path| path.display().to_string()),
            "agent_group": context.agent_group.as_ref().map(|path| path.display().to_string()),
            "agent_group_id": context.agent_group_id.as_ref().map(|id| id.as_str()),
            "policy": {
                "run_high_bytes": policy.run_high,
                "run_max_bytes": policy.run_max,
                "agent_high_bytes": policy.agent_high,
                "agent_max_bytes": policy.agent_max,
                "psi_memory_some_threshold_us": self.config.psi_memory_some_threshold_us,
                "psi_memory_window_us": self.config.psi_memory_window_us,
                "oom_score_adj": self.config.oom_score_adj,
                "kill_policy": self.config.kill_policy,
                "oom_policy": self.config.oom_policy,
            },
            "host": {
                "platform": self.host_status().platform,
                "cgroup_v2": self.host_status().cgroup_v2,
                "psi_memory": self.host_status().psi_memory,
            },
            "sample": sample,
        });
        if let Some(parent) = context.status_artifact.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        fs::write(
            &context.status_artifact,
            serde_json::to_string_pretty(&payload)
                .map_err(|e| format!("render memory status artifact: {e}"))?,
        )
        .map_err(|e| format!("write {}: {e}", context.status_artifact.display()))
    }
}

impl MemoryController for LinuxMemoryController {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport {
            adapter_kind: "memory_controller".to_string(),
            adapter_name: "agentactr-linux-memory".to_string(),
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            product_name: "linux-cgroup-v2".to_string(),
            product_version: if cfg!(target_os = "linux") {
                "kernel-cgroup-v2".to_string()
            } else {
                "unavailable".to_string()
            },
            api_version: "cgroup-v2".to_string(),
            capability_digest:
                "create-run-group,attach-pid,sample,reclaim,kill-group,finalize-group".to_string(),
            degraded_features: if cfg!(target_os = "linux") {
                Vec::new()
            } else {
                vec!["linux_cgroup_v2".to_string()]
            },
            required_actions: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn capabilities(&self) -> AdapterCapabilities {
        let mut supported_features = vec![
            "create_run_group".to_string(),
            "attach_pid".to_string(),
            "sample".to_string(),
            "finalize_group".to_string(),
        ];
        let mut degraded_features = Vec::new();
        if cfg!(target_os = "linux") {
            supported_features.push("memory_reclaim".to_string());
            supported_features.push("cgroup_kill".to_string());
        } else {
            degraded_features.push("memory_reclaim".to_string());
            degraded_features.push("cgroup_kill".to_string());
        }
        AdapterCapabilities {
            adapter_kind: "memory_controller".to_string(),
            supported_features,
            degraded_features,
            required_actions: Vec::new(),
        }
    }

    fn create_run_group(&self, req: MemoryGroupRequest) -> Result<MemoryGroup, String> {
        let path = req.path.unwrap_or_else(|| self.cgroup_root.clone());
        if self.should_enforce() {
            fs::create_dir_all(&path)
                .map_err(|e| format!("create cgroup {}: {e}", path.display()))?;
        }
        Ok(MemoryGroup {
            group_id: req.group_id,
            path: Some(path),
        })
    }

    fn attach_pid(&self, group: &MemoryGroup, pid: u32) -> Result<(), String> {
        attach_pid_to_cgroup(memory_group_path(group)?, pid)
    }

    fn sample(&self, group: &MemoryGroup) -> Result<MemorySample, String> {
        let payload = sample_cgroup(memory_group_path(group)?)?;
        Ok(MemorySample {
            payload_json: serde_json::to_string(&payload)
                .map_err(|e| format!("render memory sample: {e}"))?,
        })
    }

    fn reclaim(&self, group: &MemoryGroup, bytes: u64) -> Result<MemoryActionResult, String> {
        let group_id = memory_group_id(group);
        let action = MemoryAction::Reclaim { group_id, bytes };
        match reclaim_cgroup_memory(memory_group_path(group)?, bytes) {
            Ok(()) => Ok(MemoryActionResult::succeeded(
                action,
                "memory.reclaim completed",
            )),
            Err(err) => Ok(MemoryActionResult::degraded(action, err)),
        }
    }

    fn kill_group(
        &self,
        group: &MemoryGroup,
        terminal_cleanup: bool,
    ) -> Result<MemoryActionResult, String> {
        let action = MemoryAction::KillGroup {
            group_id: memory_group_id(group),
            terminal_cleanup,
        };
        match kill_cgroup(memory_group_path(group)?) {
            Ok(()) => Ok(MemoryActionResult::succeeded(
                action,
                "cgroup.kill completed",
            )),
            Err(err) => Ok(MemoryActionResult::degraded(action, err)),
        }
    }

    fn finalize_group(&self, group: &MemoryGroup) -> Result<MemoryActionResult, String> {
        let action = MemoryAction::KillGroup {
            group_id: memory_group_id(group),
            terminal_cleanup: true,
        };
        let path = memory_group_path(group)?;
        match fs::remove_dir(path) {
            Ok(()) => Ok(MemoryActionResult::succeeded(
                action,
                "memory cgroup finalized",
            )),
            Err(_err) if !path.exists() => Ok(MemoryActionResult::succeeded(
                action,
                "memory cgroup already finalized",
            )),
            Err(err) => Ok(MemoryActionResult::failed(
                action,
                format!("finalize cgroup {}: {err}", path.display()),
            )),
        }
    }
}

fn memory_group_path(group: &MemoryGroup) -> Result<&Path, String> {
    group
        .path
        .as_deref()
        .ok_or("memory group is missing cgroup path".to_string())
}

fn memory_group_id(group: &MemoryGroup) -> MemoryGroupId {
    group
        .group_id
        .clone()
        .unwrap_or_else(|| MemoryGroupId::new("linux-memory-group"))
}

pub(crate) fn start_memory_monitor(
    cgroup: &Path,
    root_pid: u32,
    artifact_dir: &Path,
    subject: Option<MemoryMonitorSubject>,
    registry: Option<ActiveMemoryRegistry>,
    trace_context: Option<MemoryTraceContext>,
    process_supervisor: Option<Arc<dyn RuntimeProcessSupervisor>>,
) -> Result<Option<MemoryMonitor>, String> {
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }
    let log_path = artifact_dir.join("memory_monitor.jsonl");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let cgroup = fs::canonicalize(cgroup).unwrap_or_else(|_| cgroup.to_path_buf());
    let expected_relative = expected_cgroup_relative_path(&cgroup)?;
    let registry_key = subject.as_ref().and_then(|subject| {
        registry
            .as_ref()
            .map(|registry| registry.register(cgroup.clone(), subject))
    });
    let stop = Arc::new(AtomicBool::new(false));
    let failure = Arc::new(Mutex::new(None));
    let thread_stop = Arc::clone(&stop);
    let thread_failure = Arc::clone(&failure);
    let thread_registry = registry.clone();
    let thread_trace_context = trace_context.clone();
    let thread_process_supervisor = process_supervisor.clone();
    let thread_subject = subject.clone();
    let handle = thread::spawn(move || {
        let mut governor = RunResourceGovernor::new(MemoryGovernorPolicy::default());
        let mut previous_sample: Option<MemoryPressureRawSample> = None;
        while !thread_stop.load(Ordering::SeqCst) {
            let mut event = monitor_once(&cgroup, expected_relative.as_str(), root_pid);
            if let Some(subject) = thread_subject.as_ref() {
                event["run_id"] = json!(subject.runtime_event.run_id.as_str());
                event["agent_run_id"] = json!(subject.agent_run_id.as_str());
                event["memory_group_id"] =
                    json!(subject.group_id.as_ref().map(MemoryGroupId::as_str));
            }
            let raw_sample = MemoryPressureRawSample::from_monitor_event(&event);
            let mut snapshot = raw_sample
                .as_ref()
                .map(|sample| sample.delta(previous_sample.as_ref()))
                .unwrap_or_default();
            if let Some(subject) = thread_subject.as_ref() {
                snapshot.group_id = subject.group_id.clone();
                snapshot.agent_run_id = Some(subject.agent_run_id.clone());
            }
            let helpers = thread_registry
                .as_ref()
                .map(ActiveMemoryRegistry::helper_candidates)
                .unwrap_or_default();
            let transition = governor.observe(&snapshot, &helpers);
            let action_results = execute_memory_actions(
                &transition.actions,
                thread_registry.as_ref(),
                thread_process_supervisor.as_ref().map(Arc::as_ref),
                &thread_failure,
            );
            if transition.previous != transition.next || !transition.actions.is_empty() {
                event["governor"] = json!({
                    "previous": transition.previous.as_str(),
                    "next": transition.next.as_str(),
                    "reason": transition.reason,
                    "actions": transition.actions.iter().map(memory_action_payload).collect::<Vec<_>>(),
                    "action_results": action_results,
                    "helper_candidates": helpers.iter().map(helper_candidate_payload).collect::<Vec<_>>(),
                    "spawn_paused": governor.spawn_paused(),
                });
                if let Some(trace_context) = thread_trace_context.as_ref() {
                    let _ = emit_memory_governor_trace_event(
                        trace_context,
                        &event,
                        &transition,
                        &action_results,
                        &helpers,
                        governor.spawn_paused(),
                    );
                }
            }
            previous_sample = raw_sample;
            if let Some(err) = event.get("failure").and_then(serde_json::Value::as_str) {
                if let Ok(mut failure) = thread_failure.lock() {
                    if failure.is_none() {
                        *failure = Some(err.to_string());
                    }
                }
            }
            let _ = append_jsonl(&log_path, &event);
            thread::sleep(Duration::from_millis(250));
        }
        let _ = append_jsonl(
            &log_path,
            &json!({
                "event": "memory.monitor.stopped",
                "root_pid": root_pid,
            }),
        );
    });
    Ok(Some(MemoryMonitor {
        stop,
        failure,
        handle: Some(handle),
        registry,
        registry_key,
    }))
}

pub(crate) fn preserve_memory_debug_bundle(
    artifact_dir: &Path,
    cgroup: Option<&Path>,
    root_pid: Option<u32>,
    reason: &str,
) -> Result<PathBuf, String> {
    let bundle = artifact_dir.join("memory_debug_bundle");
    fs::create_dir_all(&bundle).map_err(|e| format!("create {}: {e}", bundle.display()))?;
    fs::write(bundle.join("reason.txt"), reason)
        .map_err(|e| format!("write memory debug reason: {e}"))?;
    if let Some(cgroup) = cgroup {
        fs::write(bundle.join("cgroup_path.txt"), cgroup.display().to_string())
            .map_err(|e| format!("write memory debug cgroup path: {e}"))?;
        for file in [
            "cgroup.controllers",
            "cgroup.procs",
            "cgroup.subtree_control",
            "memory.current",
            "memory.peak",
            "memory.high",
            "memory.max",
            "memory.events",
            "memory.stat",
            "memory.pressure",
        ] {
            let source = cgroup.join(file);
            if let Ok(content) = fs::read_to_string(&source) {
                fs::write(bundle.join(file.replace('.', "_")), content)
                    .map_err(|e| format!("write memory debug copy for {file}: {e}"))?;
            }
        }
    }
    if let Some(root_pid) = root_pid {
        let descendants = process_tree_pids(root_pid);
        fs::write(
            bundle.join("process_tree.json"),
            serde_json::to_string_pretty(&json!({
                "root_pid": root_pid,
                "pids": descendants,
            }))
            .map_err(|e| format!("render process tree debug bundle: {e}"))?,
        )
        .map_err(|e| format!("write process tree debug bundle: {e}"))?;
        for file in ["status", "cgroup", "oom_score", "oom_score_adj"] {
            let source = Path::new("/proc").join(root_pid.to_string()).join(file);
            if let Ok(content) = fs::read_to_string(&source) {
                fs::write(bundle.join(format!("root_proc_{file}")), content)
                    .map_err(|e| format!("write root proc debug copy for {file}: {e}"))?;
            }
        }
    }
    Ok(bundle)
}

pub(crate) fn attach_pid_to_cgroup(cgroup: &Path, pid: u32) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    fs::write(cgroup.join("cgroup.procs"), pid.to_string()).map_err(|e| {
        format!(
            "attach pid {pid} to cgroup {} via cgroup.procs: {e}",
            cgroup.display()
        )
    })
}

#[allow(dead_code)]
pub(crate) fn reclaim_cgroup_memory(cgroup: &Path, bytes: u64) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("memory.reclaim is only available on Linux cgroup v2".to_string());
    }
    let path = cgroup.join("memory.reclaim");
    if !path.exists() {
        return Err(format!(
            "memory.reclaim is not supported for cgroup {}",
            cgroup.display()
        ));
    }
    fs::write(&path, bytes.to_string()).map_err(|e| {
        format!(
            "write memory.reclaim={} in {}: {e}",
            bytes,
            cgroup.display()
        )
    })
}

#[allow(dead_code)]
pub(crate) fn kill_cgroup(cgroup: &Path) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("cgroup.kill is only available on Linux cgroup v2".to_string());
    }
    let path = cgroup.join("cgroup.kill");
    if !path.exists() {
        return Err(format!(
            "cgroup.kill is not supported for cgroup {}",
            cgroup.display()
        ));
    }
    fs::write(&path, "1").map_err(|e| format!("write cgroup.kill in {}: {e}", cgroup.display()))
}

fn monitor_once(cgroup: &Path, expected_relative: &str, root_pid: u32) -> serde_json::Value {
    let mut reattached = Vec::new();
    let mut membership_errors = Vec::new();
    let pids = process_tree_pids(root_pid);
    for pid in &pids {
        match process_cgroup_membership(*pid, expected_relative) {
            CgroupMembership::InGroup | CgroupMembership::Gone => continue,
            CgroupMembership::OutOfGroup => {}
            CgroupMembership::Error(err) => {
                membership_errors.push(json!({ "pid": pid, "error": err }));
                continue;
            }
        }
        match attach_pid_to_cgroup(cgroup, *pid) {
            Ok(()) => reattached.push(*pid),
            Err(_) if !proc_pid_exists(*pid) => continue,
            Err(err) => membership_errors.push(json!({ "pid": pid, "error": err })),
        }
    }
    let sample = sample_cgroup(cgroup).unwrap_or_else(|err| json!({ "sample_error": err }));
    let oom = memory_event_count(&sample, "oom");
    let oom_kill = memory_event_count(&sample, "oom_kill");
    let terminal_oom = oom > 0 || oom_kill > 0;
    let failure = if !membership_errors.is_empty() {
        Some("memory cgroup descendant attachment failed")
    } else if terminal_oom {
        Some("memory cgroup reported oom or oom_kill")
    } else {
        None
    };
    json!({
        "event": "memory.monitor.sample",
        "root_pid": root_pid,
        "pids": pids,
        "reattached_pids": reattached,
        "membership_errors": membership_errors,
        "failure": failure,
        "sample": sample,
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct MemoryPressureRawSample {
    memory_current_bytes: Option<u64>,
    memory_peak_bytes: Option<u64>,
    high: u64,
    oom: u64,
    oom_kill: u64,
    psi_some_total_us: u64,
    psi_full_total_us: u64,
}

impl MemoryPressureRawSample {
    fn from_monitor_event(event: &serde_json::Value) -> Option<Self> {
        let sample = event.get("sample")?;
        Some(Self {
            memory_current_bytes: sample
                .get("memory_current_bytes")
                .and_then(serde_json::Value::as_u64),
            memory_peak_bytes: sample
                .get("memory_peak_bytes")
                .and_then(serde_json::Value::as_u64),
            high: memory_event_count(sample, "high"),
            oom: memory_event_count(sample, "oom"),
            oom_kill: memory_event_count(sample, "oom_kill"),
            psi_some_total_us: sample
                .get("memory_pressure")
                .and_then(serde_json::Value::as_str)
                .and_then(|pressure| psi_total_us(pressure, "some"))
                .unwrap_or(0),
            psi_full_total_us: sample
                .get("memory_pressure")
                .and_then(serde_json::Value::as_str)
                .and_then(|pressure| psi_total_us(pressure, "full"))
                .unwrap_or(0),
        })
    }

    fn delta(self, previous: Option<&Self>) -> MemoryPressureSnapshot {
        MemoryPressureSnapshot {
            memory_current_bytes: self.memory_current_bytes,
            memory_peak_bytes: self.memory_peak_bytes,
            memory_events_high_delta: saturating_delta(
                self.high,
                previous.map(|sample| sample.high),
            ),
            memory_events_oom_delta: saturating_delta(self.oom, previous.map(|sample| sample.oom)),
            memory_events_oom_kill_delta: saturating_delta(
                self.oom_kill,
                previous.map(|sample| sample.oom_kill),
            ),
            psi_some_total_delta_us: saturating_delta(
                self.psi_some_total_us,
                previous.map(|sample| sample.psi_some_total_us),
            ),
            psi_full_total_delta_us: saturating_delta(
                self.psi_full_total_us,
                previous.map(|sample| sample.psi_full_total_us),
            ),
            sampled_at_unix_ms: unix_ms(),
            ..MemoryPressureSnapshot::default()
        }
    }
}

fn saturating_delta(current: u64, previous: Option<u64>) -> u64 {
    previous.map_or(0, |previous| current.saturating_sub(previous))
}

fn psi_total_us(pressure: &str, prefix: &str) -> Option<u64> {
    let line = pressure
        .lines()
        .find(|line| line.split_whitespace().next() == Some(prefix))?;
    line.split_whitespace()
        .find_map(|part| part.strip_prefix("total="))
        .and_then(|value| value.parse::<u64>().ok())
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn memory_action_payload(action: &MemoryAction) -> serde_json::Value {
    match action {
        MemoryAction::CancelReadOnlyHelper {
            agent_run_id,
            reason,
        } => json!({
            "action": action.as_str(),
            "agent_run_id": agent_run_id.as_str(),
            "reason": reason,
        }),
        MemoryAction::Reclaim { group_id, bytes } => json!({
            "action": action.as_str(),
            "group_id": group_id.as_str(),
            "bytes": bytes,
        }),
        MemoryAction::KillGroup {
            group_id,
            terminal_cleanup,
        } => json!({
            "action": action.as_str(),
            "group_id": group_id.as_str(),
            "terminal_cleanup": terminal_cleanup,
        }),
        MemoryAction::FailRun { reason } => json!({
            "action": action.as_str(),
            "reason": reason,
        }),
        _ => json!({ "action": action.as_str() }),
    }
}

fn helper_candidate_payload(helper: &HelperMemoryCandidate) -> serde_json::Value {
    json!({
        "agent_run_id": helper.agent_run_id.as_str(),
        "read_only": helper.read_only,
        "priority": helper.priority,
        "memory_pressure_score": helper.memory_pressure_score,
        "started_at_unix_ms": helper.started_at_unix_ms,
    })
}

fn emit_memory_governor_trace_event(
    context: &MemoryTraceContext,
    monitor_event: &serde_json::Value,
    transition: &MemoryPressureTransition,
    action_results: &[serde_json::Value],
    helpers: &[HelperMemoryCandidate],
    spawn_paused: bool,
) -> Result<(), String> {
    let ts_unix_ms = current_epoch_millis();
    let run_id = monitor_event
        .get("run_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    let agent_run_id = monitor_event
        .get("agent_run_id")
        .and_then(serde_json::Value::as_str);
    let event = json!({
        "schema_version": "0.1",
        "ts": iso_timestamp_from_epoch_millis(ts_unix_ms),
        "ts_unix_ms": ts_unix_ms,
        "run_id": run_id,
        "issue_id": format!("github:{}#{}", context.repo, context.issue),
        "agent_run_id": agent_run_id,
        "parent_agent_run_id": serde_json::Value::Null,
        "event_type": "memory.governor.transition",
        "span_id": agent_run_id
            .map(|agent| format!("span:{run_id}:{agent}:memory.governor"))
            .unwrap_or_else(|| format!("span:{run_id}:memory.governor")),
        "parent_span_id": serde_json::Value::Null,
        "payload": {
            "previous": transition.previous.as_str(),
            "next": transition.next.as_str(),
            "reason": transition.reason.as_str(),
            "actions": transition.actions.iter().map(memory_action_payload).collect::<Vec<_>>(),
            "action_results": action_results,
            "helper_candidates": helpers.iter().map(helper_candidate_payload).collect::<Vec<_>>(),
            "spawn_paused": spawn_paused,
            "cgroup": monitor_event.get("cgroup").cloned().unwrap_or(serde_json::Value::Null),
            "root_pid": monitor_event.get("root_pid").cloned().unwrap_or(serde_json::Value::Null),
        },
    });
    append_jsonl(&context.trace_path, &event)
}

fn execute_memory_actions(
    actions: &[MemoryAction],
    registry: Option<&ActiveMemoryRegistry>,
    process_supervisor: Option<&dyn RuntimeProcessSupervisor>,
    failure: &Arc<Mutex<Option<String>>>,
) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    for action in actions {
        match action {
            MemoryAction::CancelReadOnlyHelper {
                agent_run_id,
                reason,
            } => {
                let Some(registry) = registry else {
                    results.push(json!({
                        "action": action.as_str(),
                        "agent_run_id": agent_run_id.as_str(),
                        "supported": false,
                        "succeeded": false,
                        "detail": "no active memory registry is available for helper cancellation",
                    }));
                    continue;
                };
                let Some(target) = registry.mark_cancelled(agent_run_id) else {
                    results.push(json!({
                        "action": action.as_str(),
                        "agent_run_id": agent_run_id.as_str(),
                        "supported": true,
                        "succeeded": false,
                        "detail": "helper was already cancelled or is no longer active",
                    }));
                    continue;
                };
                let supervisor_result = process_supervisor
                    .ok_or_else(|| {
                        "no runtime process supervisor is available for helper cancellation"
                            .to_string()
                    })
                    .and_then(|supervisor| {
                        supervisor.cancel_process_tree(&target.runtime_event, reason)
                    });
                let outcome = match supervisor_result {
                    Ok(detail) => json!({
                        "action": action.as_str(),
                        "agent_run_id": agent_run_id.as_str(),
                        "group_id": target.group_id.as_ref().map(MemoryGroupId::as_str),
                        "supported": true,
                        "succeeded": true,
                        "process_supervisor": {
                            "attempted": true,
                            "succeeded": true,
                            "detail": detail,
                        },
                        "cgroup_kill": {
                            "attempted": false,
                            "detail": "process supervisor cancellation completed before cgroup.kill fallback",
                        },
                        "detail": format!("cancelled read-only helper due to {reason}"),
                    }),
                    Err(supervisor_err) => {
                        let cgroup_outcome = match kill_cgroup(&target.cgroup) {
                            Ok(()) => json!({
                                "attempted": true,
                                "supported": true,
                                "succeeded": true,
                                "detail": "cgroup.kill fallback completed after process supervisor cancellation failed",
                            }),
                            Err(err) => json!({
                                "attempted": true,
                                "supported": false,
                                "succeeded": false,
                                "detail": err,
                            }),
                        };
                        let succeeded = cgroup_outcome
                            .get("succeeded")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        json!({
                            "action": action.as_str(),
                            "agent_run_id": agent_run_id.as_str(),
                            "group_id": target.group_id.as_ref().map(MemoryGroupId::as_str),
                            "supported": succeeded,
                            "succeeded": succeeded,
                            "process_supervisor": {
                                "attempted": true,
                                "succeeded": false,
                                "detail": supervisor_err,
                            },
                            "cgroup_kill": cgroup_outcome,
                            "detail": if succeeded {
                                format!("cancelled read-only helper through cgroup.kill fallback due to {reason}")
                            } else {
                                format!("failed to cancel read-only helper due to {reason}")
                            },
                        })
                    }
                };
                results.push(outcome);
            }
            MemoryAction::FailRun { reason } => {
                if let Ok(mut failure) = failure.lock() {
                    if failure.is_none() {
                        *failure = Some(reason.clone());
                    }
                }
                results.push(json!({
                    "action": action.as_str(),
                    "supported": true,
                    "succeeded": true,
                    "detail": reason,
                }));
            }
            MemoryAction::PauseReadOnlySpawns
            | MemoryAction::ReleaseSpawnPause
            | MemoryAction::RequestRuntimeMitigation(_)
            | MemoryAction::Observe => {
                results.push(json!({
                    "action": action.as_str(),
                    "supported": true,
                    "succeeded": true,
                    "detail": "governor state recorded; runtime has no live spawn queue for this action",
                }));
            }
            MemoryAction::Reclaim { group_id, bytes } => {
                results.push(json!({
                    "action": action.as_str(),
                    "group_id": group_id.as_str(),
                    "bytes": bytes,
                    "supported": false,
                    "succeeded": false,
                    "detail": "memory.reclaim is available as a Linux primitive but disabled by the default governor policy",
                }));
            }
            MemoryAction::KillGroup {
                group_id,
                terminal_cleanup,
            } => {
                let Some(registry) = registry else {
                    results.push(json!({
                        "action": action.as_str(),
                        "group_id": group_id.as_str(),
                        "terminal_cleanup": terminal_cleanup,
                        "supported": false,
                        "succeeded": false,
                        "detail": "no active memory registry is available for cgroup cleanup",
                    }));
                    continue;
                };
                let Some(target) = registry.target_by_group_id(group_id) else {
                    results.push(json!({
                        "action": action.as_str(),
                        "group_id": group_id.as_str(),
                        "terminal_cleanup": terminal_cleanup,
                        "supported": false,
                        "succeeded": false,
                        "detail": "no active runtime target was registered for this memory group",
                    }));
                    continue;
                };
                let supervisor_result = process_supervisor
                    .ok_or_else(|| {
                        "no runtime process supervisor is available before cgroup.kill cleanup"
                            .to_string()
                    })
                    .and_then(|supervisor| {
                        supervisor.cancel_process_tree(
                            &target.runtime_event,
                            if *terminal_cleanup {
                                "terminal memory cleanup"
                            } else {
                                "memory group cleanup"
                            },
                        )
                    });
                let cgroup_outcome = match kill_cgroup(&target.cgroup) {
                    Ok(()) => json!({
                        "attempted": true,
                        "supported": true,
                        "succeeded": true,
                        "detail": "cgroup.kill final primitive completed",
                    }),
                    Err(err) => json!({
                        "attempted": true,
                        "supported": false,
                        "succeeded": false,
                        "detail": err,
                    }),
                };
                let cgroup_succeeded = cgroup_outcome
                    .get("succeeded")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let (supervisor_attempted, supervisor_succeeded, supervisor_detail) =
                    match supervisor_result {
                        Ok(detail) => (true, true, detail),
                        Err(err) => (true, false, err),
                    };
                results.push(json!({
                    "action": action.as_str(),
                    "group_id": group_id.as_str(),
                    "terminal_cleanup": terminal_cleanup,
                    "supported": supervisor_succeeded || cgroup_succeeded,
                    "succeeded": supervisor_succeeded || cgroup_succeeded,
                    "process_supervisor": {
                        "attempted": supervisor_attempted,
                        "succeeded": supervisor_succeeded,
                        "detail": supervisor_detail,
                    },
                    "cgroup_kill": cgroup_outcome,
                    "detail": "governor-decided cleanup attempted process supervisor before cgroup.kill final primitive",
                }));
            }
        }
    }
    results
}

fn memory_event_count(sample: &serde_json::Value, key: &str) -> u64 {
    sample
        .get("memory_events")
        .and_then(|events| events.get(key))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

fn expected_cgroup_relative_path(cgroup: &Path) -> Result<String, String> {
    let relative = cgroup.strip_prefix("/sys/fs/cgroup").map_err(|_| {
        format!(
            "memory cgroup path {} is outside /sys/fs/cgroup; strict membership validation requires a canonical cgroup v2 path",
            cgroup.display()
        )
    })?;
    Ok(format!("/{}", relative.display()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CgroupMembership {
    InGroup,
    OutOfGroup,
    Gone,
    Error(String),
}

fn process_cgroup_membership(pid: u32, expected_relative: &str) -> CgroupMembership {
    let path = Path::new("/proc").join(pid.to_string()).join("cgroup");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return CgroupMembership::Gone,
        Err(err) => {
            return CgroupMembership::Error(format!(
                "read cgroup membership for pid {pid} at {}: {err}",
                path.display()
            ))
        }
    };
    if content.lines().any(|line| {
        line.rsplit_once(':')
            .map(|(_, membership)| membership == expected_relative)
            .unwrap_or(false)
    }) {
        CgroupMembership::InGroup
    } else {
        CgroupMembership::OutOfGroup
    }
}

fn process_tree_pids(root_pid: u32) -> Vec<u32> {
    if !proc_pid_exists(root_pid) {
        return Vec::new();
    }
    let mut pids = vec![root_pid];
    let mut cursor = 0;
    while cursor < pids.len() {
        let pid = pids[cursor];
        cursor += 1;
        for child in child_pids(pid) {
            if !pids.contains(&child) {
                pids.push(child);
            }
        }
    }
    pids
}

fn proc_pid_exists(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

fn child_pids(pid: u32) -> Vec<u32> {
    let task_dir = Path::new("/proc").join(pid.to_string()).join("task");
    let Ok(tasks) = fs::read_dir(task_dir) else {
        return Vec::new();
    };
    let mut children = Vec::new();
    for task in tasks.flatten() {
        let children_path = task.path().join("children");
        let Ok(content) = fs::read_to_string(children_path) else {
            continue;
        };
        for child in content.split_whitespace() {
            if let Ok(pid) = child.parse::<u32>() {
                if !children.contains(&pid) {
                    children.push(pid);
                }
            }
        }
    }
    children
}

fn append_jsonl(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    writeln!(file, "{value}").map_err(|e| format!("write {}: {e}", path.display()))
}

fn enable_memory_controller(cgroup: &Path) -> Result<(), String> {
    let controllers = cgroup.join("cgroup.controllers");
    if !controllers.exists() {
        return Ok(());
    }
    let available = fs::read_to_string(&controllers)
        .map_err(|e| format!("read {}: {e}", controllers.display()))?;
    if !available
        .split_whitespace()
        .any(|controller| controller == "memory")
    {
        return Err(format!(
            "memory controller is not available in {}",
            controllers.display()
        ));
    }
    let subtree = cgroup.join("cgroup.subtree_control");
    if subtree.exists() {
        fs::write(&subtree, "+memory")
            .map_err(|e| format!("enable memory controller in {}: {e}", subtree.display()))?;
    }
    Ok(())
}

fn write_cgroup_value(cgroup: &Path, file: &str, value: u64) -> Result<(), String> {
    let rendered = if value == u64::MAX {
        "max".to_string()
    } else {
        value.to_string()
    };
    fs::write(cgroup.join(file), rendered)
        .map_err(|e| format!("write {} in {}: {e}", file, cgroup.display()))
}

fn sample_cgroup(cgroup: &Path) -> Result<serde_json::Value, String> {
    let memory_current = read_optional_u64(&cgroup.join("memory.current"))?;
    let memory_peak = read_optional_u64(&cgroup.join("memory.peak"))?;
    let events = read_key_value_u64(&cgroup.join("memory.events"))?;
    let pressure = fs::read_to_string(cgroup.join("memory.pressure")).ok();
    let process_count = fs::read_to_string(cgroup.join("cgroup.procs"))
        .map(|value| value.lines().filter(|line| !line.trim().is_empty()).count())
        .ok();
    Ok(json!({
        "memory_current_bytes": memory_current,
        "memory_peak_bytes": memory_peak,
        "memory_events": events,
        "memory_pressure": pressure,
        "process_count": process_count,
    }))
}

fn read_optional_u64(path: &Path) -> Result<Option<u64>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let value = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    value
        .trim()
        .parse::<u64>()
        .map(Some)
        .map_err(|e| format!("parse {} as u64: {e}", path.display()))
}

fn read_key_value_u64(path: &Path) -> Result<serde_json::Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let content = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut object = serde_json::Map::new();
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };
        let Some(value) = parts.next() else {
            continue;
        };
        let value = value
            .parse::<u64>()
            .map_err(|e| format!("parse {} entry `{line}`: {e}", path.display()))?;
        object.insert(key.to_string(), json!(value));
    }
    Ok(serde_json::Value::Object(object))
}

fn parse_memory_bytes(value: &str) -> Result<u64, String> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("max") {
        return Ok(u64::MAX);
    }
    let split_at = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, suffix) = trimmed.split_at(split_at);
    let number = number
        .parse::<u64>()
        .map_err(|e| format!("parse memory value `{value}`: {e}"))?;
    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024_u64.pow(2),
        "g" | "gb" | "gib" => 1024_u64.pow(3),
        "t" | "tb" | "tib" => 1024_u64.pow(4),
        other => return Err(format!("unsupported memory suffix `{other}` in `{value}`")),
    };
    number
        .checked_mul(multiplier)
        .ok_or_else(|| format!("memory value `{value}` overflows u64"))
}

fn safe_cgroup_name(value: &str) -> Result<String, String> {
    let safe = value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => ch,
            _ => '_',
        })
        .collect::<String>();
    if safe.is_empty() || safe == "." || safe == ".." {
        Err(format!("invalid cgroup name derived from `{value}`"))
    } else {
        Ok(safe)
    }
}

fn agent_memory_group_id(run_id: &str, agent_run_id: &str) -> MemoryGroupId {
    MemoryGroupId::new(format!("run:{run_id}:agent:{agent_run_id}"))
}

fn configured_cgroup_root(config: &LinuxMemoryConfig) -> PathBuf {
    if config.cgroup_root.trim().is_empty() || config.cgroup_root == "auto" {
        let root_group = config.root_group.trim();
        if root_group.is_empty() || root_group == "agentactr" {
            return PathBuf::from(DEFAULT_CGROUP_ROOT);
        }
        return PathBuf::from("/sys/fs/cgroup")
            .join(format!("{}.slice", safe_group_segment(root_group)));
    }
    PathBuf::from(&config.cgroup_root)
}

fn safe_group_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn available_text(value: bool) -> &'static str {
    if value {
        "available"
    } else {
        "missing"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn parses_binary_memory_units() {
        assert_eq!(parse_memory_bytes("1K").unwrap(), 1024);
        assert_eq!(parse_memory_bytes("6G").unwrap(), 6 * 1024_u64.pow(3));
        assert_eq!(parse_memory_bytes("8GiB").unwrap(), 8 * 1024_u64.pow(3));
        assert_eq!(parse_memory_bytes("max").unwrap(), u64::MAX);
    }

    #[test]
    fn rejects_invalid_memory_suffixes() {
        let err = parse_memory_bytes("12XB").unwrap_err();
        assert!(err.contains("unsupported memory suffix"));
    }

    #[test]
    fn reads_oom_and_oom_kill_independently() {
        let sample = json!({
            "memory_events": {
                "oom": 0,
                "oom_kill": 2
            }
        });

        assert_eq!(memory_event_count(&sample, "oom"), 0);
        assert_eq!(memory_event_count(&sample, "oom_kill"), 2);
    }

    #[test]
    fn parses_psi_totals_for_governor_snapshots() {
        let pressure = "some avg10=0.00 avg60=0.00 avg300=0.00 total=42\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=7\n";

        assert_eq!(psi_total_us(pressure, "some"), Some(42));
        assert_eq!(psi_total_us(pressure, "full"), Some(7));
    }

    #[test]
    fn memory_pressure_raw_sample_builds_deltas() {
        let previous = MemoryPressureRawSample {
            high: 3,
            oom: 1,
            oom_kill: 0,
            psi_some_total_us: 50,
            psi_full_total_us: 5,
            ..MemoryPressureRawSample::default()
        };
        let current = MemoryPressureRawSample {
            high: 5,
            oom: 1,
            oom_kill: 1,
            psi_some_total_us: 70,
            psi_full_total_us: 5,
            ..MemoryPressureRawSample::default()
        };

        let snapshot = current.delta(Some(&previous));

        assert_eq!(snapshot.memory_events_high_delta, 2);
        assert_eq!(snapshot.memory_events_oom_delta, 0);
        assert_eq!(snapshot.memory_events_oom_kill_delta, 1);
        assert_eq!(snapshot.psi_some_total_delta_us, 20);
        assert_eq!(snapshot.psi_full_total_delta_us, 0);
    }

    struct FakeProcessSupervisor;

    impl RuntimeProcessSupervisor for FakeProcessSupervisor {
        fn start(
            &self,
            _event: &RuntimeProcessEvent,
            _artifact_dir: &Path,
        ) -> Result<Option<Box<dyn agentactr_sdk::RuntimeProcessMonitor>>, String> {
            Ok(None)
        }

        fn preserve_debug_bundle(
            &self,
            _event: Option<&RuntimeProcessEvent>,
            _artifact_dir: &Path,
            _reason: &str,
        ) -> Result<(), String> {
            Ok(())
        }

        fn cancel_process_tree(
            &self,
            _event: &RuntimeProcessEvent,
            reason: &str,
        ) -> Result<String, String> {
            Ok(format!("process supervisor cancelled for {reason}"))
        }
    }

    fn test_runtime_event(agent_run_id: &str) -> RuntimeProcessEvent {
        let attribution = agentactr_sdk::RuntimeProcessAttribution::new(
            agentactr_sdk::RunId::new("run-1"),
            AgentRunId::new(agent_run_id),
            agentactr_sdk::RuntimeKind::new("codex"),
            agentactr_sdk::RuntimeTransportKind::new("cli_json"),
            agentactr_sdk::RuntimeProcessModel::OneShotProcess,
        )
        .with_root_pid(agentactr_sdk::ProcessId(1234))
        .with_process_group_id(agentactr_sdk::ProcessGroupId(1234))
        .with_parent_agent_run_id(AgentRunId::new("agent-parent"))
        .with_memory_group_id(MemoryGroupId::new("memory-child"));
        RuntimeProcessEvent::new(agentactr_sdk::RuntimeProcessEventKind::Started, attribution)
    }

    #[test]
    fn helper_cancellation_uses_process_supervisor_before_cgroup_kill() {
        let registry = ActiveMemoryRegistry::default();
        let agent_run_id = AgentRunId::new("agent-child");
        let runtime_event = test_runtime_event(agent_run_id.as_str());
        registry.register(
            PathBuf::from("/not/a/real/cgroup"),
            &MemoryMonitorSubject {
                group_id: Some(MemoryGroupId::new("memory-child")),
                agent_run_id: agent_run_id.clone(),
                runtime_event,
                read_only_helper: true,
                priority: 10,
            },
        );
        let failure = Arc::new(Mutex::new(None));
        let results = execute_memory_actions(
            &[MemoryAction::CancelReadOnlyHelper {
                agent_run_id,
                reason: "test pressure".to_string(),
            }],
            Some(&registry),
            Some(&FakeProcessSupervisor),
            &failure,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["succeeded"], true);
        assert_eq!(results[0]["process_supervisor"]["succeeded"], true);
        assert_eq!(results[0]["cgroup_kill"]["attempted"], false);
    }

    #[test]
    fn terminal_kill_group_uses_process_supervisor_before_cgroup_final_primitive() {
        let registry = ActiveMemoryRegistry::default();
        let agent_run_id = AgentRunId::new("agent-writer");
        let runtime_event = test_runtime_event(agent_run_id.as_str());
        registry.register(
            PathBuf::from("/not/a/real/cgroup"),
            &MemoryMonitorSubject {
                group_id: Some(MemoryGroupId::new("memory-child")),
                agent_run_id,
                runtime_event,
                read_only_helper: false,
                priority: 0,
            },
        );
        let failure = Arc::new(Mutex::new(None));
        let results = execute_memory_actions(
            &[MemoryAction::KillGroup {
                group_id: MemoryGroupId::new("memory-child"),
                terminal_cleanup: true,
            }],
            Some(&registry),
            Some(&FakeProcessSupervisor),
            &failure,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["succeeded"], true);
        assert_eq!(results[0]["process_supervisor"]["succeeded"], true);
        assert_eq!(results[0]["cgroup_kill"]["attempted"], true);
    }

    #[test]
    fn memory_governor_transition_is_written_to_run_trace() {
        let root = std::env::temp_dir().join(format!(
            "agentactr-memory-trace-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let trace_path = root.join("events.jsonl");
        let context = MemoryTraceContext {
            trace_path: trace_path.clone(),
            repo: "OWNER/REPO".to_string(),
            issue: "42".to_string(),
        };
        let transition = MemoryPressureTransition::new(
            agentactr_sdk::MemoryPressureState::PressureObserved,
            agentactr_sdk::MemoryPressureState::PressureSustained,
            vec![MemoryAction::PauseReadOnlySpawns],
            "test sustained pressure",
        );

        emit_memory_governor_trace_event(
            &context,
            &json!({
                "run_id": "run-1",
                "agent_run_id": "agent-1",
                "root_pid": 1234,
                "cgroup": "/sys/fs/cgroup/agentactr/run-1/agent-1",
            }),
            &transition,
            &[json!({"action":"pause_read_only_spawns","succeeded":true})],
            &[],
            true,
        )
        .unwrap();

        let trace = fs::read_to_string(&trace_path).unwrap();
        assert!(trace.contains(r#""event_type":"memory.governor.transition""#));
        assert!(trace.contains(r#""next":"pressure_sustained""#));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn monitor_ignores_exited_or_missing_root_pid() {
        let event = monitor_once(
            Path::new("/tmp/agentactr-missing-cgroup"),
            "/agentactr/missing",
            u32::MAX,
        );

        assert_eq!(event["pids"].as_array().unwrap().len(), 0);
        assert_eq!(event["membership_errors"].as_array().unwrap().len(), 0);
        assert!(event["failure"].is_null());
    }

    #[test]
    fn missing_proc_cgroup_is_reported_as_gone_membership() {
        let membership = process_cgroup_membership(u32::MAX, "/agentactr/missing");

        assert_eq!(membership, CgroupMembership::Gone);
    }

    #[test]
    fn cgroup_membership_path_must_be_canonical_cgroup_v2_path() {
        let err =
            expected_cgroup_relative_path(Path::new("/tmp/agentactr/delegated/run")).unwrap_err();

        assert!(err.contains("outside /sys/fs/cgroup"));
    }

    #[test]
    fn observe_only_preparation_writes_status_artifact_without_cgroups() {
        let root = env::temp_dir().join(format!(
            "agentactr-memory-observe-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut config = agentactr_sdk::AgentactrConfig::strict_defaults("OWNER/REPO").linux_memory;
        config.mode = "observe_only".to_string();
        let controller = LinuxMemoryController::new(&config);

        let context = controller
            .prepare_run("run-1", "agent-run-1", &root)
            .unwrap();

        assert!(!context.enforce);
        assert!(context.run_group.is_none());
        assert!(context.agent_group.is_none());
        assert!(root.join("memory_status.json").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn agent_memory_group_ids_are_per_child_not_shared() {
        assert_eq!(
            agent_memory_group_id("run-1", "agent-implementer").as_str(),
            "run:run-1:agent:agent-implementer"
        );
        assert_ne!(
            agent_memory_group_id("run-1", "agent-implementer"),
            agent_memory_group_id("run-1", "agent-reviewer")
        );
    }

    #[test]
    fn observe_only_child_agent_preparation_returns_no_lease() {
        let root = env::temp_dir().join(format!(
            "agentactr-memory-child-observe-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut config = agentactr_sdk::AgentactrConfig::strict_defaults("OWNER/REPO").linux_memory;
        config.mode = "observe_only".to_string();
        let controller = LinuxMemoryController::new(&config);
        let context = controller
            .prepare_run("run-1", "agent-implementer", &root)
            .unwrap();

        let prepared = controller
            .prepare_child_agent(&context, "run-1", "agent-reviewer", &root.join("child"))
            .unwrap();

        assert!(prepared.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn linux_cgroup_v2_integration_uses_delegated_root_when_opted_in() {
        if env::var("AGENTACTR_LINUX_CGROUP_IT").as_deref() != Ok("1") {
            return;
        }
        if !is_linux_test_host() {
            panic!("AGENTACTR_LINUX_CGROUP_IT=1 requires a Linux host");
        }
        let cgroup_root = env::var("AGENTACTR_CGROUP_ROOT")
            .expect("AGENTACTR_CGROUP_ROOT must point to a writable delegated cgroup v2 root");
        let root =
            env::temp_dir().join(format!("agentactr-memory-cgroup-it-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut config = agentactr_sdk::AgentactrConfig::strict_defaults("OWNER/REPO").linux_memory;
        config.mode = "enforce_on_linux_observe_elsewhere".to_string();
        config.psi_required = false;
        let controller = LinuxMemoryController {
            config,
            cgroup_root: PathBuf::from(cgroup_root),
        };
        let context = controller
            .prepare_run("run-it", "agent-run-it", &root)
            .unwrap();
        let agent_group = context.agent_group.as_ref().unwrap();

        assert!(agent_group.join("memory.high").exists());
        assert!(agent_group.join("memory.max").exists());
        assert!(root.join("memory_status.json").exists());

        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 0.2 & wait")
            .spawn()
            .unwrap();
        attach_pid_to_cgroup(agent_group, child.id()).unwrap();
        let monitor = start_memory_monitor(agent_group, child.id(), &root, None, None, None, None)
            .unwrap()
            .expect("Linux memory monitor should start");
        let status = child.wait().unwrap();
        assert!(status.success());
        monitor.stop().unwrap();
        assert!(root.join("memory_monitor.jsonl").exists());
        let _ = preserve_memory_debug_bundle(&root, Some(agent_group), None, "integration test")
            .unwrap();
        assert!(root.join("memory_debug_bundle/reason.txt").exists());
        let _ = fs::remove_dir_all(root);
    }

    fn is_linux_test_host() -> bool {
        cfg!(target_os = "linux")
    }
}
