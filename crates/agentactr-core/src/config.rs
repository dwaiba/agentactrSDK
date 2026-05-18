#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodexAuthMode {
    Auto,
    ChatGptSubscription,
    OpenAiApiKey,
}

impl CodexAuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ChatGptSubscription => "chatgpt",
            Self::OpenAiApiKey => "api_key",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "chatgpt" | "subscription" | "chatgpt_subscription" => Ok(Self::ChatGptSubscription),
            "api-key" | "api_key" | "openai_api_key" => Ok(Self::OpenAiApiKey),
            other => Err(format!("unsupported Codex auth mode: {other}")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodexMode {
    CliJsonExec,
    AppServer,
    CodexSdk,
}

impl CodexMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CliJsonExec => "cli_json",
            Self::AppServer => "app_server",
            Self::CodexSdk => "codex_sdk",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "cli_json" | "cli-json" | "exec_json" | "exec-json" | "codex_exec_json" => {
                Ok(Self::CliJsonExec)
            }
            "app_server" | "app-server" => Ok(Self::AppServer),
            "codex_sdk" | "codex-sdk" | "sdk" => Ok(Self::CodexSdk),
            other => Err(format!(
                "unsupported Codex mode: {other}; expected cli_json, app_server, or codex_sdk"
            )),
        }
    }

    pub fn parse_canonical_config(value: &str) -> Result<Self, String> {
        let parsed = Self::parse(value)?;
        if value == parsed.as_str() {
            Ok(parsed)
        } else {
            Err(canonical_config_error("codex.mode", value, parsed.as_str()))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodexAppServerTransport {
    Stdio,
    Websocket,
}

impl CodexAppServerTransport {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Websocket => "websocket",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "stdio" | "stdio://" => Ok(Self::Stdio),
            "websocket" | "websocket-experimental" | "ws" | "ws://" => Ok(Self::Websocket),
            other => Err(format!(
                "unsupported codex.app_server_transport: {other}; expected stdio or websocket"
            )),
        }
    }

    pub fn parse_canonical_config(value: &str) -> Result<Self, String> {
        let parsed = Self::parse(value)?;
        if value == parsed.as_str() {
            Ok(parsed)
        } else {
            Err(canonical_config_error(
                "codex.app_server_transport",
                value,
                parsed.as_str(),
            ))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodexSdkBridge {
    TypeScript,
}

impl CodexSdkBridge {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "typescript" | "ts" | "node" | "nodejs" => Ok(Self::TypeScript),
            other => Err(format!(
                "unsupported codex.sdk_bridge: {other}; expected typescript"
            )),
        }
    }

    pub fn parse_canonical_config(value: &str) -> Result<Self, String> {
        let parsed = Self::parse(value)?;
        if value == parsed.as_str() {
            Ok(parsed)
        } else {
            Err(canonical_config_error(
                "codex.sdk_bridge",
                value,
                parsed.as_str(),
            ))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodexFallbackMode {
    CliJson,
}

impl CodexFallbackMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CliJson => "cli_json",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "cli_json" | "cli-json" | "exec_json" | "exec-json" | "codex_exec_json" => {
                Ok(Self::CliJson)
            }
            other => Err(format!(
                "unsupported codex.fallback_mode: {other}; expected cli_json"
            )),
        }
    }

    pub fn parse_canonical_config(value: &str) -> Result<Self, String> {
        let parsed = Self::parse(value)?;
        if value == parsed.as_str() {
            Ok(parsed)
        } else {
            Err(canonical_config_error(
                "codex.fallback_mode",
                value,
                parsed.as_str(),
            ))
        }
    }
}

fn canonical_config_error(key: &str, value: &str, canonical: &str) -> String {
    format!(
        "{key} must use canonical stored value `{canonical}`, got alias `{value}`; aliases are accepted only by CLI-facing parsers"
    )
}

#[derive(Clone, Debug)]
pub struct AgentactrConfig {
    pub tracker: TrackerConfig,
    pub codex: CodexConfig,
    pub human_intervention: HumanInterventionConfig,
    pub github: GithubConfig,
    pub mcp: McpConfig,
    pub repository: RepositoryConfig,
    pub vcs: VcsConfig,
    pub quality: QualityConfig,
    pub architecture: ArchitectureConfig,
    pub templates: TemplatesConfig,
    pub commit: CommitConfig,
    pub merge: MergeConfig,
    pub workspace: WorkspaceConfig,
    pub scheduling: SchedulingConfig,
    pub spawn: SpawnConfig,
    pub execution: ExecutionConfig,
    pub linux_memory: LinuxMemoryConfig,
    pub observability: ObservabilityConfig,
}

impl AgentactrConfig {
    pub fn strict_defaults(repo: impl Into<String>) -> Self {
        Self {
            tracker: TrackerConfig {
                kind: "github".to_string(),
                repo: repo.into(),
                token_env: "GITHUB_TOKEN".to_string(),
                github_api_version: "2026-03-10".to_string(),
                active_labels: vec!["agentactr:ready".to_string()],
                ignore_labels: vec!["agentactr:blocked".to_string()],
                claim_label: "agentactr:claimed".to_string(),
                running_label: "agentactr:running".to_string(),
                failed_label: "agentactr:failed".to_string(),
                done_label: "agentactr:done".to_string(),
            },
            codex: CodexConfig {
                command: "codex".to_string(),
                mode: "cli_json".to_string(),
                profile: "agentactr".to_string(),
                approval_policy: "never".to_string(),
                sandbox_mode: "workspace-write".to_string(),
                network: "off".to_string(),
                default_model: "configured-by-codex".to_string(),
                model_reasoning_effort: "medium".to_string(),
                auth_mode: CodexAuthMode::Auto,
                openai_api_key_env: "CODEX_API_KEY".to_string(),
                app_server_transport: "stdio".to_string(),
                app_server_experimental_api: false,
                sdk_bridge: "typescript".to_string(),
                fallback_mode: "cli_json".to_string(),
            },
            human_intervention: HumanInterventionConfig {
                mode: "fail_closed".to_string(),
                on_codex_approval_request: "fail_run".to_string(),
                on_ambiguous_diff: "fail_quality_gate".to_string(),
                on_review_disagreement: "fail_quality_gate".to_string(),
                on_missing_codex_auth: "fail_startup".to_string(),
                on_missing_github_token: "fail_startup".to_string(),
                run_start_banner: true,
                print_override_steps: true,
            },
            github: GithubConfig {
                finalization: "require_human_review".to_string(),
                standard_label_policy: "ensure_on_issue_create".to_string(),
                project_automation: "disabled".to_string(),
                project_owner: "auto".to_string(),
                project_number: 0,
                project_title: "Agentactr".to_string(),
                project_priority_field: "Priority".to_string(),
                project_size_field: "Size".to_string(),
            },
            mcp: McpConfig {
                default_policy: "auto_setup_detected_credentials".to_string(),
                remote_research_servers: "auto_enable_when_credentials_detected".to_string(),
                remote_github_read_tools: "auto_enable_when_token_detected".to_string(),
                remote_github_write_tools: "disabled_by_default".to_string(),
                openai_developer_docs: "auto_enable_no_auth".to_string(),
                google_developer_api: "auto_enable_with_GOOGLE_API_KEY".to_string(),
                huggingface: "auto_enable_with_oauth_or_HF_TOKEN".to_string(),
                github_remote: "auto_enable_read_only_with_token".to_string(),
                fail_on_required_mcp_missing: true,
            },
            repository: RepositoryConfig {
                empty_repo_policy: "fail_closed_unless_stack_declared".to_string(),
                declared_primary_stack: "auto".to_string(),
                allowed_bootstrap: "explicit_only".to_string(),
                bootstrap_prereqs: "minimal_for_declared_stack".to_string(),
                fail_on_low_confidence_stack_detection: true,
            },
            vcs: VcsConfig {
                kind: "git".to_string(),
                workspace_strategy: "worktree".to_string(),
                base_ref: "origin/main".to_string(),
                worktree_root: ".agentactr/worktrees".to_string(),
                branch_template: "agentactr/{repo_slug}/issue-{issue_number}/{run_id}".to_string(),
                record_base_commit: true,
                fail_on_dirty_source_checkout: true,
                copy_runtime_config_to_worktree: true,
                detect_cross_issue_file_overlap: true,
                overlap_policy: "fail_closed".to_string(),
            },
            quality: QualityConfig {
                profile: "strict".to_string(),
                pre_commit_mode: "required".to_string(),
                technology_detection: "auto".to_string(),
                domains: vec!["auto".to_string()],
                domain_gate_opt_ins: Vec::new(),
                run_existing_pre_commit_config: true,
                fail_on_missing_toolchain: true,
                fail_on_untracked_generated_files: true,
                allow_test_omission_reason: true,
                artifact_dir: ".agentactr/artifacts/quality".to_string(),
                dependency_checks: true,
                architecture_checks: true,
                tool_pinning: "required_for_strict".to_string(),
            },
            architecture: ArchitectureConfig {
                domains: vec!["auto".to_string()],
                domain_graph_artifact: ".agentactr/artifacts/domain_graph.json".to_string(),
                fail_on_domain_drift: true,
            },
            templates: TemplatesConfig {
                enabled_domains: vec!["auto".to_string()],
                framework_profile: "auto".to_string(),
                agents_policy: "generate_when_absent".to_string(),
            },
            commit: CommitConfig {
                mode: "local_after_quality_gates".to_string(),
                signoff: false,
                gpg_sign: "inherit".to_string(),
                message_template: "agentactr: fix {tracker_ref}".to_string(),
                required_trailers: vec![
                    "Agentactr-Run-Id".to_string(),
                    "Tracker-Ref".to_string(),
                    "Base-Commit".to_string(),
                    "Policy".to_string(),
                ],
            },
            merge: MergeConfig {
                mode: "disabled".to_string(),
                push: "disabled".to_string(),
                strategy: "fast_forward_only".to_string(),
                require_clean_rebase: true,
                require_no_cross_issue_overlap: true,
                require_human_review_for_merge: true,
            },
            workspace: WorkspaceConfig {
                root: ".agentactr/workspaces".to_string(),
                keep_successful: true,
                keep_failed: true,
            },
            scheduling: SchedulingConfig {
                poll_interval_ms: 30_000,
                max_concurrent_issue_runs: 3,
                lease_ttl_ms: 300_000,
                max_retries: 5,
            },
            spawn: SpawnConfig {
                enabled: true,
                max_child_agents_per_issue: 4,
                max_spawn_depth: 1,
                allow_parallel_read_only: true,
                allow_parallel_writers: false,
                strategy: "budget_aware_one_writer".to_string(),
                max_total_uncached_input_tokens: 250_000,
                max_child_uncached_input_tokens: 80_000,
                max_child_output_tokens: 12_000,
                artifact_handoff: "refs_summaries_and_digests".to_string(),
                pause_on_memory_pressure: true,
            },
            execution: ExecutionConfig {
                backend: "auto".to_string(),
                strict_memory_required: true,
                docker: DockerExecutionConfig {
                    command: "docker".to_string(),
                    image: "ghcr.io/dwaiba/agentactr-runtime:0.1.0-linux-arm64".to_string(),
                    pull_policy: "if_missing".to_string(),
                    network: "bridge".to_string(),
                    workspace_mount: "rw".to_string(),
                    artifact_mount: "rw".to_string(),
                    remove_containers: true,
                    container_prefix: "agentactr".to_string(),
                },
            },
            linux_memory: LinuxMemoryConfig {
                enabled: true,
                cgroup_root: "auto".to_string(),
                root_group: "agentactr".to_string(),
                mode: "enforce_on_linux_observe_elsewhere".to_string(),
                cgroup_v2_required: true,
                psi_required: true,
                per_issue_memory_high: "4G".to_string(),
                per_issue_memory_max: "6G".to_string(),
                per_agent_memory_high: "2G".to_string(),
                per_agent_memory_max: "2G".to_string(),
                psi_memory_some_threshold_us: 150_000,
                psi_memory_window_us: 1_000_000,
                oom_score_adj: 300,
                setrlimit_address_space: "disabled".to_string(),
                setrlimit_file_size: "disabled".to_string(),
                kill_policy: "cancel_lowest_priority_subagent".to_string(),
                oom_policy: "fail_run_preserve_debug_bundle".to_string(),
            },
            observability: ObservabilityConfig {
                jsonl: ".agentactr/runs/events.jsonl".to_string(),
                sqlite: ".agentactr/runs/agentactr.sqlite".to_string(),
                artifact_root: ".agentactr/artifacts".to_string(),
                otel_enabled: false,
                otel_endpoint: "http://localhost:4317".to_string(),
                debug_bundle_root: ".agentactr/debug".to_string(),
                redact_secrets: true,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct TrackerConfig {
    pub kind: String,
    pub repo: String,
    pub token_env: String,
    pub github_api_version: String,
    pub active_labels: Vec<String>,
    pub ignore_labels: Vec<String>,
    pub claim_label: String,
    pub running_label: String,
    pub failed_label: String,
    pub done_label: String,
}

#[derive(Clone, Debug)]
pub struct CodexConfig {
    pub command: String,
    pub mode: String,
    pub profile: String,
    pub approval_policy: String,
    pub sandbox_mode: String,
    pub network: String,
    pub default_model: String,
    pub model_reasoning_effort: String,
    pub auth_mode: CodexAuthMode,
    pub openai_api_key_env: String,
    pub app_server_transport: String,
    pub app_server_experimental_api: bool,
    pub sdk_bridge: String,
    pub fallback_mode: String,
}

impl CodexConfig {
    pub fn validate_milestone_policy(&self) -> Result<(), String> {
        let _mode = CodexMode::parse_canonical_config(&self.mode)?;
        let app_server_transport =
            CodexAppServerTransport::parse_canonical_config(&self.app_server_transport)?;
        if app_server_transport == CodexAppServerTransport::Websocket
            && !self.app_server_experimental_api
        {
            return Err(
                "codex.app_server_transport=websocket requires codex.app_server_experimental_api=true because Codex app-server WebSocket is experimental and unsupported"
                    .to_string(),
            );
        }
        let _sdk_bridge = CodexSdkBridge::parse_canonical_config(&self.sdk_bridge)?;
        let _fallback = CodexFallbackMode::parse_canonical_config(&self.fallback_mode)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct HumanInterventionConfig {
    pub mode: String,
    pub on_codex_approval_request: String,
    pub on_ambiguous_diff: String,
    pub on_review_disagreement: String,
    pub on_missing_codex_auth: String,
    pub on_missing_github_token: String,
    pub run_start_banner: bool,
    pub print_override_steps: bool,
}

#[derive(Clone, Debug)]
pub struct GithubConfig {
    pub finalization: String,
    pub standard_label_policy: String,
    pub project_automation: String,
    pub project_owner: String,
    pub project_number: u32,
    pub project_title: String,
    pub project_priority_field: String,
    pub project_size_field: String,
}

#[derive(Clone, Debug)]
pub struct McpConfig {
    pub default_policy: String,
    pub remote_research_servers: String,
    pub remote_github_read_tools: String,
    pub remote_github_write_tools: String,
    pub openai_developer_docs: String,
    pub google_developer_api: String,
    pub huggingface: String,
    pub github_remote: String,
    pub fail_on_required_mcp_missing: bool,
}

#[derive(Clone, Debug)]
pub struct RepositoryConfig {
    pub empty_repo_policy: String,
    pub declared_primary_stack: String,
    pub allowed_bootstrap: String,
    pub bootstrap_prereqs: String,
    pub fail_on_low_confidence_stack_detection: bool,
}

#[derive(Clone, Debug)]
pub struct VcsConfig {
    pub kind: String,
    pub workspace_strategy: String,
    pub base_ref: String,
    pub worktree_root: String,
    pub branch_template: String,
    pub record_base_commit: bool,
    pub fail_on_dirty_source_checkout: bool,
    pub copy_runtime_config_to_worktree: bool,
    pub detect_cross_issue_file_overlap: bool,
    pub overlap_policy: String,
}

#[derive(Clone, Debug)]
pub struct QualityConfig {
    pub profile: String,
    pub pre_commit_mode: String,
    pub technology_detection: String,
    pub domains: Vec<String>,
    pub domain_gate_opt_ins: Vec<String>,
    pub run_existing_pre_commit_config: bool,
    pub fail_on_missing_toolchain: bool,
    pub fail_on_untracked_generated_files: bool,
    pub allow_test_omission_reason: bool,
    pub artifact_dir: String,
    pub dependency_checks: bool,
    pub architecture_checks: bool,
    pub tool_pinning: String,
}

#[derive(Clone, Debug)]
pub struct ArchitectureConfig {
    pub domains: Vec<String>,
    pub domain_graph_artifact: String,
    pub fail_on_domain_drift: bool,
}

#[derive(Clone, Debug)]
pub struct TemplatesConfig {
    pub enabled_domains: Vec<String>,
    pub framework_profile: String,
    pub agents_policy: String,
}

#[derive(Clone, Debug)]
pub struct CommitConfig {
    pub mode: String,
    pub signoff: bool,
    pub gpg_sign: String,
    pub message_template: String,
    pub required_trailers: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct MergeConfig {
    pub mode: String,
    pub push: String,
    pub strategy: String,
    pub require_clean_rebase: bool,
    pub require_no_cross_issue_overlap: bool,
    pub require_human_review_for_merge: bool,
}

#[derive(Clone, Debug)]
pub struct WorkspaceConfig {
    pub root: String,
    pub keep_successful: bool,
    pub keep_failed: bool,
}

#[derive(Clone, Debug)]
pub struct SchedulingConfig {
    pub poll_interval_ms: u64,
    pub max_concurrent_issue_runs: u64,
    pub lease_ttl_ms: u64,
    pub max_retries: u64,
}

#[derive(Clone, Debug)]
pub struct SpawnConfig {
    pub enabled: bool,
    pub max_child_agents_per_issue: u64,
    pub max_spawn_depth: u64,
    pub allow_parallel_read_only: bool,
    pub allow_parallel_writers: bool,
    pub strategy: String,
    pub max_total_uncached_input_tokens: u64,
    pub max_child_uncached_input_tokens: u64,
    pub max_child_output_tokens: u64,
    pub artifact_handoff: String,
    pub pause_on_memory_pressure: bool,
}

#[derive(Clone, Debug)]
pub struct ExecutionConfig {
    pub backend: String,
    pub strict_memory_required: bool,
    pub docker: DockerExecutionConfig,
}

#[derive(Clone, Debug)]
pub struct DockerExecutionConfig {
    pub command: String,
    pub image: String,
    pub pull_policy: String,
    pub network: String,
    pub workspace_mount: String,
    pub artifact_mount: String,
    pub remove_containers: bool,
    pub container_prefix: String,
}

#[derive(Clone, Debug)]
pub struct LinuxMemoryConfig {
    pub enabled: bool,
    pub cgroup_root: String,
    pub root_group: String,
    pub mode: String,
    pub cgroup_v2_required: bool,
    pub psi_required: bool,
    pub per_issue_memory_high: String,
    pub per_issue_memory_max: String,
    pub per_agent_memory_high: String,
    pub per_agent_memory_max: String,
    pub psi_memory_some_threshold_us: u64,
    pub psi_memory_window_us: u64,
    pub oom_score_adj: i64,
    pub setrlimit_address_space: String,
    pub setrlimit_file_size: String,
    pub kill_policy: String,
    pub oom_policy: String,
}

#[derive(Clone, Debug)]
pub struct ObservabilityConfig {
    pub jsonl: String,
    pub sqlite: String,
    pub artifact_root: String,
    pub otel_enabled: bool,
    pub otel_endpoint: String,
    pub debug_bundle_root: String,
    pub redact_secrets: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        AgentactrConfig, CodexAppServerTransport, CodexFallbackMode, CodexMode, CodexSdkBridge,
    };

    #[test]
    fn codex_mode_parse_accepts_stable_app_server_and_sdk_values() {
        assert_eq!(
            CodexMode::parse("cli_json").unwrap(),
            CodexMode::CliJsonExec
        );
        assert_eq!(
            CodexMode::parse("exec-json").unwrap(),
            CodexMode::CliJsonExec
        );
        assert_eq!(
            CodexMode::parse("app_server").unwrap(),
            CodexMode::AppServer
        );
        assert_eq!(
            CodexMode::parse("app-server").unwrap(),
            CodexMode::AppServer
        );
        assert_eq!(CodexMode::parse("codex_sdk").unwrap(), CodexMode::CodexSdk);
        assert_eq!(CodexMode::parse("sdk").unwrap(), CodexMode::CodexSdk);
    }

    #[test]
    fn codex_mode_parse_rejects_unknown_values() {
        let err = CodexMode::parse("responses").unwrap_err();

        assert!(err.contains("unsupported Codex mode"));
        assert!(err.contains("cli_json"));
        assert!(err.contains("app_server"));
        assert!(err.contains("codex_sdk"));
    }

    #[test]
    fn strict_defaults_use_publishable_docker_runtime_with_model_egress() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");

        assert_eq!(
            config.execution.docker.image,
            "ghcr.io/dwaiba/agentactr-runtime:0.1.0-linux-arm64"
        );
        assert_eq!(config.execution.docker.network, "bridge");
        assert_eq!(config.codex.network, "off");
    }

    #[test]
    fn strict_defaults_keep_milestone_transports_fail_closed_and_configured() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");

        assert_eq!(config.codex.mode, "cli_json");
        assert_eq!(config.codex.app_server_transport, "stdio");
        assert!(!config.codex.app_server_experimental_api);
        assert_eq!(config.codex.sdk_bridge, "typescript");
        assert_eq!(config.codex.fallback_mode, "cli_json");
        config.codex.validate_milestone_policy().unwrap();
    }

    #[test]
    fn codex_milestone_policy_accepts_only_documented_bootstrap_values() {
        assert_eq!(
            CodexAppServerTransport::parse("stdio").unwrap(),
            CodexAppServerTransport::Stdio
        );
        assert_eq!(
            CodexAppServerTransport::parse("websocket").unwrap(),
            CodexAppServerTransport::Websocket
        );
        assert_eq!(
            CodexSdkBridge::parse("typescript").unwrap(),
            CodexSdkBridge::TypeScript
        );
        assert_eq!(
            CodexFallbackMode::parse("cli-json").unwrap(),
            CodexFallbackMode::CliJson
        );

        let transport_err = CodexAppServerTransport::parse("http").unwrap_err();
        let sdk_err = CodexSdkBridge::parse("python").unwrap_err();
        let fallback_err = CodexFallbackMode::parse("app_server").unwrap_err();

        assert!(transport_err.contains("codex.app_server_transport"));
        assert!(sdk_err.contains("codex.sdk_bridge"));
        assert!(fallback_err.contains("codex.fallback_mode"));
    }

    #[test]
    fn codex_milestone_policy_rejects_stored_aliases() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        config.mode = "exec-json".to_string();
        let err = config.validate_milestone_policy().unwrap_err();
        assert!(err.contains("codex.mode"));
        assert!(err.contains("canonical stored value `cli_json`"));

        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        config.app_server_transport = "ws".to_string();
        config.app_server_experimental_api = true;
        let err = config.validate_milestone_policy().unwrap_err();
        assert!(err.contains("codex.app_server_transport"));
        assert!(err.contains("canonical stored value `websocket`"));

        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        config.sdk_bridge = "ts".to_string();
        let err = config.validate_milestone_policy().unwrap_err();
        assert!(err.contains("codex.sdk_bridge"));
        assert!(err.contains("canonical stored value `typescript`"));

        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        config.fallback_mode = "exec-json".to_string();
        let err = config.validate_milestone_policy().unwrap_err();
        assert!(err.contains("codex.fallback_mode"));
        assert!(err.contains("canonical stored value `cli_json`"));
    }

    #[test]
    fn codex_milestone_policy_requires_explicit_experimental_flag_for_websocket() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO").codex;
        config.app_server_transport = "websocket".to_string();

        let err = config.validate_milestone_policy().unwrap_err();

        assert!(err.contains("app_server_transport=websocket"));
        assert!(err.contains("app_server_experimental_api=true"));

        config.app_server_experimental_api = true;
        config.validate_milestone_policy().unwrap();
    }
}
