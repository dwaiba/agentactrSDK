use agentactr_execution::{
    docker_command, resolve_execution_backend, ExecutionBackend, ProcessCommandSpec,
};
use agentactr_sdk::{
    AdapterVersionReport, AgentIssueRunRequest, AgentNode, AgentRunReport, AgentRuntime,
    AgentRuntimeCapabilities, AgentSession, AgentStartRequest, AgentTurnRequest, AgentTurnStream,
    CancelReason, CodexConfig, CodexMode, ExecutionConfig, Issue, LinuxMemoryConfig, MemoryGroupId,
    ProcessGroupId, ProcessId, RunId, RuntimeKind, RuntimeProcessAttribution, RuntimeProcessEvent,
    RuntimeProcessEventKind, RuntimeProcessModel, RuntimeProcessMonitor, RuntimeProcessSupervisor,
    RuntimeTransportKind, SpawnPlan, WriteScope,
};
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub use agentactr_sdk::{
    RuntimeProcessMonitor as CodexMemoryMonitor, RuntimeProcessSupervisor as CodexMemorySupervisor,
};

#[derive(Default)]
pub struct NoopCodexMemorySupervisor;

impl RuntimeProcessSupervisor for NoopCodexMemorySupervisor {
    fn start(
        &self,
        event: &RuntimeProcessEvent,
        _artifact_dir: &Path,
    ) -> Result<Option<Box<dyn RuntimeProcessMonitor>>, String> {
        Err(format!(
            "memory group {} was configured, but no RuntimeProcessSupervisor was injected",
            event
                .attribution
                .memory_group_id
                .as_ref()
                .map(MemoryGroupId::as_str)
                .unwrap_or("<unknown>")
        ))
    }

    fn preserve_debug_bundle(
        &self,
        _event: Option<&RuntimeProcessEvent>,
        _artifact_dir: &Path,
        _reason: &str,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct CodexRuntimeAdapter {
    transport: CodexRuntimeTransport,
}

#[derive(Clone)]
enum CodexRuntimeTransport {
    CliJson(Box<CodexCliAdapter>),
    AppServer(CodexAppServerAdapter),
    Sdk(CodexSdkAdapter),
}

impl CodexRuntimeAdapter {
    pub fn new(config: &CodexConfig) -> Result<Self, String> {
        config.validate_milestone_policy()?;
        let memory_supervisor: Arc<dyn CodexMemorySupervisor> = Arc::new(NoopCodexMemorySupervisor);
        let transport = match CodexMode::parse(&config.mode)? {
            CodexMode::CliJsonExec => CodexRuntimeTransport::CliJson(Box::new(
                CodexCliAdapter::new(config, memory_supervisor),
            )),
            CodexMode::AppServer => {
                CodexRuntimeTransport::AppServer(CodexAppServerAdapter::new(config))
            }
            CodexMode::CodexSdk => CodexRuntimeTransport::Sdk(CodexSdkAdapter::new(config)),
        };
        Ok(Self { transport })
    }

    pub fn with_process_supervisor(
        self,
        memory_supervisor: Arc<dyn CodexMemorySupervisor>,
    ) -> Self {
        let transport = match self.transport {
            CodexRuntimeTransport::CliJson(adapter) => CodexRuntimeTransport::CliJson(Box::new(
                (*adapter).with_memory_supervisor(memory_supervisor),
            )),
            CodexRuntimeTransport::AppServer(adapter) => {
                CodexRuntimeTransport::AppServer(adapter.with_memory_supervisor(memory_supervisor))
            }
            CodexRuntimeTransport::Sdk(adapter) => {
                CodexRuntimeTransport::Sdk(adapter.with_memory_supervisor(memory_supervisor))
            }
        };
        Self { transport }
    }

    pub fn with_memory_supervisor(self, memory_supervisor: Arc<dyn CodexMemorySupervisor>) -> Self {
        self.with_process_supervisor(memory_supervisor)
    }

    pub fn with_process_execution(
        self,
        execution: ExecutionConfig,
        memory: LinuxMemoryConfig,
    ) -> Self {
        let transport = match self.transport {
            CodexRuntimeTransport::CliJson(adapter) => CodexRuntimeTransport::CliJson(Box::new(
                (*adapter).with_process_execution(execution, memory),
            )),
            CodexRuntimeTransport::AppServer(adapter) => {
                CodexRuntimeTransport::AppServer(adapter.with_process_execution(execution, memory))
            }
            CodexRuntimeTransport::Sdk(adapter) => {
                CodexRuntimeTransport::Sdk(adapter.with_process_execution(execution, memory))
            }
        };
        Self { transport }
    }
}

impl AgentRuntime for CodexRuntimeAdapter {
    fn version_report(&self) -> AdapterVersionReport {
        match &self.transport {
            CodexRuntimeTransport::CliJson(adapter) => adapter.version_report(),
            CodexRuntimeTransport::AppServer(adapter) => adapter.version_report(),
            CodexRuntimeTransport::Sdk(adapter) => adapter.version_report(),
        }
    }

    fn capabilities(&self) -> AgentRuntimeCapabilities {
        match &self.transport {
            CodexRuntimeTransport::CliJson(adapter) => adapter.capabilities(),
            CodexRuntimeTransport::AppServer(adapter) => adapter.capabilities(),
            CodexRuntimeTransport::Sdk(adapter) => adapter.capabilities(),
        }
    }

    fn start(&self, req: AgentStartRequest) -> Result<AgentSession, String> {
        match &self.transport {
            CodexRuntimeTransport::CliJson(adapter) => adapter.start(req),
            CodexRuntimeTransport::AppServer(adapter) => adapter.start(req),
            CodexRuntimeTransport::Sdk(adapter) => adapter.start(req),
        }
    }

    fn run_turn(&self, req: AgentTurnRequest) -> Result<AgentTurnStream, String> {
        match &self.transport {
            CodexRuntimeTransport::CliJson(adapter) => adapter.run_turn(req),
            CodexRuntimeTransport::AppServer(adapter) => adapter.run_turn(req),
            CodexRuntimeTransport::Sdk(adapter) => adapter.run_turn(req),
        }
    }

    fn run_issue(&self, req: AgentIssueRunRequest) -> Result<AgentRunReport, String> {
        match &self.transport {
            CodexRuntimeTransport::CliJson(adapter) => adapter.run_issue(req),
            CodexRuntimeTransport::AppServer(adapter) => adapter.run_issue(req),
            CodexRuntimeTransport::Sdk(adapter) => adapter.run_issue(req),
        }
    }

    fn cancel(&self, session_id: &str, reason: CancelReason) -> Result<(), String> {
        match &self.transport {
            CodexRuntimeTransport::CliJson(adapter) => adapter.cancel(session_id, reason),
            CodexRuntimeTransport::AppServer(adapter) => adapter.cancel(session_id, reason),
            CodexRuntimeTransport::Sdk(adapter) => adapter.cancel(session_id, reason),
        }
    }
}

#[derive(Clone)]
pub struct CodexAppServerAdapter {
    command: String,
    profile: String,
    sandbox_mode: String,
    api_key_env: String,
    transport: String,
    experimental_api: bool,
    fallback_mode: String,
    memory_supervisor: Arc<dyn CodexMemorySupervisor>,
}

impl CodexAppServerAdapter {
    pub fn new(config: &CodexConfig) -> Self {
        Self {
            command: config.command.clone(),
            profile: config.profile.clone(),
            sandbox_mode: config.sandbox_mode.clone(),
            api_key_env: config.openai_api_key_env.clone(),
            transport: config.app_server_transport.clone(),
            experimental_api: config.app_server_experimental_api,
            fallback_mode: config.fallback_mode.clone(),
            memory_supervisor: Arc::new(NoopCodexMemorySupervisor),
        }
    }

    pub fn with_memory_supervisor(
        mut self,
        memory_supervisor: Arc<dyn CodexMemorySupervisor>,
    ) -> Self {
        self.memory_supervisor = memory_supervisor;
        self
    }

    pub fn with_process_execution(
        self,
        _execution: ExecutionConfig,
        _memory: LinuxMemoryConfig,
    ) -> Self {
        self
    }

    pub fn unsupported_message() -> String {
        "codex.mode = \"app_server\" is configured, but the Codex app-server runtime adapter is not implemented or contract-tested in this bootstrap build; set codex.mode = \"cli_json\" to use the production codex exec --json transport"
            .to_string()
    }
}

impl AgentRuntime for CodexAppServerAdapter {
    fn version_report(&self) -> AdapterVersionReport {
        let warnings = vec![
            "app_server transport is feature-gated and fails closed in this bootstrap build"
                .to_string(),
            "use codex.mode = \"cli_json\" for production runs".to_string(),
            format!(
                "configured profile={}, sandbox={}, api_key_env={}, transport={}, experimental_api={}, fallback_mode={}",
                self.profile,
                self.sandbox_mode,
                self.api_key_env,
                self.transport,
                self.experimental_api,
                self.fallback_mode
            ),
        ];
        AdapterVersionReport {
            adapter_kind: "agent_runtime".to_string(),
            adapter_name: "agentactr-codex-app-server".to_string(),
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            product_name: self.command.clone(),
            product_version: command_output(&self.command, &["--version"])
                .unwrap_or_else(|_| "unknown".to_string()),
            api_version: "codex-app-server".to_string(),
            capability_digest: format!(
                "app-server-selected,transport={},experimental_api={},initialize-pending,thread-start-pending,turn-start-pending,cancel-pending,fail-closed",
                self.transport, self.experimental_api
            ),
            degraded_features: vec![
                "single_shot_issue_run".to_string(),
                "session_start".to_string(),
                "turn_streaming".to_string(),
                "cancellation".to_string(),
            ],
            required_actions: vec![
                "implement stdio JSON-RPC initialize/thread/turn lifecycle".to_string(),
                "add app-server approval, cancellation, memory attribution, and contract tests"
                    .to_string(),
                "keep codex.mode = \"cli_json\" for production runs".to_string(),
            ],
            warnings,
        }
    }

    fn capabilities(&self) -> AgentRuntimeCapabilities {
        AgentRuntimeCapabilities {
            single_shot_issue_run: false,
            session_start: false,
            turn_streaming: false,
            cancellation: false,
            exec_json: false,
            app_server: true,
            codex_sdk: false,
            child_agent_execution: false,
            parallel_read_only_child_agents: false,
        }
    }

    fn start(&self, _req: AgentStartRequest) -> Result<AgentSession, String> {
        Err(Self::unsupported_message())
    }

    fn run_turn(&self, _req: AgentTurnRequest) -> Result<AgentTurnStream, String> {
        Err(Self::unsupported_message())
    }

    fn run_issue(&self, _req: AgentIssueRunRequest) -> Result<AgentRunReport, String> {
        Err(Self::unsupported_message())
    }

    fn cancel(&self, session_id: &str, _reason: CancelReason) -> Result<(), String> {
        Err(format!(
            "{}; cancel requested for session {session_id}",
            Self::unsupported_message()
        ))
    }
}

#[derive(Clone)]
pub struct CodexSdkAdapter {
    command: String,
    profile: String,
    sandbox_mode: String,
    api_key_env: String,
    bridge: String,
    fallback_mode: String,
    memory_supervisor: Arc<dyn CodexMemorySupervisor>,
}

impl CodexSdkAdapter {
    pub fn new(config: &CodexConfig) -> Self {
        Self {
            command: config.command.clone(),
            profile: config.profile.clone(),
            sandbox_mode: config.sandbox_mode.clone(),
            api_key_env: config.openai_api_key_env.clone(),
            bridge: config.sdk_bridge.clone(),
            fallback_mode: config.fallback_mode.clone(),
            memory_supervisor: Arc::new(NoopCodexMemorySupervisor),
        }
    }

    pub fn with_memory_supervisor(
        mut self,
        memory_supervisor: Arc<dyn CodexMemorySupervisor>,
    ) -> Self {
        self.memory_supervisor = memory_supervisor;
        self
    }

    pub fn with_process_execution(
        self,
        _execution: ExecutionConfig,
        _memory: LinuxMemoryConfig,
    ) -> Self {
        self
    }

    pub fn unsupported_message() -> String {
        "codex.mode = \"codex_sdk\" is configured, but the TypeScript @openai/codex-sdk sidecar is not implemented or contract-tested in this bootstrap build; set codex.mode = \"cli_json\" to use the production codex exec --json transport"
            .to_string()
    }
}

impl AgentRuntime for CodexSdkAdapter {
    fn version_report(&self) -> AdapterVersionReport {
        let warnings = vec![
            "codex_sdk transport is feature-gated and fails closed in this bootstrap build"
                .to_string(),
            "TypeScript @openai/codex-sdk bridge is pending; use codex.mode = \"cli_json\" for production runs"
                .to_string(),
            format!(
                "configured command={}, profile={}, sandbox={}, api_key_env={}, bridge={}, fallback_mode={}",
                self.command,
                self.profile,
                self.sandbox_mode,
                self.api_key_env,
                self.bridge,
                self.fallback_mode
            ),
        ];
        AdapterVersionReport {
            adapter_kind: "agent_runtime".to_string(),
            adapter_name: "agentactr-codex-sdk".to_string(),
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            product_name: "@openai/codex-sdk".to_string(),
            product_version: "not-probed".to_string(),
            api_version: "codex-sdk-typescript".to_string(),
            capability_digest: format!(
                "codex-sdk-selected,bridge={},typescript-sidecar-pending,node18-required,run-pending,cancel-pending,fail-closed",
                self.bridge
            ),
            degraded_features: vec![
                "single_shot_issue_run".to_string(),
                "session_start".to_string(),
                "turn_streaming".to_string(),
                "cancellation".to_string(),
            ],
            required_actions: vec![
                "implement TypeScript @openai/codex-sdk sidecar bridge".to_string(),
                "add Node.js 18+ preflight, schema drift, approval, cancellation, memory attribution, and contract tests"
                    .to_string(),
                "keep codex.mode = \"cli_json\" for production runs".to_string(),
            ],
            warnings,
        }
    }

    fn capabilities(&self) -> AgentRuntimeCapabilities {
        AgentRuntimeCapabilities {
            single_shot_issue_run: false,
            session_start: false,
            turn_streaming: false,
            cancellation: false,
            exec_json: false,
            app_server: false,
            codex_sdk: true,
            child_agent_execution: false,
            parallel_read_only_child_agents: false,
        }
    }

    fn start(&self, _req: AgentStartRequest) -> Result<AgentSession, String> {
        Err(Self::unsupported_message())
    }

    fn run_turn(&self, _req: AgentTurnRequest) -> Result<AgentTurnStream, String> {
        Err(Self::unsupported_message())
    }

    fn run_issue(&self, _req: AgentIssueRunRequest) -> Result<AgentRunReport, String> {
        Err(Self::unsupported_message())
    }

    fn cancel(&self, session_id: &str, _reason: CancelReason) -> Result<(), String> {
        Err(format!(
            "{}; cancel requested for session {session_id}",
            Self::unsupported_message()
        ))
    }
}

#[derive(Clone)]
pub struct CodexCliAdapter {
    command: String,
    profile: String,
    sandbox_mode: String,
    api_key_env: String,
    timeout: Duration,
    memory_supervisor: Arc<dyn CodexMemorySupervisor>,
    execution: Option<(ExecutionConfig, LinuxMemoryConfig)>,
}

impl CodexCliAdapter {
    pub fn new(config: &CodexConfig, memory_supervisor: Arc<dyn CodexMemorySupervisor>) -> Self {
        Self {
            command: config.command.clone(),
            profile: config.profile.clone(),
            sandbox_mode: config.sandbox_mode.clone(),
            api_key_env: config.openai_api_key_env.clone(),
            timeout: Duration::from_secs(30 * 60),
            memory_supervisor,
            execution: None,
        }
    }

    pub fn with_memory_supervisor(
        mut self,
        memory_supervisor: Arc<dyn CodexMemorySupervisor>,
    ) -> Self {
        self.memory_supervisor = memory_supervisor;
        self
    }

    pub fn with_process_execution(
        mut self,
        execution: ExecutionConfig,
        memory: LinuxMemoryConfig,
    ) -> Self {
        self.execution = Some((execution, memory));
        self
    }

    fn run_issue_request(&self, req: &AgentIssueRunRequest) -> Result<(), String> {
        let child_reports = self.run_child_agents(req)?;
        let issue_context_text = render_issue_context(&req.issue_context);
        let spawn_context = render_spawn_context(req.spawn_plan.as_ref(), &child_reports);
        let prompt = format!(
            r#"You are the implementation agent for agentactr.

Target GitHub issue: {}#{}
Run id: {}
Agent id: {}
Agent role: {}
Agent objective: {}
Write scope: {}
Context manifest: {}
Spawn handoff manifest: {}

Strict defaults:
- Work only in this Git worktree.
- Use the context manifest and agentactr MCP tools as read-only context sources.
- Use spawned read-only helper artifacts as advisory context only; you remain the single writer.
- Honor the configured Codex approval policy: {}.
- Preserve SOLID boundaries and existing project style.
- Run the applicable quality plan before final response.
- If a network-dependent command is required and approval is unavailable, stop retrying and report the blocker with the exact rerun guidance `--human-intervention interactive --codex-approval on-request`.
- If required context or authorization is missing, fail explicitly.

Issue context:
{issue_context_text}

Spawn context:
{spawn_context}
"#,
            req.repo,
            req.issue,
            req.run_id,
            req.agent_run_id,
            req.role,
            req.objective,
            req.write_scope,
            req.context_manifest.display(),
            req.artifact_dir.join("spawn_handoffs.json").display(),
            codex_approval_config_value(req.approval_policy)
        );
        self.run_codex_exec(req, &prompt, &self.sandbox_mode, self.timeout, "codex")?;
        Ok(())
    }

    fn run_child_agents(
        &self,
        req: &AgentIssueRunRequest,
    ) -> Result<Vec<CodexChildRunReport>, String> {
        let Some(plan) = req.spawn_plan.as_ref() else {
            return Ok(Vec::new());
        };
        if plan.child_nodes.is_empty() {
            write_spawn_handoff_manifest(&req.artifact_dir, &[])?;
            return Ok(Vec::new());
        }

        let mut handles = Vec::new();
        for node in &plan.child_nodes {
            ensure_read_only_child(node)?;
            let adapter = self.clone();
            let child_req = child_request_from_node(req, node);
            handles.push(thread::spawn(move || adapter.run_child_agent(child_req)));
        }

        let mut reports = join_child_agent_threads(handles)?;
        reports.sort_by(|left, right| left.agent_run_id.cmp(&right.agent_run_id));
        write_spawn_handoff_manifest(&req.artifact_dir, &reports)?;
        Ok(reports)
    }

    fn run_child_agent(&self, req: AgentIssueRunRequest) -> Result<CodexChildRunReport, String> {
        let prompt = render_child_prompt(&req);
        let prompt_ref = prompt_artifact_ref(&req.artifact_dir, &prompt);
        self.run_codex_exec(
            &req,
            &prompt,
            "read-only",
            self.child_timeout(),
            &format!("codex[{}]", req.role),
        )?;
        let stdout_jsonl = req.artifact_dir.join("codex.stdout.jsonl");
        let stderr_log = req.artifact_dir.join("codex.stderr.log");
        let handoff = req.artifact_dir.join("handoff.md");
        let handoff_body = format!(
            "# {} handoff\n\nagent_run_id: {}\nrole: {}\nstdout_jsonl: {}\nstderr_log: {}\n\nThis read-only helper completed. Treat its artifacts as advisory context for the single writer.\n",
            req.role,
            req.agent_run_id,
            req.role,
            stdout_jsonl.display(),
            stderr_log.display()
        );
        write_file(&handoff, &handoff_body)?;
        Ok(CodexChildRunReport {
            agent_run_id: req.agent_run_id,
            role: req.role,
            artifact_dir: req.artifact_dir,
            prompt_artifact: prompt_ref.prompt_artifact,
            prompt_metadata: prompt_ref.prompt_metadata,
            prompt_sha256: prompt_ref.sha256,
            prompt_bytes: prompt_ref.bytes,
            prompt_chars: prompt_ref.chars,
            handoff,
            handoff_sha256: format!("sha256:{}", sha256_hex(handoff_body.as_bytes())),
            handoff_bytes: handoff_body.len(),
            handoff_chars: handoff_body.chars().count(),
            stdout_jsonl,
            stderr_log,
        })
    }

    fn child_timeout(&self) -> Duration {
        self.timeout.min(Duration::from_secs(10 * 60))
    }

    fn run_codex_exec(
        &self,
        req: &AgentIssueRunRequest,
        prompt: &str,
        sandbox_mode: &str,
        timeout: Duration,
        console_prefix: &str,
    ) -> Result<AgentRunReport, String> {
        fs::create_dir_all(&req.artifact_dir)
            .map_err(|e| format!("create {}: {e}", req.artifact_dir.display()))?;
        write_codex_prompt_artifacts(&req.artifact_dir, prompt)?;
        let approval_override = format!(
            "approval_policy=\"{}\"",
            codex_approval_cli_value(req.approval_policy)
        );
        let mut command = Command::new(&self.command);
        command.arg("exec").arg("--json");
        append_codex_project_profile_overrides(&mut command, &req.worktree, &self.profile)?;
        command
            .arg("--sandbox")
            .arg(sandbox_mode)
            .arg("-c")
            .arg(approval_override)
            .arg("--cd")
            .arg(&req.worktree)
            .arg(prompt)
            .current_dir(&req.worktree)
            .env("AGENTACTR_ARTIFACT_ROOT", &req.artifact_dir)
            .env("AGENTACTR_REPO_ROOT", &req.worktree)
            .env("AGENTACTR_TRACE_PATH", &req.trace_path)
            .env("AGENTACTR_RUN_ID", &req.run_id)
            .env("AGENTACTR_AGENT_RUN_ID", &req.agent_run_id)
            .env("AGENTACTR_CONTEXT_MANIFEST", &req.context_manifest)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        forward_codex_api_key_env(&mut command, &self.api_key_env);
        if self.uses_docker_backend()? {
            let codex_home = prepare_docker_codex_home(req)?;
            command.env("CODEX_HOME", codex_home);
        }
        let command = self.apply_execution_backend(command, req)?;
        let mut command = command;
        configure_process_group(&mut command);
        configure_linux_launch_limits(
            &mut command,
            self.execution.as_ref().map(|(_, memory)| memory),
        )?;
        let mut child = command
            .spawn()
            .map_err(|e| format!("start codex exec --json: {e}"))?;
        let process_attribution = runtime_process_attribution(req, child.id());
        let mut lifecycle = match RuntimeProcessLifecycle::start(
            Arc::clone(&self.memory_supervisor),
            process_attribution,
        ) {
            Ok(lifecycle) => lifecycle,
            Err(err) => {
                terminate_child(&mut child, Duration::from_secs(2));
                return Err(err);
            }
        };
        let started_event = lifecycle.event(RuntimeProcessEventKind::Started);
        let memory_monitor = if req.memory.is_some() {
            match self
                .memory_supervisor
                .start(&started_event, &req.artifact_dir)
            {
                Ok(monitor) => {
                    lifecycle.attributed()?;
                    monitor
                }
                Err(err) => {
                    let _ = self.memory_supervisor.preserve_debug_bundle(
                        Some(&started_event),
                        &req.artifact_dir,
                        &err,
                    );
                    terminate_child(&mut child, Duration::from_secs(2));
                    let _ = lifecycle.terminated();
                    return Err(err);
                }
            }
        } else {
            lifecycle.attributed()?;
            None
        };
        let stdout = child.stdout.take().ok_or("codex stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("codex stderr unavailable")?;
        let stdout_path = req.artifact_dir.join("codex.stdout.jsonl");
        let stderr_path = req.artifact_dir.join("codex.stderr.log");
        let stdout_prefix = console_prefix.to_string();
        let stdout_thread =
            thread::spawn(move || stream_codex_stdout(stdout, stdout_path, stdout_prefix));
        let stderr_thread = thread::spawn(move || stream_process_stderr(stderr, stderr_path));
        let status = match wait_child_with_timeout(&mut child, timeout, memory_monitor.as_deref()) {
            Ok(status) => status,
            Err(err) => {
                if req.memory.is_some() {
                    let _ = self.memory_supervisor.preserve_debug_bundle(
                        Some(&started_event),
                        &req.artifact_dir,
                        &err,
                    );
                }
                if let Some(monitor) = memory_monitor {
                    let _ = monitor.stop();
                }
                let _ = lifecycle.terminated();
                join_codex_streams(stdout_thread, stderr_thread)?;
                return Err(format!("wait codex exec: {err}"));
            }
        };
        if let Some(monitor) = memory_monitor {
            if let Err(err) = monitor.stop() {
                if req.memory.is_some() {
                    let _ = self.memory_supervisor.preserve_debug_bundle(
                        Some(&started_event),
                        &req.artifact_dir,
                        &err,
                    );
                }
                let _ = lifecycle.terminated();
                join_codex_streams(stdout_thread, stderr_thread)?;
                return Err(format!("memory monitor failed closed: {err}"));
            }
        }
        lifecycle.terminated()?;
        join_codex_streams(stdout_thread, stderr_thread)?;
        if !status.success() {
            return Err(format!("codex exec --json exited with {status}"));
        }
        let stdout_path = req.artifact_dir.join("codex.stdout.jsonl");
        if codex_jsonl_has_error_event(&stdout_path)? {
            return Err(format!(
                "codex exec --json emitted an error event; stdout_jsonl={}",
                stdout_path.display()
            ));
        }
        Ok(AgentRunReport {
            stdout_jsonl: req.artifact_dir.join("codex.stdout.jsonl"),
            stderr_log: req.artifact_dir.join("codex.stderr.log"),
        })
    }

    fn apply_execution_backend(
        &self,
        command: Command,
        req: &AgentIssueRunRequest,
    ) -> Result<Command, String> {
        let Some((execution, memory)) = &self.execution else {
            return Ok(command);
        };
        let decision = resolve_execution_backend(execution)?;
        match decision.effective {
            ExecutionBackend::DockerLinuxVm => {
                let spec = command_spec_from_command(command, req)?;
                docker_command(execution, memory, &spec)
            }
            ExecutionBackend::NativeLinuxCgroupV2
            | ExecutionBackend::NativeMacosObserveOnly
            | ExecutionBackend::ObserveOnly => Ok(command),
        }
    }

    fn uses_docker_backend(&self) -> Result<bool, String> {
        let Some((execution, _)) = &self.execution else {
            return Ok(false);
        };
        Ok(resolve_execution_backend(execution)?.effective == ExecutionBackend::DockerLinuxVm)
    }
}

fn join_child_agent_threads(
    handles: Vec<thread::JoinHandle<Result<CodexChildRunReport, String>>>,
) -> Result<Vec<CodexChildRunReport>, String> {
    let mut reports = Vec::new();
    let mut first_error = None;
    for handle in handles {
        match handle.join() {
            Ok(Ok(report)) => reports.push(report),
            Ok(Err(err)) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
            Err(_) => {
                if first_error.is_none() {
                    first_error = Some("Codex child agent thread panicked".to_string());
                }
            }
        }
    }
    if let Some(err) = first_error {
        Err(err)
    } else {
        Ok(reports)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodexChildRunReport {
    agent_run_id: String,
    role: String,
    artifact_dir: PathBuf,
    prompt_artifact: PathBuf,
    prompt_metadata: PathBuf,
    prompt_sha256: String,
    prompt_bytes: usize,
    prompt_chars: usize,
    handoff: PathBuf,
    handoff_sha256: String,
    handoff_bytes: usize,
    handoff_chars: usize,
    stdout_jsonl: PathBuf,
    stderr_log: PathBuf,
}

impl AgentRuntime for CodexCliAdapter {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport {
            adapter_kind: "agent_runtime".to_string(),
            adapter_name: "agentactr-codex-cli".to_string(),
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            product_name: self.command.clone(),
            product_version: command_output(&self.command, &["--version"])
                .unwrap_or_else(|_| "unknown".to_string()),
            api_version: "codex-exec-json".to_string(),
            capability_digest:
                "single-shot-run-issue,exec-json,repo-local-profile-overrides,sandbox,cd,approval-policy-override,cancel-unsupported"
                    .to_string(),
            degraded_features: vec![
                "session_start".to_string(),
                "turn_streaming".to_string(),
                "cancellation".to_string(),
            ],
            required_actions: vec![
                "use app_server or codex_sdk after their adapters pass contract tests for session and cancellation support"
                    .to_string(),
            ],
            warnings: vec![
                "session start, turn streaming, and cancellation are disabled in this milestone"
                    .to_string(),
            ],
        }
    }

    fn capabilities(&self) -> AgentRuntimeCapabilities {
        AgentRuntimeCapabilities {
            single_shot_issue_run: true,
            session_start: false,
            turn_streaming: false,
            cancellation: false,
            exec_json: true,
            app_server: false,
            codex_sdk: false,
            child_agent_execution: true,
            parallel_read_only_child_agents: true,
        }
    }

    fn start(&self, _req: AgentStartRequest) -> Result<AgentSession, String> {
        Err("start is represented by run_issue in this bootstrap adapter".to_string())
    }

    fn run_turn(&self, _req: AgentTurnRequest) -> Result<AgentTurnStream, String> {
        Err("run_turn is not implemented in this milestone".to_string())
    }

    fn run_issue(&self, req: AgentIssueRunRequest) -> Result<AgentRunReport, String> {
        self.run_issue_request(&req)?;
        Ok(AgentRunReport {
            stdout_jsonl: req.artifact_dir.join("codex.stdout.jsonl"),
            stderr_log: req.artifact_dir.join("codex.stderr.log"),
        })
    }

    fn cancel(&self, session_id: &str, _reason: CancelReason) -> Result<(), String> {
        Err(format!(
            "cancel is not implemented for session {session_id} in the single-shot codex exec bootstrap adapter"
        ))
    }
}

pub fn append_codex_project_profile_overrides(
    command: &mut Command,
    worktree: &Path,
    profile: &str,
) -> Result<(), String> {
    for (key, value) in codex_project_profile_overrides(worktree, profile)? {
        command.arg("-c").arg(format!("{key}={value}"));
    }
    Ok(())
}

fn parse_toml_document(content: &str) -> Result<toml::Value, String> {
    toml::from_str::<toml::Table>(content)
        .map(toml::Value::Table)
        .map_err(|e| e.to_string())
}

fn codex_project_profile_overrides(
    worktree: &Path,
    profile: &str,
) -> Result<Vec<(String, String)>, String> {
    let codex_config = worktree.join(".codex").join("config.toml");
    let content = fs::read_to_string(&codex_config)
        .map_err(|e| format!("read Codex project config {}: {e}", codex_config.display()))?;
    let parsed = parse_toml_document(&content)
        .map_err(|e| format!("parse {}: {e}", codex_config.display()))?;
    let root_table = parsed.as_table().ok_or_else(|| {
        format!(
            "Codex project config {} must be a TOML table",
            codex_config.display()
        )
    })?;

    const PROJECT_OVERRIDE_KEYS: &[&str] = &[
        "approval_policy",
        "sandbox_mode",
        "model_reasoning_effort",
        "forced_login_method",
    ];
    let mut overrides = root_table
        .iter()
        .filter(|(key, _)| PROJECT_OVERRIDE_KEYS.contains(&key.as_str()))
        .filter_map(|(key, value)| {
            if matches!(
                value,
                toml::Value::String(_)
                    | toml::Value::Integer(_)
                    | toml::Value::Float(_)
                    | toml::Value::Boolean(_)
                    | toml::Value::Datetime(_)
                    | toml::Value::Array(_)
            ) {
                return Some(Ok((key.clone(), value.to_string())));
            }
            None
        })
        .collect::<Result<Vec<_>, String>>()?;

    if overrides.is_empty() {
        let profile_table = parsed
        .get("profiles")
        .and_then(|profiles| profiles.get(profile))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            format!(
                    "Codex project config {} does not define top-level Codex overrides or legacy profiles.{profile}",
                codex_config.display()
            )
        })?;

        overrides = profile_table
            .iter()
            .map(|(key, value)| {
                if matches!(value, toml::Value::Table(_)) {
                    return Err(format!(
                        "Codex profile {profile} key {key} uses a nested table; repo-local profile overrides support scalar and array values"
                    ));
                }
                Ok((key.clone(), value.to_string()))
            })
            .collect::<Result<Vec<_>, String>>()?;
    }

    overrides.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(overrides)
}

struct RuntimeProcessLifecycle {
    supervisor: Arc<dyn CodexMemorySupervisor>,
    attribution: RuntimeProcessAttribution,
    terminated: bool,
}

impl RuntimeProcessLifecycle {
    fn start(
        supervisor: Arc<dyn CodexMemorySupervisor>,
        attribution: RuntimeProcessAttribution,
    ) -> Result<Self, String> {
        let lifecycle = Self {
            supervisor,
            attribution,
            terminated: false,
        };
        lifecycle
            .supervisor
            .observe(&lifecycle.event(RuntimeProcessEventKind::Started))?;
        Ok(lifecycle)
    }

    fn event(&self, kind: RuntimeProcessEventKind) -> RuntimeProcessEvent {
        RuntimeProcessEvent::new(kind, self.attribution.clone())
    }

    fn attributed(&self) -> Result<(), String> {
        self.supervisor
            .observe(&self.event(RuntimeProcessEventKind::Attributed))
    }

    fn terminated(&mut self) -> Result<(), String> {
        if !self.terminated {
            self.supervisor
                .observe(&self.event(RuntimeProcessEventKind::Terminated))?;
            self.terminated = true;
        }
        Ok(())
    }
}

impl Drop for RuntimeProcessLifecycle {
    fn drop(&mut self) {
        let _ = self.terminated();
    }
}

fn wait_child_with_timeout(
    child: &mut Child,
    timeout: Duration,
    memory_monitor: Option<&dyn CodexMemoryMonitor>,
) -> Result<ExitStatus, String> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("poll child: {e}"))? {
            return Ok(status);
        }
        if let Some(err) = memory_monitor.and_then(|monitor| monitor.failure()) {
            terminate_child(child, Duration::from_secs(2));
            return Err(format!("memory monitor failed closed: {err}"));
        }
        if start.elapsed() >= timeout {
            terminate_child(child, Duration::from_secs(2));
            return Err(format!("timed out after {}s", timeout.as_secs()));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn runtime_process_attribution(
    req: &AgentIssueRunRequest,
    root_pid: u32,
) -> RuntimeProcessAttribution {
    let mut attribution = RuntimeProcessAttribution::new(
        RunId::new(req.run_id.clone()),
        agentactr_sdk::AgentRunId::new(req.agent_run_id.clone()),
        RuntimeKind::new("codex"),
        RuntimeTransportKind::new("cli_json"),
        RuntimeProcessModel::OneShotProcess,
    )
    .with_root_pid(ProcessId(root_pid))
    .with_process_group_id(ProcessGroupId(i64::from(root_pid)));
    if let Some(parent_agent_run_id) = req.parent_agent_run_id.as_ref() {
        attribution = attribution
            .with_parent_agent_run_id(agentactr_sdk::AgentRunId::new(parent_agent_run_id.clone()));
    }
    if let Some(lease) = req.memory.as_ref() {
        attribution = attribution.with_memory_group_id(lease.group_id.clone());
    }
    attribution
}

fn terminate_child(child: &mut Child, grace: Duration) {
    terminate_process_group(child, "TERM");
    if wait_for_process_group_exit(child, grace) {
        return;
    }
    terminate_process_group(child, "KILL");
    let _ = child.kill();
    let _ = wait_for_process_group_exit(child, grace);
    let _ = child.wait();
}

fn wait_for_process_group_exit(child: &mut Child, grace: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < grace {
        let root_exited = matches!(child.try_wait(), Ok(Some(_)) | Err(_));
        if cfg!(not(unix)) && root_exited {
            return true;
        }
        if cfg!(unix) && !process_group_alive(child.id()) {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
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
fn configure_process_group(_command: &mut Command) {}

#[cfg(target_os = "linux")]
fn configure_linux_launch_limits(
    command: &mut Command,
    memory: Option<&LinuxMemoryConfig>,
) -> Result<(), String> {
    let Some(memory) = memory else {
        return Ok(());
    };
    let address_space = parse_optional_memory_limit(&memory.setrlimit_address_space)?;
    let file_size = parse_optional_memory_limit(&memory.setrlimit_file_size)?;
    if address_space.is_none() && file_size.is_none() {
        return Ok(());
    }
    unsafe {
        command.pre_exec(move || {
            if let Some(bytes) = address_space {
                set_process_rlimit(RLIMIT_AS, bytes)?;
            }
            if let Some(bytes) = file_size {
                set_process_rlimit(RLIMIT_FSIZE, bytes)?;
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn configure_linux_launch_limits(
    _command: &mut Command,
    memory: Option<&LinuxMemoryConfig>,
) -> Result<(), String> {
    let Some(memory) = memory else {
        return Ok(());
    };
    if parse_optional_memory_limit(&memory.setrlimit_address_space)?.is_some()
        || parse_optional_memory_limit(&memory.setrlimit_file_size)?.is_some()
    {
        return Err("setrlimit launch limits are supported only on Linux".to_string());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
const RLIMIT_FSIZE: i32 = 1;
#[cfg(target_os = "linux")]
const RLIMIT_AS: i32 = 9;

#[cfg(target_os = "linux")]
fn set_process_rlimit(resource: i32, bytes: u64) -> std::io::Result<()> {
    #[repr(C)]
    struct Rlimit {
        rlim_cur: u64,
        rlim_max: u64,
    }
    unsafe extern "C" {
        fn setrlimit(resource: i32, rlim: *const Rlimit) -> i32;
    }
    let limit = Rlimit {
        rlim_cur: bytes,
        rlim_max: bytes,
    };
    if unsafe { setrlimit(resource, &limit) } == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn parse_optional_memory_limit(value: &str) -> Result<Option<u64>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("disabled")
        || trimmed.eq_ignore_ascii_case("off")
    {
        return Ok(None);
    }
    parse_memory_bytes(trimmed).map(Some)
}

fn parse_memory_bytes(value: &str) -> Result<u64, String> {
    let value = value.trim();
    let split_at = value
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(split_at);
    if number.is_empty() {
        return Err(format!("invalid setrlimit memory value `{value}`"));
    }
    let base = number
        .parse::<u64>()
        .map_err(|e| format!("parse setrlimit memory value `{value}`: {e}"))?;
    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        other => {
            return Err(format!(
                "unsupported setrlimit memory suffix `{other}` in `{value}`"
            ))
        }
    };
    base.checked_mul(multiplier)
        .ok_or_else(|| format!("setrlimit memory value `{value}` overflows u64"))
}

#[cfg(unix)]
fn terminate_process_group(child: &Child, signal: &str) {
    let _ = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(format!("-{}", child.id()))
        .status();
}

#[cfg(not(unix))]
fn terminate_process_group(_child: &Child, _signal: &str) {}

#[cfg(unix)]
fn process_group_alive(process_group_id: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(format!("-{process_group_id}"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_group_alive(_process_group_id: u32) -> bool {
    false
}

fn stream_codex_stdout(
    stdout: impl std::io::Read,
    path: PathBuf,
    console_prefix: String,
) -> Result<(), String> {
    let reader = BufReader::new(stdout);
    let mut file = File::create(&path).map_err(|e| format!("create {}: {e}", path.display()))?;
    for line in reader.lines() {
        let line = line.map_err(|e| format!("read codex stdout: {e}"))?;
        writeln!(file, "{line}").map_err(|e| format!("write {}: {e}", path.display()))?;
        if let Some(phase) = concise_codex_phase(&line) {
            println!("{console_prefix}: {phase}");
        }
    }
    Ok(())
}

fn stream_process_stderr(stderr: impl std::io::Read, path: PathBuf) -> Result<(), String> {
    let reader = BufReader::new(stderr);
    let mut file = File::create(&path).map_err(|e| format!("create {}: {e}", path.display()))?;
    for line in reader.lines() {
        let line = line.map_err(|e| format!("read process stderr: {e}"))?;
        writeln!(file, "{line}").map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn concise_codex_phase(line: &str) -> Option<String> {
    let event = serde_json::from_str::<serde_json::Value>(line).ok()?;
    let event_type = event
        .get("type")
        .or_else(|| event.get("event"))
        .and_then(serde_json::Value::as_str)?;
    match event_type {
        "thread.started" => Some("thread started".to_string()),
        "turn.started" => Some("turn started".to_string()),
        "turn.completed" => Some(
            codex_usage_summary(&event)
                .map(|usage| format!("turn completed ({usage})"))
                .unwrap_or_else(|| "turn completed".to_string()),
        ),
        "turn.failed" => Some(format!("turn failed{}", codex_event_detail(&event))),
        "error" => Some(format!("error{}", codex_event_detail(&event))),
        "item.started" | "item.completed" => concise_codex_item_phase(event_type, &event),
        _ => None,
    }
}

fn concise_codex_item_phase(event_type: &str, event: &serde_json::Value) -> Option<String> {
    let item = event.get("item")?;
    let item_type = item.get("type").and_then(serde_json::Value::as_str)?;
    let started = event_type == "item.started";
    let status = item
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(if started { "in_progress" } else { "completed" });

    match item_type {
        "agent_message" if !started => item
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(|text| format!("message: {}", compact_console_text(text, 240))),
        "command_execution" => concise_command_phase(item, started, status),
        "mcp_tool_call" => concise_mcp_phase(item, started, status),
        "file_change" => concise_file_change_phase(item, started, status),
        "error" => item
            .get("message")
            .and_then(serde_json::Value::as_str)
            .map(|message| format!("error: {}", compact_console_text(message, 240))),
        _ if status == "failed" => Some(format!(
            "{item_type} failed{}",
            codex_item_error_detail(item)
        )),
        _ => None,
    }
}

fn concise_command_phase(item: &serde_json::Value, started: bool, status: &str) -> Option<String> {
    let command = item
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(|command| compact_console_text(command, 220))?;
    if started {
        return Some(format!("command started: {command}"));
    }
    let exit = item.get("exit_code").and_then(serde_json::Value::as_i64);
    let label = match (status, exit) {
        ("failed", _) => "command failed",
        (_, Some(0)) => "command ok",
        (_, Some(_)) => "command failed",
        _ => "command completed",
    };
    let exit_detail = exit.map(|code| format!(" exit={code}")).unwrap_or_default();
    let output_detail = if label == "command failed" {
        item.get("aggregated_output")
            .and_then(serde_json::Value::as_str)
            .filter(|output| !output.trim().is_empty())
            .map(|output| format!(" output={}", compact_console_text(output, 240)))
            .unwrap_or_default()
    } else {
        String::new()
    };
    Some(format!("{label}: {command}{exit_detail}{output_detail}"))
}

fn concise_mcp_phase(item: &serde_json::Value, started: bool, status: &str) -> Option<String> {
    let server = item
        .get("server")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let tool = item
        .get("tool")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    if started {
        return Some(format!("mcp started: {server}.{tool}"));
    }
    if status == "failed" {
        return Some(format!(
            "mcp failed: {server}.{tool}{}",
            codex_item_error_detail(item)
        ));
    }
    Some(format!("mcp completed: {server}.{tool}"))
}

fn concise_file_change_phase(
    item: &serde_json::Value,
    started: bool,
    status: &str,
) -> Option<String> {
    let changes = item.get("changes").and_then(serde_json::Value::as_array)?;
    let summary = changes
        .iter()
        .filter_map(|change| {
            let path = change.get("path").and_then(serde_json::Value::as_str)?;
            let kind = change
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("change");
            Some(format!("{kind} {}", compact_console_text(path, 160)))
        })
        .collect::<Vec<_>>()
        .join("; ");
    if summary.is_empty() {
        return None;
    }
    let verb = if started { "started" } else { status };
    Some(format!(
        "file changes {verb}: {}",
        compact_console_text(&summary, 260)
    ))
}

fn codex_event_detail(event: &serde_json::Value) -> String {
    event
        .get("message")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            event.get("error").and_then(|error| {
                error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| error.as_str())
            })
        })
        .map(|message| format!(": {}", compact_console_text(message, 240)))
        .unwrap_or_default()
}

fn codex_item_error_detail(item: &serde_json::Value) -> String {
    item.get("error")
        .and_then(|error| {
            error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .or_else(|| error.as_str())
        })
        .map(|message| format!(": {}", compact_console_text(message, 240)))
        .unwrap_or_default()
}

fn codex_usage_summary(event: &serde_json::Value) -> Option<String> {
    let usage = event.get("usage")?;
    let mut parts = Vec::new();
    for (key, label) in [
        ("input_tokens", "input"),
        ("cached_input_tokens", "cached"),
        ("output_tokens", "output"),
        ("reasoning_output_tokens", "reasoning"),
        ("total_tokens", "total"),
    ] {
        if let Some(value) = usage.get(key).and_then(serde_json::Value::as_u64) {
            parts.push(format!("{label}={value}"));
        }
    }
    (!parts.is_empty()).then(|| format!("tokens: {}", parts.join(", ")))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PromptArtifactRef {
    prompt_artifact: PathBuf,
    prompt_metadata: PathBuf,
    sha256: String,
    bytes: usize,
    chars: usize,
}

fn prompt_artifact_ref(artifact_dir: &Path, prompt: &str) -> PromptArtifactRef {
    let prompt_artifact = artifact_dir.join("codex.prompt.txt");
    let prompt_metadata = artifact_dir.join("codex.prompt.metadata.json");
    PromptArtifactRef {
        prompt_artifact,
        prompt_metadata,
        sha256: format!("sha256:{}", sha256_hex(prompt.as_bytes())),
        bytes: prompt.len(),
        chars: prompt.chars().count(),
    }
}

fn write_codex_prompt_artifacts(
    artifact_dir: &Path,
    prompt: &str,
) -> Result<PromptArtifactRef, String> {
    fs::create_dir_all(artifact_dir)
        .map_err(|e| format!("create {}: {e}", artifact_dir.display()))?;
    let prompt_ref = prompt_artifact_ref(artifact_dir, prompt);
    write_file(&prompt_ref.prompt_artifact, prompt)?;
    let metadata = serde_json::json!({
        "schema_version": "0.1",
        "prompt_artifact": prompt_ref.prompt_artifact.display().to_string(),
        "artifact_sha256": prompt_ref.sha256,
        "bytes": prompt_ref.bytes,
        "chars": prompt_ref.chars,
        "redaction": "none",
        "visibility_mode": "full_body_sensitive_artifact",
        "note": "Full Codex prompt used for this run. Treat as sensitive run artifact."
    });
    write_file(
        &prompt_ref.prompt_metadata,
        &serde_json::to_string_pretty(&metadata)
            .map_err(|e| format!("render prompt metadata: {e}"))?,
    )?;
    Ok(prompt_ref)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn compact_console_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut truncated = compact.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn render_issue_context(issue: &Issue) -> String {
    let labels = if issue.labels.is_empty() {
        "[]".to_string()
    } else {
        issue.labels.join(", ")
    };
    let source = issue
        .source_artifact
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "not recorded".to_string());
    format!(
        "id: {}\nrepo: {}\nnumber: {}\nstate: {}\nauthor: {}\nlabels: {}\nurl: {}\nsource_artifact: {}\n\ntitle:\n{}\n\nbody:\n{}",
        issue.id,
        issue.repo,
        issue.number,
        issue.state,
        issue.author,
        labels,
        issue.html_url.as_deref().unwrap_or(""),
        source,
        issue.title,
        issue.body
    )
}

fn ensure_read_only_child(node: &AgentNode) -> Result<(), String> {
    if node.write_scope != WriteScope::None {
        return Err(format!(
            "child agent {} requested non-read-only write scope; only the Implementer may write",
            node.agent_run_id.as_str()
        ));
    }
    if node.tool_policy.allow_write_tools {
        return Err(format!(
            "child agent {} requested write tools; read-only helpers must not write",
            node.agent_run_id.as_str()
        ));
    }
    Ok(())
}

fn child_request_from_node(req: &AgentIssueRunRequest, node: &AgentNode) -> AgentIssueRunRequest {
    let mut child = req.clone();
    child.agent_run_id = node.agent_run_id.as_str().to_string();
    child.parent_agent_run_id = node
        .parent_agent_run_id
        .as_ref()
        .map(|agent| agent.as_str().to_string());
    child.role = node.role.as_str().to_string();
    child.objective = node.objective.clone();
    child.write_scope = "read_only".to_string();
    child.artifact_dir = node.artifact_dir.clone();
    child.memory = req.child_memory_lease(node.agent_run_id.as_str());
    child.child_memory.clear();
    child.spawn_plan = None;
    child
}

fn render_child_prompt(req: &AgentIssueRunRequest) -> String {
    format!(
        r#"You are a read-only helper agent for agentactr.

Target GitHub issue: {}#{}
Run id: {}
Agent id: {}
Parent agent id: {}
Role: {}
Objective: {}
Context manifest: {}

Strict helper rules:
- Do not modify files.
- Do not run commands that intentionally mutate repository state.
- Inspect only what is needed for your objective.
- Write a concise handoff in your final response with file paths, risks, and recommended next steps for the single writer.
- If context is missing, state that explicitly instead of guessing.

Issue context:
{}
"#,
        req.repo,
        req.issue,
        req.run_id,
        req.agent_run_id,
        req.parent_agent_run_id.as_deref().unwrap_or("none"),
        req.role,
        req.objective,
        req.context_manifest.display(),
        render_issue_context(&req.issue_context)
    )
}

fn render_spawn_context(plan: Option<&SpawnPlan>, reports: &[CodexChildRunReport]) -> String {
    let Some(plan) = plan else {
        return "spawn_plan: none".to_string();
    };
    if reports.is_empty() {
        return format!(
            "spawn_plan: present\nchildren_planned: {}\nchildren_completed: 0",
            plan.child_nodes.len()
        );
    }
    let mut lines = vec![
        "spawn_plan: present".to_string(),
        format!("children_planned: {}", plan.child_nodes.len()),
        format!("children_completed: {}", reports.len()),
    ];
    for report in reports {
        lines.push(format!(
            "- role={} agent={} handoff={} stdout_jsonl={}",
            report.role,
            report.agent_run_id,
            report.handoff.display(),
            report.stdout_jsonl.display()
        ));
    }
    lines.join("\n")
}

fn write_spawn_handoff_manifest(
    artifact_dir: &Path,
    reports: &[CodexChildRunReport],
) -> Result<(), String> {
    fs::create_dir_all(artifact_dir)
        .map_err(|e| format!("create {}: {e}", artifact_dir.display()))?;
    let payload = serde_json::json!({
        "schema_version": "0.1",
        "mode": "parallel_read_only_helpers",
        "children": reports.iter().map(|report| {
            serde_json::json!({
                "agent_run_id": report.agent_run_id,
                "role": report.role,
                "artifact_dir": report.artifact_dir.display().to_string(),
                "handoff": report.handoff.display().to_string(),
                "handoff_sha256": report.handoff_sha256,
                "handoff_bytes": report.handoff_bytes,
                "handoff_chars": report.handoff_chars,
                "handoff_redaction": "none",
                "handoff_visibility_mode": "reference_only",
                "prompt_artifact": report.prompt_artifact.display().to_string(),
                "prompt_metadata": report.prompt_metadata.display().to_string(),
                "prompt_artifact_sha256": report.prompt_sha256,
                "prompt_bytes": report.prompt_bytes,
                "prompt_chars": report.prompt_chars,
                "prompt_redaction": "none",
                "prompt_visibility_mode": "reference_only",
                "stdout_jsonl": report.stdout_jsonl.display().to_string(),
                "stderr_log": report.stderr_log.display().to_string(),
            })
        }).collect::<Vec<_>>(),
    });
    write_file(
        artifact_dir.join("spawn_handoffs.json"),
        &serde_json::to_string_pretty(&payload)
            .map_err(|e| format!("render spawn handoff manifest: {e}"))?,
    )
}

fn join_codex_streams(
    stdout_thread: thread::JoinHandle<Result<(), String>>,
    stderr_thread: thread::JoinHandle<Result<(), String>>,
) -> Result<(), String> {
    let mut first_error = None;
    match stdout_thread.join() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => first_error = Some(err),
        Err(_) => first_error = Some("codex stdout stream thread panicked".to_string()),
    }
    match stderr_thread.join() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            if first_error.is_none() {
                first_error = Some(err);
            }
        }
        Err(_) => {
            if first_error.is_none() {
                first_error = Some("codex stderr stream thread panicked".to_string());
            }
        }
    }
    if let Some(err) = first_error {
        Err(err)
    } else {
        Ok(())
    }
}

fn codex_jsonl_has_error_event(path: &Path) -> Result<bool, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(content.lines().any(codex_json_line_is_error_event))
}

fn codex_json_line_is_error_event(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|event| {
            event
                .get("type")
                .or_else(|| event.get("event"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
        .is_some_and(|event| event == "error" || event == "turn.failed")
}

fn codex_approval_config_value(policy: agentactr_sdk::RuntimeApprovalPolicy) -> &'static str {
    match policy {
        agentactr_sdk::RuntimeApprovalPolicy::Never => "never",
        agentactr_sdk::RuntimeApprovalPolicy::OnRequest => "on-request",
    }
}

fn codex_approval_cli_value(policy: agentactr_sdk::RuntimeApprovalPolicy) -> &'static str {
    match policy {
        agentactr_sdk::RuntimeApprovalPolicy::Never => "never",
        agentactr_sdk::RuntimeApprovalPolicy::OnRequest => "on-request",
    }
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

fn prepare_docker_codex_home(req: &AgentIssueRunRequest) -> Result<PathBuf, String> {
    let codex_home = req.artifact_dir.join("codex-home");
    fs::create_dir_all(&codex_home)
        .map_err(|e| format!("create Docker CODEX_HOME {}: {e}", codex_home.display()))?;
    let config_path = codex_home.join("config.toml");
    let mut trusted_paths = vec![req.worktree.clone()];
    if let Ok(canonical) = req.worktree.canonicalize() {
        if !trusted_paths.iter().any(|path| path == &canonical) {
            trusted_paths.push(canonical);
        }
    }

    let mut content =
        "# agentactr generated for Docker runtime; do not put secrets here\n".to_string();
    for project_path in trusted_paths {
        content.push_str(&format!(
            "\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            toml_escape(project_path.to_string_lossy().as_ref())
        ));
    }
    fs::write(&config_path, content)
        .map_err(|e| format!("write Docker Codex config {}: {e}", config_path.display()))?;
    Ok(codex_home)
}

fn toml_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn command_spec_from_command(
    command: Command,
    req: &AgentIssueRunRequest,
) -> Result<ProcessCommandSpec, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    if program.is_empty() {
        return Err("runtime command program is empty".to_string());
    }
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let cwd = command
        .get_current_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| req.worktree.clone());
    let env = command
        .get_envs()
        .filter_map(|(key, value)| {
            value.map(|value| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
        })
        .collect::<Vec<_>>();
    Ok(ProcessCommandSpec {
        program,
        args,
        cwd,
        env,
        worktree: req.worktree.clone(),
        artifact_dir: req.artifact_dir.clone(),
        trace_path: req.trace_path.clone(),
        run_id: req.run_id.clone(),
        agent_run_id: req.agent_run_id.clone(),
    })
}

fn forward_codex_api_key_env(command: &mut Command, configured_env: &str) {
    if let Ok(value) = env::var(configured_env) {
        command.env("CODEX_API_KEY", value);
    } else if configured_env != "CODEX_API_KEY" {
        if let Ok(value) = env::var("CODEX_API_KEY") {
            command.env("CODEX_API_KEY", value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentactr_sdk::{
        AgentMemoryLease, AgentNodeStatus, AgentRunId, AgentRuntime, AgentactrConfig,
        ContextBudget, MemoryLease, MemoryPolicyRef, OutputBudget, RuntimeApprovalPolicy,
        RuntimeProcessMonitor, RuntimeProcessSupervisor, ToolPolicy,
    };
    use std::ffi::OsString;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn codex_project_defaults_become_config_overrides_without_global_profile() {
        let root = temp_root("codex-profile-overrides");
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(
            root.join(".codex/config.toml"),
            r#"approval_policy = "never"
sandbox_mode = "workspace-write"
model_reasoning_effort = "medium"

[sandbox_workspace_write]
network_access = true
"#,
        )
        .unwrap();

        let mut command = Command::new("codex");
        command.arg("exec").arg("--json");
        append_codex_project_profile_overrides(&mut command, &root, "agentactr").unwrap();
        command.arg("--sandbox").arg("read-only");

        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg == "--profile"));
        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"approval_policy=\"never\"".to_string()));
        assert!(args.contains(&"model_reasoning_effort=\"medium\"".to_string()));
        assert!(args.contains(&"sandbox_mode=\"workspace-write\"".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_codex_project_profile_still_becomes_overrides() {
        let root = temp_root("codex-legacy-profile-overrides");
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(
            root.join(".codex/config.toml"),
            r#"[profiles.agentactr]
approval_policy = "never"
sandbox_mode = "workspace-write"
"#,
        )
        .unwrap();

        let mut command = Command::new("codex");
        append_codex_project_profile_overrides(&mut command, &root, "agentactr").unwrap();

        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.contains(&"approval_policy=\"never\"".to_string()));
        assert!(args.contains(&"sandbox_mode=\"workspace-write\"".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn setrlimit_memory_parser_is_opt_in_and_unit_aware() {
        assert_eq!(parse_optional_memory_limit("disabled").unwrap(), None);
        assert_eq!(parse_optional_memory_limit("off").unwrap(), None);
        assert_eq!(
            parse_optional_memory_limit("512M").unwrap(),
            Some(512 * 1024 * 1024)
        );
        assert_eq!(
            parse_optional_memory_limit("2G").unwrap(),
            Some(2 * 1024 * 1024 * 1024)
        );
        assert!(parse_optional_memory_limit("12Q").is_err());
    }

    #[test]
    fn docker_codex_home_trusts_mounted_worktree_only() {
        let root = temp_root("docker-codex-home");
        let worktree = root.join("worktree with spaces");
        let artifact_dir = root.join("artifacts");
        fs::create_dir_all(&worktree).unwrap();
        let req = AgentIssueRunRequest {
            worktree: worktree.clone(),
            artifact_dir: artifact_dir.clone(),
            ..AgentIssueRunRequest::default()
        };

        let codex_home = prepare_docker_codex_home(&req).unwrap();

        assert_eq!(codex_home, artifact_dir.join("codex-home"));
        let content = fs::read_to_string(codex_home.join("config.toml")).unwrap();
        let parsed = parse_toml_document(&content).unwrap();
        assert_eq!(
            parsed["projects"][worktree.to_string_lossy().as_ref()]["trust_level"].as_str(),
            Some("trusted")
        );
        assert!(!content.contains("CODEX_API_KEY"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_api_key_fallback_is_forwarded_explicitly_for_docker() {
        let _guard = ENV_LOCK.lock().unwrap();
        let custom_old = env::var_os("AGENTACTR_CUSTOM_CODEX_KEY");
        let fallback_old = env::var_os("CODEX_API_KEY");
        env::remove_var("AGENTACTR_CUSTOM_CODEX_KEY");
        env::set_var("CODEX_API_KEY", "fallback-secret");
        let mut command = Command::new("codex");

        forward_codex_api_key_env(&mut command, "AGENTACTR_CUSTOM_CODEX_KEY");

        let env_entries = command
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect::<Vec<_>>();
        assert!(env_entries
            .iter()
            .any(|(key, value)| key == "CODEX_API_KEY" && value == "fallback-secret"));
        match custom_old {
            Some(value) => env::set_var("AGENTACTR_CUSTOM_CODEX_KEY", value),
            None => env::remove_var("AGENTACTR_CUSTOM_CODEX_KEY"),
        }
        match fallback_old {
            Some(value) => env::set_var("CODEX_API_KEY", value),
            None => env::remove_var("CODEX_API_KEY"),
        }
    }

    #[test]
    fn codex_cancel_returns_explicit_milestone_error() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        let adapter = CodexRuntimeAdapter::new(&config).unwrap();

        let err = adapter.cancel("session-1", CancelReason::User).unwrap_err();

        assert!(err.contains("not implemented"));
        assert!(err.contains("session-1"));
    }

    #[test]
    fn codex_capabilities_mark_cancellation_as_unsupported() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        let adapter = CodexRuntimeAdapter::new(&config).unwrap();

        let capabilities = adapter.capabilities();
        let version = adapter.version_report();

        assert!(capabilities.single_shot_issue_run);
        assert!(capabilities.exec_json);
        assert!(!capabilities.app_server);
        assert!(!capabilities.cancellation);
        assert!(version.capability_digest.contains("cancel-unsupported"));
        assert!(version
            .degraded_features
            .contains(&"cancellation".to_string()));
        assert!(version
            .required_actions
            .iter()
            .any(|action| action.contains("contract tests")));
        assert!(!version.warnings.is_empty());
    }

    #[test]
    fn codex_runtime_selector_defaults_to_exec_json_adapter() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        let adapter = CodexRuntimeAdapter::new(&config).unwrap();

        let capabilities = adapter.capabilities();
        let version = adapter.version_report();

        assert!(capabilities.exec_json);
        assert!(!capabilities.app_server);
        assert_eq!(version.api_version, "codex-exec-json");
    }

    #[test]
    fn codex_runtime_selector_supports_app_server_stub() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        config.mode = "app_server".to_string();

        let adapter = CodexRuntimeAdapter::new(&config).unwrap();
        let capabilities = adapter.capabilities();
        let version = adapter.version_report();
        let err = adapter
            .run_issue(AgentIssueRunRequest::default())
            .unwrap_err();

        assert!(!capabilities.exec_json);
        assert!(capabilities.app_server);
        assert!(!capabilities.single_shot_issue_run);
        assert_eq!(version.api_version, "codex-app-server");
        assert!(version.capability_digest.contains("fail-closed"));
        assert!(version.capability_digest.contains("transport=stdio"));
        assert!(version
            .degraded_features
            .contains(&"single_shot_issue_run".to_string()));
        assert!(version
            .required_actions
            .iter()
            .any(|action| action.contains("stdio JSON-RPC")));
        assert!(version
            .warnings
            .iter()
            .any(|warning| warning.contains("fallback_mode=cli_json")));
        assert!(version
            .warnings
            .iter()
            .any(|warning| warning.contains("feature-gated")));
        assert!(err.contains("codex.mode = \"app_server\""));
        assert!(err.contains("codex.mode = \"cli_json\""));
    }

    #[test]
    fn codex_runtime_selector_supports_sdk_stub() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        config.mode = "codex_sdk".to_string();

        let adapter = CodexRuntimeAdapter::new(&config).unwrap();
        let capabilities = adapter.capabilities();
        let version = adapter.version_report();
        let err = adapter
            .run_issue(AgentIssueRunRequest::default())
            .unwrap_err();

        assert!(!capabilities.exec_json);
        assert!(!capabilities.app_server);
        assert!(capabilities.codex_sdk);
        assert!(!capabilities.single_shot_issue_run);
        assert_eq!(version.adapter_name, "agentactr-codex-sdk");
        assert_eq!(version.api_version, "codex-sdk-typescript");
        assert!(version.capability_digest.contains("fail-closed"));
        assert!(version.capability_digest.contains("bridge=typescript"));
        assert!(version
            .degraded_features
            .contains(&"single_shot_issue_run".to_string()));
        assert!(version
            .required_actions
            .iter()
            .any(|action| action.contains("@openai/codex-sdk sidecar")));
        assert!(version
            .warnings
            .iter()
            .any(|warning| warning.contains("fallback_mode=cli_json")));
        assert!(version
            .warnings
            .iter()
            .any(|warning| warning.contains("feature-gated")));
        assert!(err.contains("codex.mode = \"codex_sdk\""));
        assert!(err.contains("codex.mode = \"cli_json\""));
    }

    #[test]
    fn codex_runtime_selector_rejects_invalid_milestone_policy_before_adapter_selection() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        config.mode = "app_server".to_string();
        config.app_server_transport = "http".to_string();

        let err = match CodexRuntimeAdapter::new(&config) {
            Ok(_) => panic!("invalid app-server transport should fail closed"),
            Err(err) => err,
        };

        assert!(err.contains("codex.app_server_transport"));
        assert!(err.contains("stdio"));
        assert!(err.contains("websocket"));
    }

    #[test]
    fn child_request_uses_child_specific_memory_lease() {
        let parent_lease = MemoryLease {
            group_id: MemoryGroupId::new("run:run-1:agent:parent"),
            policy: MemoryPolicyRef::new("linux_memory.agent"),
        };
        let child_lease = MemoryLease {
            group_id: MemoryGroupId::new("run:run-1:agent:child"),
            policy: MemoryPolicyRef::new("linux_memory.agent"),
        };
        let request = AgentIssueRunRequest {
            agent_run_id: "agent-parent".to_string(),
            memory: Some(parent_lease),
            child_memory: vec![AgentMemoryLease {
                agent_run_id: "agent-child".to_string(),
                lease: child_lease.clone(),
            }],
            ..AgentIssueRunRequest::default()
        };
        let node = AgentNode {
            agent_run_id: AgentRunId::new("agent-child"),
            parent_agent_run_id: Some(AgentRunId::new("agent-parent")),
            role: agentactr_sdk::AgentRole::RepoExplorer,
            objective: "inspect".to_string(),
            read_scope: agentactr_sdk::ReadScope::FullWorkspace,
            write_scope: WriteScope::None,
            tool_policy: ToolPolicy::read_only(),
            context_budget: ContextBudget {
                max_uncached_input_tokens: 1_000,
                max_files: 10,
            },
            output_budget: OutputBudget {
                max_output_tokens: 1_000,
                max_artifact_bytes: 10_000,
            },
            memory_policy: None,
            artifact_dir: PathBuf::from("/tmp/agent-child"),
            status: AgentNodeStatus::Pending,
        };

        let child = child_request_from_node(&request, &node);

        assert_eq!(child.memory, Some(child_lease));
        assert!(child.child_memory.is_empty());
        assert_eq!(child.parent_agent_run_id.as_deref(), Some("agent-parent"));
        assert_eq!(
            runtime_process_attribution(&child, 1234)
                .parent_agent_run_id
                .as_ref()
                .map(AgentRunId::as_str),
            Some("agent-parent")
        );
    }

    #[test]
    fn child_request_without_registered_lease_does_not_reuse_parent_memory() {
        let request = AgentIssueRunRequest {
            agent_run_id: "agent-parent".to_string(),
            memory: Some(MemoryLease {
                group_id: MemoryGroupId::new("run:run-1:agent:parent"),
                policy: MemoryPolicyRef::new("linux_memory.agent"),
            }),
            ..AgentIssueRunRequest::default()
        };
        let node = AgentNode {
            agent_run_id: AgentRunId::new("agent-child"),
            parent_agent_run_id: Some(AgentRunId::new("agent-parent")),
            role: agentactr_sdk::AgentRole::Reviewer,
            objective: "review".to_string(),
            read_scope: agentactr_sdk::ReadScope::FullWorkspace,
            write_scope: WriteScope::None,
            tool_policy: ToolPolicy::read_only(),
            context_budget: ContextBudget {
                max_uncached_input_tokens: 1_000,
                max_files: 10,
            },
            output_budget: OutputBudget {
                max_output_tokens: 1_000,
                max_artifact_bytes: 10_000,
            },
            memory_policy: None,
            artifact_dir: PathBuf::from("/tmp/agent-child"),
            status: AgentNodeStatus::Pending,
        };

        let child = child_request_from_node(&request, &node);

        assert!(child.memory.is_none());
    }

    #[test]
    fn child_agent_join_waits_for_all_threads_after_first_failure() {
        let completed = Arc::new(Mutex::new(Vec::new()));
        let fast_failure = thread::spawn(|| Err("fast child failed".to_string()));
        let completed_clone = Arc::clone(&completed);
        let slow_success = thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            completed_clone
                .lock()
                .unwrap()
                .push("slow-child".to_string());
            Ok(child_report("slow-child"))
        });

        let err = join_child_agent_threads(vec![fast_failure, slow_success]).unwrap_err();

        assert_eq!(err, "fast child failed");
        assert_eq!(completed.lock().unwrap().as_slice(), ["slow-child"]);
    }

    #[test]
    fn codex_stream_join_waits_for_stderr_after_stdout_failure() {
        let completed = Arc::new(Mutex::new(Vec::new()));
        let stdout_failure = thread::spawn(|| Err("stdout failed".to_string()));
        let completed_clone = Arc::clone(&completed);
        let stderr_success = thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            completed_clone.lock().unwrap().push("stderr".to_string());
            Ok(())
        });

        let err = join_codex_streams(stdout_failure, stderr_success).unwrap_err();

        assert_eq!(err, "stdout failed");
        assert_eq!(completed.lock().unwrap().as_slice(), ["stderr"]);
    }

    #[test]
    fn runtime_process_event_carries_neutral_memory_attribution() {
        let request = AgentIssueRunRequest {
            run_id: "run-1".to_string(),
            agent_run_id: "agent-1".to_string(),
            memory: Some(MemoryLease {
                group_id: MemoryGroupId::new("run:run-1:agent:agent-1"),
                policy: MemoryPolicyRef::new("linux_memory.agent"),
            }),
            ..AgentIssueRunRequest::default()
        };

        let event = RuntimeProcessEvent::new(
            RuntimeProcessEventKind::Started,
            runtime_process_attribution(&request, 1234),
        );

        assert_eq!(event.kind, RuntimeProcessEventKind::Started);
        assert_eq!(event.run_id.as_str(), "run-1");
        assert_eq!(event.agent_run_id.as_str(), "agent-1");
        assert_eq!(event.attribution.runtime_kind.as_str(), "codex");
        assert_eq!(event.attribution.transport_kind.as_str(), "cli_json");
        assert_eq!(event.attribution.root_pid, Some(ProcessId(1234)));
        assert_eq!(
            event
                .attribution
                .memory_group_id
                .as_ref()
                .map(MemoryGroupId::as_str),
            Some("run:run-1:agent:agent-1")
        );
    }

    fn child_report(agent_run_id: &str) -> CodexChildRunReport {
        let root = PathBuf::from(format!("/tmp/{agent_run_id}"));
        CodexChildRunReport {
            agent_run_id: agent_run_id.to_string(),
            role: "RepoExplorer".to_string(),
            artifact_dir: root.clone(),
            prompt_artifact: root.join("codex.prompt.txt"),
            prompt_metadata: root.join("codex.prompt.metadata.json"),
            prompt_sha256: "abc123".to_string(),
            prompt_bytes: 3,
            prompt_chars: 3,
            handoff: root.join("handoff.md"),
            handoff_sha256: "def456".to_string(),
            handoff_bytes: 3,
            handoff_chars: 3,
            stdout_jsonl: root.join("codex.stdout.jsonl"),
            stderr_log: root.join("codex.stderr.log"),
        }
    }

    #[test]
    fn runtime_process_lifecycle_events_share_attribution() {
        let request = AgentIssueRunRequest {
            run_id: "run-1".to_string(),
            agent_run_id: "agent-1".to_string(),
            ..AgentIssueRunRequest::default()
        };

        let events = [
            RuntimeProcessEvent::new(
                RuntimeProcessEventKind::Started,
                runtime_process_attribution(&request, 1234),
            ),
            RuntimeProcessEvent::new(
                RuntimeProcessEventKind::Attributed,
                runtime_process_attribution(&request, 1234),
            ),
            RuntimeProcessEvent::new(
                RuntimeProcessEventKind::Terminated,
                runtime_process_attribution(&request, 1234),
            ),
        ];

        assert_eq!(events[0].kind, RuntimeProcessEventKind::Started);
        assert_eq!(events[1].kind, RuntimeProcessEventKind::Attributed);
        assert_eq!(events[2].kind, RuntimeProcessEventKind::Terminated);
        assert!(events
            .iter()
            .all(|event| event.attribution.root_pid == Some(ProcessId(1234))));
        assert!(events
            .iter()
            .all(|event| event.attribution.transport_kind.as_str() == "cli_json"));
    }

    #[derive(Default)]
    struct RecordingSupervisor {
        events: Mutex<Vec<RuntimeProcessEventKind>>,
    }

    impl RuntimeProcessSupervisor for RecordingSupervisor {
        fn observe(&self, event: &RuntimeProcessEvent) -> Result<(), String> {
            self.events.lock().unwrap().push(event.kind);
            Ok(())
        }

        fn start(
            &self,
            _event: &RuntimeProcessEvent,
            _artifact_dir: &Path,
        ) -> Result<Option<Box<dyn RuntimeProcessMonitor>>, String> {
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
    }

    #[test]
    fn runtime_process_lifecycle_drop_emits_terminated() {
        let supervisor = Arc::new(RecordingSupervisor::default());
        let attribution = RuntimeProcessAttribution::new(
            RunId::new("run-1"),
            AgentRunId::new("agent-1"),
            RuntimeKind::new("codex"),
            RuntimeTransportKind::new("cli_json"),
            RuntimeProcessModel::OneShotProcess,
        )
        .with_root_pid(ProcessId(1234));

        {
            let _lifecycle =
                RuntimeProcessLifecycle::start(supervisor.clone(), attribution).unwrap();
        }

        assert_eq!(
            supervisor.events.lock().unwrap().as_slice(),
            [
                RuntimeProcessEventKind::Started,
                RuntimeProcessEventKind::Terminated
            ]
        );
    }

    #[test]
    fn codex_jsonl_error_events_are_failures() {
        assert!(codex_json_line_is_error_event(
            r#"{"type":"turn.failed","error":{"message":"boom"}}"#
        ));
        assert!(codex_json_line_is_error_event(
            r#"{"event":"error","message":"boom"}"#
        ));
        assert!(!codex_json_line_is_error_event(
            r#"{"type":"turn.completed"}"#
        ));
    }

    #[test]
    fn codex_console_phase_reports_turn_token_usage() {
        let phase = concise_codex_phase(
            r#"{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":80,"output_tokens":12,"reasoning_output_tokens":3,"total_tokens":112}}"#,
        )
        .unwrap();

        assert!(phase.contains("turn completed"));
        assert!(phase.contains("input=100"));
        assert!(phase.contains("cached=80"));
        assert!(phase.contains("output=12"));
        assert!(phase.contains("reasoning=3"));
        assert!(phase.contains("total=112"));
    }

    #[test]
    fn codex_prompt_artifacts_record_exact_prompt_and_metadata() {
        let root = temp_root("codex-prompt-artifact");
        let issue = Issue {
            id: "issue-1".to_string(),
            repo: "OWNER/REPO".to_string(),
            number: 42,
            title: "Fix bug".to_string(),
            body: "Body text".to_string(),
            ..Issue::default()
        };
        let request = AgentIssueRunRequest {
            run_id: "run-1".to_string(),
            agent_run_id: "agent-1".to_string(),
            role: "Implementer".to_string(),
            objective: "Implement issue".to_string(),
            write_scope: "workspace".to_string(),
            worktree: root.clone(),
            artifact_dir: root.clone(),
            trace_path: root.join("trace.jsonl"),
            context_manifest: root.join("context_manifest.json"),
            repo: "OWNER/REPO".to_string(),
            issue: "42".to_string(),
            issue_context: issue,
            approval_policy: RuntimeApprovalPolicy::Never,
            ..AgentIssueRunRequest::default()
        };
        let prompt = render_prompt_for_test(&request);
        fs::create_dir_all(&root).unwrap();

        write_codex_prompt_artifacts(&root, &prompt).unwrap();

        let prompt_text = fs::read_to_string(root.join("codex.prompt.txt")).unwrap();
        let metadata = fs::read_to_string(root.join("codex.prompt.metadata.json")).unwrap();
        let _ = fs::remove_dir_all(root);

        assert_eq!(prompt_text, prompt);
        assert!(metadata.contains("codex.prompt.txt"));
        assert!(metadata.contains(&format!(
            r#""artifact_sha256": "sha256:{}""#,
            sha256_hex(prompt.as_bytes())
        )));
        assert!(metadata.contains("\"redaction\": \"none\""));
        assert!(metadata.contains("\"visibility_mode\": \"full_body_sensitive_artifact\""));
        assert!(include_str!("lib.rs")
            .contains("--human-intervention interactive --codex-approval on-request"));
    }

    #[test]
    fn spawn_handoff_manifest_references_child_prompt_artifacts() {
        let root = temp_root("codex-spawn-handoff");
        let child_dir = root.join("child-1");
        let report = CodexChildRunReport {
            agent_run_id: "agent-child-1".to_string(),
            role: "RepoExplorer".to_string(),
            artifact_dir: child_dir.clone(),
            prompt_artifact: child_dir.join("codex.prompt.txt"),
            prompt_metadata: child_dir.join("codex.prompt.metadata.json"),
            prompt_sha256: "sha256:abc123".to_string(),
            prompt_bytes: 123,
            prompt_chars: 121,
            handoff: child_dir.join("handoff.md"),
            handoff_sha256: "sha256:def456".to_string(),
            handoff_bytes: 456,
            handoff_chars: 451,
            stdout_jsonl: child_dir.join("codex.stdout.jsonl"),
            stderr_log: child_dir.join("codex.stderr.log"),
        };
        fs::create_dir_all(&root).unwrap();

        write_spawn_handoff_manifest(&root, &[report]).unwrap();

        let manifest = fs::read_to_string(root.join("spawn_handoffs.json")).unwrap();
        let parsed = serde_json::from_str::<serde_json::Value>(&manifest).unwrap();
        let child = &parsed["children"][0];
        let _ = fs::remove_dir_all(root);

        assert_eq!(
            child["prompt_artifact"],
            child_dir.join("codex.prompt.txt").display().to_string()
        );
        assert_eq!(
            child["prompt_metadata"],
            child_dir
                .join("codex.prompt.metadata.json")
                .display()
                .to_string()
        );
        assert_eq!(child["handoff_sha256"], "sha256:def456");
        assert_eq!(child["handoff_bytes"], 456);
        assert_eq!(child["handoff_chars"], 451);
        assert_eq!(child["handoff_redaction"], "none");
        assert_eq!(child["handoff_visibility_mode"], "reference_only");
        assert_eq!(child["prompt_artifact_sha256"], "sha256:abc123");
        assert_eq!(child["prompt_bytes"], 123);
        assert_eq!(child["prompt_chars"], 121);
        assert_eq!(child["prompt_redaction"], "none");
        assert_eq!(child["prompt_visibility_mode"], "reference_only");
    }

    fn render_prompt_for_test(req: &AgentIssueRunRequest) -> String {
        format!(
            "Target GitHub issue: {}#{}\nRun id: {}\n{}",
            req.repo, req.issue, req.run_id, req.issue_context.title
        )
    }

    fn temp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!(
            "agentactr-codex-{name}-{nanos}-{}",
            std::process::id()
        ))
    }

    #[allow(dead_code)]
    fn with_env_var(key: &str, value: Option<&str>, test: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap();
        let old = env::var_os(key);
        match value {
            Some(value) => env::set_var(key, value),
            None => env::remove_var(key),
        }
        test();
        match old {
            Some(value) => env::set_var(key, value),
            None => env::remove_var(key),
        }
    }

    #[allow(dead_code)]
    fn os(value: &str) -> OsString {
        OsString::from(value)
    }
}
