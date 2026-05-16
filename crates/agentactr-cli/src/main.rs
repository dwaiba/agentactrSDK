mod adapters;
mod artifacts;
mod linux_memory;

use adapters::{
    append_codex_project_profile_overrides, codex_runtime_adapter, validate_github_repo,
    validate_issue_number, CliCodexMemorySupervisor, CodexAppServerAdapter, CodexRuntimeAdapter,
    CodexSdkAdapter, GithubRestAdapter, LocalGitAdapter,
};
use agentactr_execution::{resolve_execution_backend, ExecutionBackend, ExecutionBackendDecision};
use agentactr_sdk::{
    apply_declared_stack_to_inspection_with_config, discover_repository,
    discover_repository_with_config, domain_findings, domain_findings_to_json,
    domain_graph_to_json, domain_quality_plan_to_json, finalize_recorded_run_with_tracker,
    recorded_run_lifecycle_summary, render_agentactr_toml, render_agents_md,
    render_codex_config_toml, render_gitignore_additions, render_workflow_md, AdapterCapabilities,
    AdapterVersionReport, AgentActrBuilder, AgentActrUseCases, AgentIssueRunRequest,
    AgentMemoryLease, AgentRuntime, AgentactrConfig, CodexAppServerTransport, CodexAuthMode,
    CodexFallbackMode, CodexMode, CodexSdkBridge, DetectedCredentials, FinalizeDecision,
    FsRunFinalizationArtifacts, IssueDraftPlanner, IssueFieldValue, IssueId, IssueLifecycleMode,
    IssueLifecycleRequest, IssueProjectFieldValue, IssueProposal, IssueProposalId,
    IssueSetArtifactContext, IssueSetSource, IssueSubmissionDecision, IssueSubmissionLedgerEntry,
    IssueSubmissionLedgerState, IssueTracker, LifecycleLabels, MemoryLease, MergePlan,
    MergePlanRequest, QualityGateSummary, RecordedRunFinalizationRequest, RepoInspection,
    RunIssueHooks, RunIssuePostRuntimeContext, RunIssueRequest, RunIssueRuntimeContext,
    RunOutcomeSummary, RuntimeApprovalPolicy, SpawnPlan, StackKind, VersionControl, WorkspaceDiff,
    WorktreeRef, WorktreeRequest,
};
use artifacts::{sha256_hex_bytes, ArtifactIntegrityContext};
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use linux_memory::{LinuxMemoryController, MemoryRunContext};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::info;
use tracing_subscriber::EnvFilter;

const MCP_PROTOCOL_LATEST: &str = "2025-11-25";
const MCP_PROTOCOL_SUPPORTED: &[&str] = &["2025-11-25", "2024-11-05"];
const CODEX_AUTH_VALUES: &[&str] = &["auto", "chatgpt", "api-key"];
const AUTH_CODEX_METHOD_VALUES: &[&str] = &["chatgpt", "subscription", "api-key"];
const HUMAN_INTERVENTION_VALUES: &[&str] = &["fail-closed", "interactive", "review-required"];
const CODEX_APPROVAL_VALUES: &[&str] = &["never", "on-request"];
const GITHUB_FINALIZATION_VALUES: &[&str] = &[
    "automatic_after_quality_gates",
    "require_human_review",
    "disabled",
];
const GITHUB_STANDARD_LABEL_POLICY_VALUES: &[&str] = &["ensure_on_issue_create", "disabled"];
const GITHUB_PROJECT_AUTOMATION_VALUES: &[&str] = &["disabled", "ensure_on_issue_create"];
const RUN_ISSUE_USAGE: &str = "usage: agentactr run issue --repo OWNER/REPO --issue 123 [--human-intervention fail-closed|interactive|review-required] [--codex-approval never|on-request] [--github-finalization automatic_after_quality_gates|require_human_review|disabled] [--dry-run]";
const RUN_ISSUE_VALUE_FLAGS: &[&str] = &[
    "--repo",
    "--issue",
    "--human-intervention",
    "--codex-approval",
    "--github-finalization",
];
const RUN_ISSUE_BOOL_FLAGS: &[&str] = &["--dry-run"];
const COMPLETION_SHELL_VALUES: &[&str] = &["bash", "zsh", "fish", "powershell", "elvish"];
const CONFIG_KEY_VALUES: &[&str] = &[
    "tracker.kind",
    "tracker.repo",
    "tracker.token_env",
    "tracker.github_api_version",
    "tracker.active_labels",
    "tracker.ignore_labels",
    "tracker.claim_label",
    "tracker.running_label",
    "tracker.failed_label",
    "tracker.done_label",
    "codex.command",
    "codex.mode",
    "codex.profile",
    "codex.approval_policy",
    "codex.sandbox_mode",
    "codex.network",
    "codex.default_model",
    "codex.model_reasoning_effort",
    "codex.openai_api_key_env",
    "codex.app_server_transport",
    "codex.app_server_experimental_api",
    "codex.sdk_bridge",
    "codex.fallback_mode",
    "codex.auth_mode",
    "human_intervention.mode",
    "human_intervention.on_codex_approval_request",
    "human_intervention.on_ambiguous_diff",
    "human_intervention.on_review_disagreement",
    "human_intervention.on_missing_codex_auth",
    "human_intervention.on_missing_github_token",
    "human_intervention.run_start_banner",
    "human_intervention.print_override_steps",
    "github.finalization",
    "github.standard_label_policy",
    "github.project_automation",
    "github.project_owner",
    "github.project_number",
    "github.project_title",
    "github.project_priority_field",
    "github.project_size_field",
    "mcp.default_policy",
    "mcp.remote_research_servers",
    "mcp.remote_github_read_tools",
    "mcp.remote_github_write_tools",
    "mcp.openai_developer_docs",
    "mcp.google_developer_api",
    "mcp.huggingface",
    "mcp.github_remote",
    "mcp.fail_on_required_mcp_missing",
    "repository.empty_repo_policy",
    "repository.declared_primary_stack",
    "repository.allowed_bootstrap",
    "repository.bootstrap_prereqs",
    "repository.fail_on_low_confidence_stack_detection",
    "vcs.kind",
    "vcs.workspace_strategy",
    "vcs.base_ref",
    "vcs.worktree_root",
    "vcs.branch_template",
    "vcs.record_base_commit",
    "vcs.fail_on_dirty_source_checkout",
    "vcs.copy_runtime_config_to_worktree",
    "vcs.detect_cross_issue_file_overlap",
    "vcs.overlap_policy",
    "quality.profile",
    "quality.pre_commit_mode",
    "quality.technology_detection",
    "quality.domains",
    "quality.domain_gate_opt_ins",
    "quality.run_existing_pre_commit_config",
    "quality.fail_on_missing_toolchain",
    "quality.fail_on_untracked_generated_files",
    "quality.allow_test_omission_reason",
    "quality.artifact_dir",
    "quality.dependency_checks",
    "quality.architecture_checks",
    "quality.tool_pinning",
    "architecture.domains",
    "architecture.domain_graph_artifact",
    "architecture.fail_on_domain_drift",
    "templates.enabled_domains",
    "templates.framework_profile",
    "templates.agents_policy",
    "commit.mode",
    "commit.signoff",
    "commit.gpg_sign",
    "commit.message_template",
    "commit.required_trailers",
    "merge.mode",
    "merge.push",
    "merge.strategy",
    "merge.require_clean_rebase",
    "merge.require_no_cross_issue_overlap",
    "merge.require_human_review_for_merge",
    "workspace.root",
    "workspace.keep_successful",
    "workspace.keep_failed",
    "scheduling.poll_interval_ms",
    "scheduling.max_concurrent_issue_runs",
    "scheduling.lease_ttl_ms",
    "scheduling.max_retries",
    "spawn.enabled",
    "spawn.max_child_agents_per_issue",
    "spawn.max_spawn_depth",
    "spawn.allow_parallel_read_only",
    "spawn.allow_parallel_writers",
    "spawn.strategy",
    "spawn.max_total_uncached_input_tokens",
    "spawn.max_child_uncached_input_tokens",
    "spawn.max_child_output_tokens",
    "spawn.artifact_handoff",
    "spawn.pause_on_memory_pressure",
    "execution.backend",
    "execution.strict_memory_required",
    "execution.docker.command",
    "execution.docker.image",
    "execution.docker.pull_policy",
    "execution.docker.network",
    "execution.docker.workspace_mount",
    "execution.docker.artifact_mount",
    "execution.docker.remove_containers",
    "execution.docker.container_prefix",
    "linux_memory.enabled",
    "linux_memory.cgroup_root",
    "linux_memory.root_group",
    "linux_memory.mode",
    "linux_memory.cgroup_v2_required",
    "linux_memory.psi_required",
    "linux_memory.per_issue_memory_high",
    "linux_memory.per_issue_memory_max",
    "linux_memory.per_agent_memory_high",
    "linux_memory.per_agent_memory_max",
    "linux_memory.psi_memory_some_threshold_us",
    "linux_memory.psi_memory_window_us",
    "linux_memory.oom_score_adj",
    "linux_memory.setrlimit_address_space",
    "linux_memory.setrlimit_file_size",
    "linux_memory.kill_policy",
    "linux_memory.oom_policy",
    "observability.jsonl",
    "observability.sqlite",
    "observability.artifact_root",
    "observability.otel_enabled",
    "observability.otel_endpoint",
    "observability.debug_bundle_root",
    "observability.redact_secrets",
];

fn static_value_parser(values: &'static [&'static str]) -> clap::builder::PossibleValuesParser {
    clap::builder::PossibleValuesParser::new(values.iter().copied())
}

#[derive(Debug, Parser)]
#[command(
    name = "agentactr",
    disable_help_flag = true,
    disable_version_flag = true,
    ignore_errors = true
)]
struct CliArgs {
    #[arg(num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, Parser)]
#[command(
    name = "agentactr",
    disable_version_flag = true,
    disable_help_subcommand = true
)]
#[command(about = "Run agentactr issue automation and inspect local run artifacts.")]
struct AgentactrHelpCli {
    #[command(subcommand)]
    command: Option<AgentactrHelpCommand>,
}

#[derive(Debug, Subcommand)]
enum AgentactrHelpCommand {
    #[command(about = "Print command help. Use `agentactr help COMMAND [SUBCOMMAND...]`.")]
    Help(HelpHelpArgs),
    #[command(about = "List CLI command inventory and implementation status.")]
    Commands(JsonFlagArgs),
    #[command(about = "Print a read-only command picker and setup navigator.")]
    Menu(JsonFlagArgs),
    #[command(
        about = "Create local agentactr, Codex, workflow, AGENTS.md, and ignore configuration."
    )]
    Init(InitHelpArgs),
    #[command(
        about = "Inspect local config, credentials, adapters, runtime, and platform readiness."
    )]
    Doctor(DoctorHelpArgs),
    #[command(subcommand, about = "Read or write local agentactr configuration.")]
    Config(ConfigHelpCommand),
    #[command(subcommand, about = "Configure authentication for supported runtimes.")]
    Auth(AuthHelpCommand),
    #[command(subcommand, about = "Run the local MCP bridge.")]
    Mcp(McpHelpCommand),
    #[command(subcommand, about = "Run issue automation.")]
    Run(RunHelpCommand),
    #[command(
        subcommand,
        about = "Inspect and submit agent-proposed tracker issues."
    )]
    Issue(IssueHelpCommand),
    #[command(about = "Run the future scheduler/daemon. Milestone command.")]
    Daemon(DaemonHelpArgs),
    #[command(subcommand, about = "Inspect the local run trace ledger.")]
    Trace(TraceHelpCommand),
    #[command(subcommand, about = "Create local debug bundles.")]
    Debug(DebugHelpCommand),
    #[command(about = "Rebuild run state from trace/artifacts. Milestone command.")]
    Replay(RunIdHelpArgs),
    #[command(
        subcommand,
        about = "Plan merge readiness without mutating Git or GitHub."
    )]
    Merge(MergeHelpCommand),
    #[command(about = "Finalize a run after human review.")]
    Finalize(FinalizeHelpArgs),
    #[command(about = "Run evaluation harnesses. Milestone command.")]
    Eval(EvalHelpArgs),
    #[command(about = "Generate shell completion scripts from the typed clap command tree.")]
    Completions(CompletionsHelpArgs),
    #[command(
        subcommand,
        about = "Generate documentation artifacts from CLI metadata."
    )]
    Docs(DocsHelpCommand),
    #[command(subcommand, about = "Inspect repository stack and quality plan.")]
    Repo(RepoHelpCommand),
    #[command(subcommand, about = "Plan or rerun quality gates.")]
    Quality(QualityHelpCommand),
    #[command(subcommand, about = "Prepare and inspect local Git worktrees.")]
    Vcs(VcsHelpCommand),
    #[command(subcommand, about = "Inspect memory controller status.")]
    Memory(MemoryHelpCommand),
    #[command(about = "Print bootstrap CLI status.")]
    Status,
}

#[derive(Debug, Args)]
struct HelpHelpArgs {
    #[arg(value_name = "COMMAND", num_args = 0.., allow_hyphen_values = true)]
    command: Vec<String>,
}

#[derive(Debug, Args)]
struct JsonFlagArgs {
    #[arg(long, help = "Print JSON output.")]
    json: bool,
}

#[derive(Debug, Args)]
struct InitHelpArgs {
    #[arg(long, help = "Permit writing generated local files.")]
    yes: bool,
    #[arg(long, value_name = "OWNER/REPO", help = "GitHub repository slug.")]
    repo: Option<String>,
    #[arg(
        long,
        value_name = "auto|chatgpt|api-key",
        value_parser = static_value_parser(CODEX_AUTH_VALUES),
        help = "Codex auth mode to render."
    )]
    codex_auth: Option<String>,
}

#[derive(Debug, Args)]
struct DoctorHelpArgs {
    #[arg(long, help = "Rewrite repo-local .codex/config.toml only.")]
    fix_codex_config: bool,
    #[arg(
        long,
        help = "Generate AGENTS.md when absent, or write a review artifact when present."
    )]
    fix_agents: bool,
    #[arg(
        long,
        help = "Explicitly update Codex user config to trust this project."
    )]
    trust_codex_project: bool,
}

#[derive(Debug, Subcommand)]
enum ConfigHelpCommand {
    #[command(about = "Read effective local configuration.")]
    Get(ConfigGetHelpArgs),
    #[command(about = "Persist a supported local configuration value.")]
    Set(ConfigSetHelpArgs),
}

#[derive(Debug, Args)]
struct ConfigGetHelpArgs {
    #[arg(value_name = "KEY", value_parser = static_value_parser(CONFIG_KEY_VALUES))]
    key: Option<String>,
}

#[derive(Debug, Args)]
struct ConfigSetHelpArgs {
    #[arg(value_name = "KEY", value_parser = static_value_parser(CONFIG_KEY_VALUES))]
    key: String,
    #[arg(value_name = "VALUE")]
    value: String,
}

#[derive(Debug, Subcommand)]
enum AuthHelpCommand {
    #[command(about = "Configure Codex authentication mode for this repository.")]
    Codex(AuthCodexHelpArgs),
}

#[derive(Debug, Args)]
struct AuthCodexHelpArgs {
    #[arg(
        long,
        value_name = "chatgpt|subscription|api-key",
        value_parser = static_value_parser(AUTH_CODEX_METHOD_VALUES)
    )]
    method: String,
    #[arg(long, value_name = "CODEX_API_KEY")]
    api_key_env: Option<String>,
}

#[derive(Debug, Subcommand)]
enum McpHelpCommand {
    #[command(about = "Run the local stdio MCP bridge for run-scoped read tools.")]
    Serve,
}

#[derive(Debug, Subcommand)]
enum RunHelpCommand {
    #[command(about = "Run a single GitHub issue through the configured runtime.")]
    Issue(RunIssueHelpArgs),
    #[command(about = "Run issues selected from tracker query results. Milestone command.")]
    Query(RunQueryHelpArgs),
}

#[derive(Debug, Subcommand)]
enum IssueHelpCommand {
    #[command(about = "Find existing tracker issues without running agents or mutating GitHub.")]
    Find(IssueFindHelpArgs),
    #[command(about = "Draft local issue proposals from repo evidence or a prompt.")]
    Draft(IssueDraftHelpArgs),
    #[command(about = "List issue-set proposals without mutating GitHub.")]
    Proposals(IssueSetIdHelpArgs),
    #[command(about = "Submit one review-gated issue proposal through the tracker.")]
    Submit(IssueSubmitHelpArgs),
    #[command(about = "Record a reviewed dedupe decision for one proposal.")]
    Mark(IssueMarkHelpArgs),
}

#[derive(Debug, Args)]
struct IssueFindHelpArgs {
    #[arg(long, value_name = "OWNER/REPO")]
    repo: String,
    #[arg(long, value_name = "open|closed|all", default_value = "open")]
    state: String,
    #[arg(long = "query", alias = "search", value_name = "TEXT")]
    query: Option<String>,
    #[arg(long, value_name = "LABEL")]
    label: Vec<String>,
    #[arg(long, value_name = "USER|none|*")]
    assignee: Option<String>,
    #[arg(long, value_name = "USER")]
    author: Option<String>,
    #[arg(long, value_name = "ISO8601")]
    since: Option<String>,
    #[arg(
        long,
        value_name = "created|updated|comments",
        default_value = "updated"
    )]
    sort: String,
    #[arg(long, value_name = "asc|desc", default_value = "desc")]
    direction: String,
    #[arg(long, value_name = "N")]
    page: Option<u32>,
    #[arg(long, value_name = "N", default_value = "50")]
    per_page: u32,
    #[arg(long, value_name = "N", default_value = "50")]
    limit: u32,
    #[arg(long, value_name = "PATH")]
    artifact_root: Option<String>,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    include_pull_requests: bool,
}

#[derive(Debug, Args)]
struct IssueDraftHelpArgs {
    #[arg(long, value_name = "OWNER/REPO")]
    repo: String,
    #[arg(long, value_name = "TEXT")]
    prompt: Option<String>,
    #[arg(long, value_name = "PATH")]
    prompt_file: Option<String>,
    #[arg(long, value_name = "rust|typescript|golang|python")]
    stack: Option<String>,
    #[arg(long, value_name = "nextjs|none")]
    framework: Option<String>,
    #[arg(long, value_name = "ISSUE_NUMBER")]
    parent: Option<u64>,
    #[arg(long, value_name = "PATH")]
    artifact_root: Option<String>,
    #[arg(long, value_name = "N", default_value = "50")]
    limit: u32,
    #[arg(long)]
    codex_draft: bool,
    #[arg(long)]
    codex_review: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct IssueSetIdHelpArgs {
    #[arg(value_name = "ISSUE_SET_ID")]
    issue_set_id: String,
}

#[derive(Debug, Args)]
struct IssueSubmitHelpArgs {
    #[arg(value_name = "ISSUE_SET_ID")]
    issue_set_id: String,
    #[arg(long, value_name = "PROPOSAL_ID")]
    proposal: String,
    #[arg(long)]
    resume: bool,
    #[arg(long)]
    yes: bool,
    #[arg(long)]
    allow_possible_duplicate: bool,
    #[arg(long, value_name = "TEXT")]
    reason: Option<String>,
    #[arg(long)]
    require_codex_review: bool,
}

#[derive(Debug, Args)]
struct IssueMarkHelpArgs {
    #[arg(value_name = "ISSUE_SET_ID")]
    issue_set_id: String,
    #[arg(long, value_name = "PROPOSAL_ID")]
    proposal: String,
    #[arg(long, value_name = "unique|duplicate_blocked")]
    dedupe: String,
    #[arg(long, value_name = "TEXT")]
    reason: String,
}

#[derive(Debug, Args)]
struct RunIssueHelpArgs {
    #[arg(long, value_name = "OWNER/REPO")]
    repo: String,
    #[arg(long, value_name = "123")]
    issue: String,
    #[arg(
        long,
        value_name = "fail-closed|interactive|review-required",
        value_parser = static_value_parser(HUMAN_INTERVENTION_VALUES)
    )]
    human_intervention: Option<String>,
    #[arg(
        long,
        value_name = "never|on-request",
        value_parser = static_value_parser(CODEX_APPROVAL_VALUES)
    )]
    codex_approval: Option<String>,
    #[arg(
        long,
        value_name = "automatic_after_quality_gates|require_human_review|disabled",
        value_parser = static_value_parser(GITHUB_FINALIZATION_VALUES)
    )]
    github_finalization: Option<String>,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct RunQueryHelpArgs {
    #[arg(long, value_name = "OWNER/REPO")]
    repo: String,
    #[arg(long, value_name = "LABEL")]
    label: String,
    #[arg(
        long,
        value_name = "fail-closed|interactive|review-required",
        value_parser = static_value_parser(HUMAN_INTERVENTION_VALUES)
    )]
    human_intervention: String,
}

#[derive(Debug, Args)]
struct DaemonHelpArgs {
    #[arg(long, value_name = "agentactr.toml")]
    config: String,
}

#[derive(Debug, Subcommand)]
enum TraceHelpCommand {
    #[command(about = "Summarize run ids in the local JSONL event ledger.")]
    List,
    #[command(about = "Show a run-scoped trace timeline and artifact integrity summary.")]
    Show(RunIdHelpArgs),
}

#[derive(Debug, Subcommand)]
enum DebugHelpCommand {
    #[command(about = "Create a redacted local debug bundle for a run.")]
    Bundle(RunIdHelpArgs),
}

#[derive(Debug, Subcommand)]
enum MergeHelpCommand {
    #[command(about = "Record a read-only merge risk plan artifact for a run.")]
    Plan(MergePlanHelpArgs),
}

#[derive(Debug, Args)]
struct MergePlanHelpArgs {
    #[arg(value_name = "RUN_ID")]
    run_id: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct FinalizeHelpArgs {
    #[arg(value_name = "RUN_ID")]
    run_id: String,
    #[arg(long, conflicts_with = "reject")]
    approve: bool,
    #[arg(long, conflicts_with = "approve")]
    reject: bool,
    #[arg(long, value_name = "REASON")]
    reason: Option<String>,
    #[arg(long)]
    resume: bool,
}

#[derive(Debug, Args)]
struct EvalHelpArgs {
    #[arg(value_name = "ARGS", num_args = 0.., trailing_var_arg = true)]
    args: Vec<String>,
}

#[derive(Debug, Args)]
struct CompletionsHelpArgs {
    #[arg(
        value_name = "bash|zsh|fish|powershell|elvish",
        value_parser = static_value_parser(COMPLETION_SHELL_VALUES)
    )]
    shell: String,
}

#[derive(Debug, Subcommand)]
enum DocsHelpCommand {
    #[command(
        about = "Render Markdown CLI reference from the typed clap tree and command catalog."
    )]
    CliMarkdown(DocsCliMarkdownHelpArgs),
}

#[derive(Debug, Args)]
struct DocsCliMarkdownHelpArgs {
    #[arg(long, value_name = "PATH")]
    output: Option<String>,
}

#[derive(Debug, Subcommand)]
enum RepoHelpCommand {
    #[command(about = "Inspect repository stack and selected quality profile.")]
    Inspect,
}

#[derive(Debug, Subcommand)]
enum QualityHelpCommand {
    #[command(about = "Print detected quality plan for the current repository.")]
    Plan,
    #[command(about = "Rerun quality gates in the recorded isolated worktree.")]
    Run(RunIdHelpArgs),
}

#[derive(Debug, Subcommand)]
enum VcsHelpCommand {
    #[command(about = "Prepare a local isolated Git worktree for an issue.")]
    Prepare(VcsPrepareHelpArgs),
    #[command(about = "List retained local run worktrees from manifest artifacts.")]
    List(JsonFlagArgs),
    #[command(about = "Show detailed recorded VCS/worktree metadata for one run.")]
    Show(ShowRunHelpArgs),
    #[command(about = "Read recorded run worktree status and touched files.")]
    Status(RunIdHelpArgs),
    #[command(about = "Record a read-only workspace diff artifact for a run.")]
    Diff(VcsDiffHelpArgs),
    #[command(about = "Create a local commit after quality gates. Milestone command.")]
    Commit(RunIdHelpArgs),
    #[command(about = "Remove retained local worktree after retention policy. Milestone command.")]
    Cleanup(RunIdHelpArgs),
}

#[derive(Debug, Args)]
struct VcsPrepareHelpArgs {
    #[arg(long, value_name = "123")]
    issue: String,
    #[arg(long, value_name = "OWNER/REPO")]
    repo: Option<String>,
}

#[derive(Debug, Args)]
struct ShowRunHelpArgs {
    #[arg(value_name = "RUN_ID")]
    run_id: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct VcsDiffHelpArgs {
    #[arg(value_name = "RUN_ID")]
    run_id: String,
    #[arg(long, value_name = "PATH")]
    output: Option<String>,
}

#[derive(Debug, Subcommand)]
enum MemoryHelpCommand {
    #[command(about = "Print local memory controller status.")]
    Status,
    #[command(about = "Print local memory pressure observations.")]
    Pressure,
}

#[derive(Debug, Args)]
struct RunIdHelpArgs {
    #[arg(value_name = "RUN_ID")]
    run_id: String,
}

fn main() {
    init_tracing();
    if let Err(err) = run() {
        eprintln!("agentactr: {err}");
        std::process::exit(1);
    }
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("agentactr_cli=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .try_init();
}

fn run() -> Result<(), String> {
    let mut args = CliArgs::try_parse()
        .map(|cli| cli.args)
        .unwrap_or_else(|_| env::args().skip(1).collect::<Vec<_>>());
    if let Some(help_path) = generated_help_path(&args) {
        print_generated_help(&help_path)?;
        return Ok(());
    }
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => cmd_version(),
        Some("init") => cmd_init(&mut args),
        Some("doctor") => cmd_doctor(&mut args),
        Some("config") => cmd_config(&mut args),
        Some("auth") => cmd_auth(&mut args),
        Some("mcp") => cmd_mcp(&mut args),
        Some("run") => cmd_run(&mut args),
        Some("issue") => cmd_issue(&mut args),
        Some("daemon") => not_implemented("daemon"),
        Some("trace") => cmd_trace(&mut args),
        Some("debug") => cmd_debug(&mut args),
        Some("replay") => not_implemented("replay"),
        Some("merge") => cmd_merge(&mut args),
        Some("finalize") => cmd_finalize(&mut args),
        Some("eval") => not_implemented("eval"),
        Some("commands") => cmd_commands(&mut args),
        Some("completions") => cmd_completions(&mut args),
        Some("docs") => cmd_docs(&mut args),
        Some("menu") => cmd_menu(&mut args),
        Some("repo") => cmd_repo(&mut args),
        Some("quality") => cmd_quality(&mut args),
        Some("vcs") => cmd_vcs(&mut args),
        Some("memory") => cmd_memory(&mut args),
        Some("status") => cmd_status(),
        Some("help") | Some("--help") | Some("-h") | None => {
            print_generated_help(&[])?;
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`. Run `agentactr help`.")),
    }
}

fn cmd_version() -> Result<(), String> {
    println!("{}", version_string());
    Ok(())
}

fn version_string() -> String {
    format!(
        "agentactr {} (git_sha={} rustc=\"{}\")",
        env!("CARGO_PKG_VERSION"),
        env!("AGENTACTR_BUILD_GIT_SHA"),
        env!("AGENTACTR_BUILD_RUSTC_VERSION")
    )
}

fn help_text() -> &'static str {
    r#"agentactr

Implemented bootstrap commands:
  --version
  commands [--json]
  init --yes --repo OWNER/REPO [--codex-auth auto|chatgpt|api-key]
  doctor [--fix-codex-config] [--fix-agents] [--trust-codex-project]
  config get [KEY]
  config set KEY VALUE
  auth codex --method chatgpt|subscription|api-key [--api-key-env CODEX_API_KEY]
  run issue --repo OWNER/REPO --issue 123 [--human-intervention fail-closed|interactive|review-required] [--codex-approval never|on-request] [--github-finalization automatic_after_quality_gates|require_human_review|disabled] [--dry-run]
  issue find --repo OWNER/REPO [--query TEXT] [--state open|closed|all] [--limit N] [--json]
  issue draft --repo OWNER/REPO [--prompt TEXT|--prompt-file PATH] --stack STACK [--framework nextjs|none] [--codex-draft] [--codex-review] [--json]
  issue proposals ISSUE_SET_ID
  issue submit ISSUE_SET_ID --proposal PROPOSAL_ID --yes [--resume] [--require-codex-review]
  issue mark ISSUE_SET_ID --proposal PROPOSAL_ID --dedupe unique|duplicate_blocked --reason TEXT
  repo inspect
  quality plan
  quality run RUN_ID
  vcs prepare --issue 123 [--repo OWNER/REPO]
  vcs list [--json]
  vcs show RUN_ID [--json]
  vcs status RUN_ID
  vcs diff RUN_ID [--output PATH]
  merge plan RUN_ID [--json]
  trace list
  trace show RUN_ID
  debug bundle RUN_ID
  memory status
  memory pressure
  finalize RUN_ID --approve [--resume]
  finalize RUN_ID --reject --reason REASON [--resume]
  menu [--json]
  completions bash|zsh|fish|powershell|elvish
  docs cli-markdown [--output PATH]
  mcp serve
  status

Specified milestone commands:
  daemon --config agentactr.toml # not implemented in this milestone
  run query --repo OWNER/REPO --label agentactr:ready --human-intervention fail-closed
                                 # not implemented in this milestone
  replay RUN_ID                  # not implemented in this milestone
  vcs commit RUN_ID              # not implemented in this milestone
  vcs cleanup RUN_ID             # not implemented in this milestone
  eval ...                       # not implemented in this milestone
"#
}

fn generated_help_path(args: &[String]) -> Option<Vec<String>> {
    match args {
        [] => Some(Vec::new()),
        [flag] if flag == "--help" || flag == "-h" => Some(Vec::new()),
        [command, rest @ ..] if command == "help" => {
            let path = strip_trailing_help_flag(rest);
            if rest.is_empty() {
                Some(Vec::new())
            } else if path.is_empty() {
                Some(vec!["help".to_string()])
            } else {
                Some(path.to_vec())
            }
        }
        args if args
            .last()
            .is_some_and(|arg| arg == "--help" || arg == "-h") =>
        {
            Some(args[..args.len() - 1].to_vec())
        }
        _ => None,
    }
}

fn strip_trailing_help_flag(args: &[String]) -> &[String] {
    if args
        .last()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        &args[..args.len() - 1]
    } else {
        args
    }
}

fn print_generated_help(path: &[String]) -> Result<(), String> {
    let text = render_generated_help(path)?;
    print!("{text}");
    if path.is_empty() {
        println!("\n{}", help_text());
    }
    Ok(())
}

fn render_generated_help(path: &[String]) -> Result<String, String> {
    let mut command = AgentactrHelpCli::command();
    let selected = find_generated_help_command(&mut command, path)?;
    let mut output = Vec::new();
    if !path.is_empty() {
        writeln!(output, "Command: agentactr {}", path.join(" "))
            .map_err(|e| format!("render help: {e}"))?;
        writeln!(output).map_err(|e| format!("render help: {e}"))?;
    }
    selected
        .write_long_help(&mut output)
        .map_err(|e| format!("render help: {e}"))?;
    output.push(b'\n');
    String::from_utf8(output).map_err(|e| format!("render help utf8: {e}"))
}

fn find_generated_help_command<'a>(
    command: &'a mut clap::Command,
    path: &[String],
) -> Result<&'a mut clap::Command, String> {
    let mut current = command;
    for segment in path {
        current = current
            .find_subcommand_mut(segment)
            .ok_or_else(|| format!("unknown help command `{segment}`"))?;
    }
    Ok(current)
}

#[derive(Clone, Copy)]
struct CommandCatalogEntry {
    command: &'static str,
    status: &'static str,
    purpose: &'static str,
    side_effects: &'static str,
    required_credentials: &'static [&'static str],
    platform_constraints: &'static [&'static str],
    sdk_use_case_owner: &'static str,
}

fn command_catalog() -> &'static [CommandCatalogEntry] {
    &[
        CommandCatalogEntry {
            command: "--version",
            status: "implemented",
            purpose: "Print the agentactr CLI version, build Git SHA, and compile-time rustc version.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "cli_operator_version",
        },
        CommandCatalogEntry {
            command: "commands [--json]",
            status: "implemented",
            purpose: "List CLI command inventory and implementation status.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "cli_operator_inventory",
        },
        CommandCatalogEntry {
            command: "help",
            status: "implemented",
            purpose: "Print top-level bootstrap help.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "cli_operator_help",
        },
        CommandCatalogEntry {
            command: "init --yes --repo OWNER/REPO [--codex-auth auto|chatgpt|api-key]",
            status: "implemented",
            purpose: "Create local agentactr, Codex, workflow, AGENTS.md when absent, and ignore configuration.",
            side_effects: "writes_config_files",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "config_rendering",
        },
        CommandCatalogEntry {
            command: "doctor [--fix-codex-config] [--fix-agents] [--trust-codex-project]",
            status: "implemented",
            purpose: "Inspect local config, credentials, adapters, runtime, domain graph, AGENTS policy, and platform readiness.",
            side_effects: "read_only_or_config_fix_when_requested",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "diagnostics",
        },
        CommandCatalogEntry {
            command: "config get [KEY]",
            status: "implemented",
            purpose: "Read effective local configuration.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "config_provider",
        },
        CommandCatalogEntry {
            command: "config set KEY VALUE",
            status: "implemented",
            purpose: "Persist a supported local configuration value.",
            side_effects: "writes_config_files",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "config_provider",
        },
        CommandCatalogEntry {
            command: "auth codex --method chatgpt|subscription|api-key [--api-key-env CODEX_API_KEY]",
            status: "implemented",
            purpose: "Configure Codex authentication mode for this repository.",
            side_effects: "writes_config_files",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "config_provider",
        },
        CommandCatalogEntry {
            command: "mcp serve",
            status: "implemented",
            purpose: "Run the local stdio MCP bridge for run-scoped read tools.",
            side_effects: "serves_stdio",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "mcp_bridge",
        },
        CommandCatalogEntry {
            command: "run issue --repo OWNER/REPO --issue 123 [--human-intervention fail-closed|interactive|review-required] [--codex-approval never|on-request] [--github-finalization automatic_after_quality_gates|require_human_review|disabled] [--dry-run]",
            status: "implemented",
            purpose: "Prepare worktree, fetch issue context, run Codex, run quality gates, and apply SDK-owned tracker lifecycle policy.",
            side_effects: "creates_worktree_artifacts_trace_sqlite_runs_runtime_and_may_mutate_tracker_lifecycle",
            required_credentials: &["github_token", "codex_auth_or_codex_api_key"],
            platform_constraints: &["git", "codex_cli_or_docker_backend"],
            sdk_use_case_owner: "run_issue",
        },
        CommandCatalogEntry {
            command: "issue find --repo OWNER/REPO [--query TEXT] [--state open|closed|all] [--label LABEL...] [--assignee USER|none|*] [--author USER] [--since ISO8601] [--sort created|updated|comments] [--direction asc|desc] [--page N] [--per-page N] [--limit N] [--artifact-root PATH] [--include-pull-requests] [--json]",
            status: "implemented",
            purpose: "Find existing tracker issues without running implementation agents or mutating GitHub.",
            side_effects: "creates_issue_set_artifacts",
            required_credentials: &["github_token"],
            platform_constraints: &["github_rest"],
            sdk_use_case_owner: "issue_discovery",
        },
        CommandCatalogEntry {
            command: "issue draft --repo OWNER/REPO [--prompt TEXT|--prompt-file PATH] --stack STACK [--framework nextjs|none] [--parent ISSUE_NUMBER] [--artifact-root PATH] [--codex-draft] [--codex-review] [--json]",
            status: "implemented",
            purpose: "Draft local issue proposals from repo evidence, deterministic prompt policy, or read-only Codex-authored structured output, optionally running a separate Codex review before GitHub submission.",
            side_effects: "creates_issue_set_artifacts_and_optional_codex_draft_or_review_artifacts",
            required_credentials: &["github_token", "codex_auth_when_codex_draft_or_review_enabled"],
            platform_constraints: &["github_rest_for_dedupe_inventory", "codex_cli_for_codex_draft_or_review"],
            sdk_use_case_owner: "issue_drafting",
        },
        CommandCatalogEntry {
            command: "issue proposals ISSUE_SET_ID",
            status: "implemented",
            purpose: "List issue-set proposals without mutating GitHub.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &["recorded_issue_set_or_run_artifacts"],
            sdk_use_case_owner: "issue_submission_review",
        },
        CommandCatalogEntry {
            command: "issue submit ISSUE_SET_ID --proposal PROPOSAL_ID --yes [--resume] [--allow-possible-duplicate --reason REASON] [--require-codex-review]",
            status: "implemented",
            purpose: "Submit or resume one review-gated issue proposal through capability-gated tracker ports.",
            side_effects: "writes_sqlite_and_may_mutate_tracker_when_adapter_supports_issue_create_link",
            required_credentials: &["github_token"],
            platform_constraints: &["recorded_issue_set_or_run_artifacts"],
            sdk_use_case_owner: "issue_submission",
        },
        CommandCatalogEntry {
            command: "issue mark ISSUE_SET_ID --proposal PROPOSAL_ID --dedupe unique|duplicate_blocked --reason REASON",
            status: "implemented",
            purpose: "Record a reviewed local dedupe decision for one issue proposal without mutating GitHub.",
            side_effects: "writes_issue_set_artifacts",
            required_credentials: &[],
            platform_constraints: &["recorded_issue_set_or_run_artifacts"],
            sdk_use_case_owner: "issue_dedupe_review",
        },
        CommandCatalogEntry {
            command: "repo inspect",
            status: "implemented",
            purpose: "Inspect repository stack and quality plan.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "repository_discovery",
        },
        CommandCatalogEntry {
            command: "quality plan",
            status: "implemented",
            purpose: "Print detected quality plan for the current repository.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "quality_planning",
        },
        CommandCatalogEntry {
            command: "quality run RUN_ID",
            status: "implemented",
            purpose: "Rerun quality gates in the recorded isolated worktree.",
            side_effects: "runs_local_commands_writes_quality_artifact_and_trace",
            required_credentials: &[],
            platform_constraints: &["recorded_worktree"],
            sdk_use_case_owner: "bootstrap_quality_rerun",
        },
        CommandCatalogEntry {
            command: "vcs prepare --issue 123 [--repo OWNER/REPO]",
            status: "implemented",
            purpose: "Prepare a local isolated Git worktree for an issue.",
            side_effects: "creates_git_worktree",
            required_credentials: &[],
            platform_constraints: &["git"],
            sdk_use_case_owner: "version_control_prepare_workspace",
        },
        CommandCatalogEntry {
            command: "vcs status RUN_ID",
            status: "implemented",
            purpose: "Read recorded run worktree status and touched files.",
            side_effects: "read_only_git_status_plus_trace_event",
            required_credentials: &[],
            platform_constraints: &["git", "recorded_worktree"],
            sdk_use_case_owner: "bootstrap_vcs_status",
        },
        CommandCatalogEntry {
            command: "vcs list [--json]",
            status: "implemented",
            purpose: "List retained local run worktrees from manifest artifacts.",
            side_effects: "read_only_local_inventory",
            required_credentials: &[],
            platform_constraints: &["git", "recorded_worktree"],
            sdk_use_case_owner: "bootstrap_vcs_inventory",
        },
        CommandCatalogEntry {
            command: "vcs show RUN_ID [--json]",
            status: "implemented",
            purpose: "Show detailed recorded VCS/worktree metadata for one run.",
            side_effects: "read_only_local_detail",
            required_credentials: &[],
            platform_constraints: &["git", "recorded_worktree"],
            sdk_use_case_owner: "bootstrap_vcs_inventory",
        },
        CommandCatalogEntry {
            command: "vcs diff RUN_ID [--output PATH]",
            status: "implemented",
            purpose: "Record a read-only workspace diff artifact for a run.",
            side_effects: "writes_diff_artifact_and_trace",
            required_credentials: &[],
            platform_constraints: &["git", "recorded_worktree"],
            sdk_use_case_owner: "version_control_diff",
        },
        CommandCatalogEntry {
            command: "merge plan RUN_ID [--json]",
            status: "implemented",
            purpose: "Record a read-only merge risk plan artifact for a run.",
            side_effects: "writes_merge_plan_artifact_and_trace",
            required_credentials: &[],
            platform_constraints: &["git", "recorded_worktree"],
            sdk_use_case_owner: "version_control_merge_plan",
        },
        CommandCatalogEntry {
            command: "trace list",
            status: "implemented",
            purpose: "Summarize run ids in the local JSONL event ledger.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "trace_inspection",
        },
        CommandCatalogEntry {
            command: "trace show RUN_ID",
            status: "implemented",
            purpose: "Show a run-scoped trace timeline and artifact integrity summary.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "trace_inspection",
        },
        CommandCatalogEntry {
            command: "debug bundle RUN_ID",
            status: "implemented",
            purpose: "Create a redacted local debug bundle for a run.",
            side_effects: "writes_debug_bundle",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "debug_bundle",
        },
        CommandCatalogEntry {
            command: "memory status",
            status: "implemented",
            purpose: "Print local memory controller status.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "memory_diagnostics",
        },
        CommandCatalogEntry {
            command: "memory pressure",
            status: "implemented",
            purpose: "Print local memory pressure observations.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "memory_diagnostics",
        },
        CommandCatalogEntry {
            command: "status",
            status: "implemented",
            purpose: "Print bootstrap CLI status.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "cli_status",
        },
        CommandCatalogEntry {
            command: "daemon --config agentactr.toml",
            status: "milestone",
            purpose: "Run the future scheduler/daemon.",
            side_effects: "not_implemented",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "scheduler_daemon",
        },
        CommandCatalogEntry {
            command: "run query --repo OWNER/REPO --label agentactr:ready --human-intervention fail-closed",
            status: "milestone",
            purpose: "Run issues selected from tracker query results.",
            side_effects: "not_implemented",
            required_credentials: &["github_token", "codex_auth_or_codex_api_key"],
            platform_constraints: &[],
            sdk_use_case_owner: "run_query",
        },
        CommandCatalogEntry {
            command: "replay RUN_ID",
            status: "milestone",
            purpose: "Rebuild run state from trace/artifacts and report divergence.",
            side_effects: "not_implemented",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "replay",
        },
        CommandCatalogEntry {
            command: "completions bash|zsh|fish|powershell|elvish",
            status: "implemented",
            purpose: "Generate shell completion scripts.",
            side_effects: "read_only_stdout",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "cli_completion_generation",
        },
        CommandCatalogEntry {
            command: "docs cli-markdown [--output PATH]",
            status: "implemented",
            purpose: "Generate Markdown CLI reference from the typed clap command tree and command catalog.",
            side_effects: "read_only_stdout_or_explicit_doc_write",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "cli_documentation_generation",
        },
        CommandCatalogEntry {
            command: "menu [--json]",
            status: "implemented",
            purpose: "Print a read-only command picker/setup navigator with exact commands.",
            side_effects: "read_only",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "cli_menu",
        },
        CommandCatalogEntry {
            command: "vcs commit RUN_ID",
            status: "milestone",
            purpose: "Create a local commit after quality gates.",
            side_effects: "not_implemented",
            required_credentials: &[],
            platform_constraints: &["git"],
            sdk_use_case_owner: "version_control_commit",
        },
        CommandCatalogEntry {
            command: "vcs cleanup RUN_ID",
            status: "milestone",
            purpose: "Remove retained local worktree after retention/approval policy.",
            side_effects: "not_implemented",
            required_credentials: &[],
            platform_constraints: &["git"],
            sdk_use_case_owner: "version_control_cleanup",
        },
        CommandCatalogEntry {
            command: "finalize RUN_ID --approve [--resume]",
            status: "implemented",
            purpose: "Approve and finalize tracker labels/comments/leases after review.",
            side_effects: "writes_sqlite_trace_artifacts_and_mutates_tracker_when_verified",
            required_credentials: &["github_token"],
            platform_constraints: &["recorded_run_artifacts", "github_rest"],
            sdk_use_case_owner: "finalization",
        },
        CommandCatalogEntry {
            command: "finalize RUN_ID --reject --reason REASON [--resume]",
            status: "implemented",
            purpose: "Reject finalization and record a human review reason.",
            side_effects: "writes_sqlite_trace_artifacts_and_mutates_tracker_when_verified",
            required_credentials: &["github_token"],
            platform_constraints: &["recorded_run_artifacts", "github_rest"],
            sdk_use_case_owner: "finalization",
        },
        CommandCatalogEntry {
            command: "eval swe-bench --subset verified-smoke",
            status: "milestone",
            purpose: "Run evaluation harnesses.",
            side_effects: "not_implemented",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "evaluation",
        },
    ]
}

fn cmd_commands(args: &mut [String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        None => {
            print_commands_text();
            Ok(())
        }
        Some("--json") => {
            print_commands_json()?;
            Ok(())
        }
        _ => Err("usage: agentactr commands [--json]".to_string()),
    }
}

fn print_commands_text() {
    println!("agentactr commands");
    for entry in command_catalog() {
        println!(
            "{}\tstatus={}\tside_effects={}\towner={}",
            entry.command, entry.status, entry.side_effects, entry.sdk_use_case_owner
        );
        println!("  {}", entry.purpose);
        if !entry.required_credentials.is_empty() {
            println!(
                "  required_credentials={}",
                entry.required_credentials.join(",")
            );
        }
        if !entry.platform_constraints.is_empty() {
            println!(
                "  platform_constraints={}",
                entry.platform_constraints.join(",")
            );
        }
    }
}

fn print_commands_json() -> Result<(), String> {
    let commands = command_catalog()
        .iter()
        .map(command_catalog_entry_json)
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "schema_version": "0.1",
        "commands": commands,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&payload)
            .map_err(|e| format!("render commands inventory: {e}"))?
    );
    Ok(())
}

fn command_catalog_entry_json(entry: &CommandCatalogEntry) -> serde_json::Value {
    serde_json::json!({
        "command": entry.command,
        "status": entry.status,
        "purpose": entry.purpose,
        "side_effects": entry.side_effects,
        "required_credentials": entry.required_credentials,
        "platform_constraints": entry.platform_constraints,
        "sdk_use_case_owner": entry.sdk_use_case_owner,
    })
}

fn cmd_completions(args: &mut [String]) -> Result<(), String> {
    let shell = args
        .get(1)
        .ok_or_else(|| "usage: agentactr completions bash|zsh|fish|powershell|elvish".to_string())
        .and_then(|value| parse_completion_shell(value))?;
    if args.len() != 2 {
        return Err("usage: agentactr completions bash|zsh|fish|powershell|elvish".to_string());
    }
    let script = completion_script(shell)?;
    print!("{script}");
    Ok(())
}

fn parse_completion_shell(value: &str) -> Result<Shell, String> {
    match value {
        "bash" => Ok(Shell::Bash),
        "zsh" => Ok(Shell::Zsh),
        "fish" => Ok(Shell::Fish),
        "powershell" => Ok(Shell::PowerShell),
        "elvish" => Ok(Shell::Elvish),
        other => Err(format!(
            "unsupported completion shell `{other}`; expected bash|zsh|fish|powershell|elvish"
        )),
    }
}

fn completion_script(shell: Shell) -> Result<String, String> {
    let mut command = AgentactrHelpCli::command();
    let mut output = Vec::new();
    generate(shell, &mut command, "agentactr", &mut output);
    String::from_utf8(output).map_err(|e| format!("render completions utf8: {e}"))
}

fn cmd_docs(args: &mut [String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("cli-markdown") => cmd_docs_cli_markdown(args),
        _ => Err("usage: agentactr docs cli-markdown [--output PATH]".to_string()),
    }
}

fn cmd_docs_cli_markdown(args: &[String]) -> Result<(), String> {
    let mut output_path = None;
    let mut index = 2;
    while index < args.len() {
        match args.get(index).map(String::as_str) {
            Some("--output") => {
                let value = args
                    .get(index + 1)
                    .ok_or("usage: agentactr docs cli-markdown [--output PATH]")?;
                output_path = Some(PathBuf::from(value));
                index += 2;
            }
            _ => return Err("usage: agentactr docs cli-markdown [--output PATH]".to_string()),
        }
    }

    let markdown = render_cli_markdown()?;
    if let Some(path) = output_path {
        write_cli_markdown_output(&path, &markdown)?;
    } else {
        print!("{markdown}");
    }
    Ok(())
}

fn write_cli_markdown_output(path: &Path, markdown: &str) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("docs output path must not be empty".to_string());
    }
    if path.exists() {
        let metadata =
            fs::symlink_metadata(path).map_err(|e| format!("inspect {}: {e}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "docs output path must not be a symlink: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            return Err(format!(
                "docs output path must not be a directory: {}",
                path.display()
            ));
        }
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create docs output directory {}: {e}", parent.display()))?;
    }
    fs::write(path, markdown).map_err(|e| format!("write {}: {e}", path.display()))
}

fn render_cli_markdown() -> Result<String, String> {
    let mut output = String::new();
    output.push_str("# agentactr CLI Reference\n\n");
    output.push_str("<!-- Generated by `agentactr docs cli-markdown`; do not edit command sections by hand. -->\n\n");
    output.push_str(
        "This reference is generated from the typed `clap` command tree and the bootstrap command catalog. ",
    );
    output.push_str(
        "The catalog supplies implementation status, side effects, required credentials, platform constraints, and SDK use-case ownership.\n\n",
    );
    output.push_str("## Commands\n\n");

    for (help_path, entries) in command_catalog_help_groups() {
        output.push_str(&format!(
            "### `{}`\n\n",
            markdown_help_heading_command(&help_path)
        ));
        output.push_str("| Catalog command | Status | Side effects | SDK owner | Required credentials | Platform constraints | Purpose |\n");
        output.push_str("| --- | --- | --- | --- | --- | --- | --- |\n");
        for entry in entries {
            output.push_str(&format!(
                "| `{}` | `{}` | `{}` | `{}` | `{}` | `{}` | {} |\n",
                markdown_table_cell(&format!("agentactr {}", entry.command)),
                markdown_table_cell(entry.status),
                markdown_table_cell(entry.side_effects),
                markdown_table_cell(entry.sdk_use_case_owner),
                markdown_table_cell(&markdown_list_or_none(entry.required_credentials)),
                markdown_table_cell(&markdown_list_or_none(entry.platform_constraints)),
                markdown_table_cell(entry.purpose)
            ));
        }
        output.push('\n');

        let help = render_generated_help(&help_path)?;
        output.push_str("#### Generated Help\n\n");
        output.push_str("```text\n");
        output.push_str(help.trim_end());
        output.push_str("\n```\n\n");
    }

    Ok(output)
}

fn command_catalog_help_groups() -> Vec<(Vec<String>, Vec<&'static CommandCatalogEntry>)> {
    let mut groups: Vec<(Vec<String>, Vec<&CommandCatalogEntry>)> = Vec::new();
    for entry in command_catalog() {
        let help_path = catalog_help_path(entry.command);
        if let Some((_, entries)) = groups.iter_mut().find(|(path, _)| path == &help_path) {
            entries.push(entry);
        } else {
            groups.push((help_path, vec![entry]));
        }
    }
    groups
}

fn markdown_help_heading_command(help_path: &[String]) -> String {
    if help_path.is_empty() {
        "agentactr".to_string()
    } else {
        format!("agentactr {}", help_path.join(" "))
    }
}

fn markdown_table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

fn markdown_list_or_none(values: &[&str]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(",")
    }
}

fn catalog_help_path(command: &str) -> Vec<String> {
    let mut root = AgentactrHelpCli::command();
    let mut path = Vec::new();
    let mut current = &mut root;
    for token in command.split_whitespace() {
        let Some(next) = current.find_subcommand_mut(token) else {
            break;
        };
        path.push(token.to_string());
        current = next;
    }
    path
}

fn cmd_menu(args: &mut [String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        None => {
            print_menu_text();
            Ok(())
        }
        Some("--json") => {
            print_menu_json()?;
            Ok(())
        }
        _ => Err("usage: agentactr menu [--json]".to_string()),
    }
}

fn print_menu_text() {
    println!("agentactr menu");
    println!("Bootstrap read-only navigator. Run the exact command shown for an action.");
    for (index, entry) in command_catalog().iter().enumerate() {
        println!(
            "{:>2}. agentactr {}  [{}; side_effects={}]",
            index + 1,
            entry.command,
            entry.status,
            entry.side_effects
        );
        println!("    {}", entry.purpose);
    }
}

fn print_menu_json() -> Result<(), String> {
    let payload = menu_json_payload();
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).map_err(|e| format!("render menu: {e}"))?
    );
    Ok(())
}

fn menu_json_payload() -> serde_json::Value {
    let actions = command_catalog()
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let mut value = command_catalog_entry_json(entry);
            if let Some(object) = value.as_object_mut() {
                object.insert("index".to_string(), serde_json::json!(index + 1));
                object.insert(
                    "equivalent_command".to_string(),
                    serde_json::json!(format!("agentactr {}", entry.command)),
                );
                object.insert("executes".to_string(), serde_json::json!(false));
            }
            value
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "schema_version": "0.1",
        "mode": "bootstrap_read_only",
        "automation_surface": "agentactr commands --json",
        "actions": actions,
    })
}

fn cmd_init(args: &mut [String]) -> Result<(), String> {
    let repo = flag_value(args, "--repo").unwrap_or_else(|| "OWNER/REPO".to_string());
    let yes = has_flag(args, "--yes");
    if !yes {
        return Err("init is fail-closed by default; pass --yes to write files".to_string());
    }

    let auth_mode = flag_value(args, "--codex-auth")
        .map(|v| CodexAuthMode::parse(&v))
        .transpose()?
        .unwrap_or(CodexAuthMode::Auto);
    let mut config = AgentactrConfig::strict_defaults(repo);
    config.codex.auth_mode = auth_mode;

    let creds = detect_credentials(&config);

    create_dir(".codex")?;
    create_dir(".agentactr")?;
    create_dir(".agentactr/runs")?;
    create_dir(".agentactr/artifacts")?;
    create_dir(".agentactr/debug")?;
    create_dir(".agentactr/workspaces")?;
    create_dir(".agentactr/worktrees")?;

    write_file("agentactr.toml", &render_agentactr_toml(&config))?;
    write_file(
        ".codex/config.toml",
        &render_codex_config_toml(&config, &creds),
    )?;
    write_file("WORKFLOW.md", &render_workflow_md())?;
    let inspection = configured_repo_inspection(Path::new("."), &config);
    write_agents_if_absent(&config, &inspection)?;
    append_gitignore(&render_gitignore_additions())?;

    println!("wrote agentactr.toml");
    println!("wrote .codex/config.toml");
    println!("wrote WORKFLOW.md");
    if Path::new("AGENTS.md").exists() {
        println!("ensured AGENTS.md");
    }
    println!("updated .gitignore");
    print_mcp_summary(&creds);
    Ok(())
}

fn cmd_doctor(args: &mut [String]) -> Result<(), String> {
    let fix = has_flag(args, "--fix-codex-config");
    let fix_agents = has_flag(args, "--fix-agents");
    let trust_codex_project = has_flag(args, "--trust-codex-project");
    let config = load_agentactr_config(None)?;
    let creds = detect_credentials(&config);
    let mut fixed_codex_user_config = None;
    let execution_decision = resolve_execution_backend(&config.execution).ok();

    if fix {
        create_dir(".codex")?;
        write_file(
            ".codex/config.toml",
            &render_codex_config_toml(&config, &creds),
        )?;
    }

    if trust_codex_project {
        fixed_codex_user_config = Some(trust_current_codex_project()?);
    }

    println!("agentactr doctor");
    check_path("agentactr.toml");
    check_path(".codex/config.toml");
    check_path("WORKFLOW.md");
    if execution_decision
        .as_ref()
        .map(should_probe_host_codex)
        .unwrap_or(true)
    {
        check_command("agentactr", &["--help"]);
        check_command(&config.codex.command, &["--version"]);
    } else {
        println!("ok: host agentactr/Codex probes skipped for Docker Linux execution backend");
    }
    check_command("git", &["--version"]);
    if execution_decision
        .as_ref()
        .map(should_probe_host_codex)
        .unwrap_or(true)
    {
        check_codex_login_status(&config.codex.command, &config.codex.openai_api_key_env);
        check_codex_project_trust(Path::new("."));
    } else {
        println!("ok: host Codex auth/trust probes skipped; Docker runtime image is authoritative");
    }
    if check_codex_transport(&config) == Some(CodexMode::CliJsonExec) {
        if execution_decision
            .as_ref()
            .map(should_probe_host_codex)
            .unwrap_or(true)
        {
            check_codex_exec_capacity(&config);
        } else {
            println!(
                "ok: host Codex exec capacity probe skipped for Docker Linux execution backend"
            );
        }
    }
    check_env(&config.tracker.token_env, "GitHub token");
    check_optional_env("GH_TOKEN", "alternate GitHub token");
    check_optional_env(
        &config.codex.openai_api_key_env,
        "Codex API key for codex exec",
    );
    check_optional_env(
        "OPENAI_API_KEY",
        "OpenAI API key for codex login --with-api-key",
    );
    check_optional_env("GOOGLE_API_KEY", "Google Developer API key");
    check_optional_env("HF_TOKEN", "Hugging Face token");
    check_github_token_governance(&config);
    check_github_api_version(&config);
    check_github_lifecycle_labels(&config);
    check_sqlite_store(&config);
    check_otlp(&config);
    check_workspace_permissions(&config);
    print_mcp_summary(&creds);
    print_security_summary(&config);
    print_memory_status();
    print_execution_status(&config);
    print_doctor_adapter_versions(&config)?;
    let inspection = configured_repo_inspection(Path::new("."), &config);
    if fix_agents {
        write_agents_if_absent_or_artifact(&config, &inspection)?;
    }
    print_domain_summary(&config, &inspection);
    print_repo_inspection(&inspection);

    if fix {
        println!("fixed .codex/config.toml");
        println!("Codex project trust was not changed by --fix-codex-config");
        println!("To explicitly trust this project, run `agentactr doctor --trust-codex-project`");
    }
    if trust_codex_project {
        if let Some(path) = fixed_codex_user_config {
            println!("fixed Codex project trust in {}", path.display());
        }
    }
    Ok(())
}

fn write_agents_if_absent(
    config: &AgentactrConfig,
    inspection: &RepoInspection,
) -> Result<(), String> {
    match config.templates.agents_policy.as_str() {
        "disabled" => return Ok(()),
        "artifact_only" => return write_agents_review_artifact(config, inspection).map(|_| ()),
        "generate_when_absent" => {}
        other => return Err(format!("unsupported templates.agents_policy `{other}`")),
    }
    if Path::new("AGENTS.md").exists() {
        return Ok(());
    }
    write_file("AGENTS.md", &render_agents_md(config, inspection))
}

fn write_agents_if_absent_or_artifact(
    config: &AgentactrConfig,
    inspection: &RepoInspection,
) -> Result<(), String> {
    match config.templates.agents_policy.as_str() {
        "disabled" => {
            println!("templates.agents_policy=disabled; skipped AGENTS.md generation");
            write_domain_artifacts(config, inspection)?;
            return Ok(());
        }
        "artifact_only" => {
            let generated = write_agents_review_artifact(config, inspection)?;
            println!("wrote AGENTS.md review artifact {}", generated.display());
            write_domain_artifacts(config, inspection)?;
            return Ok(());
        }
        "generate_when_absent" => {}
        other => return Err(format!("unsupported templates.agents_policy `{other}`")),
    }
    if !Path::new("AGENTS.md").exists() {
        write_file("AGENTS.md", &render_agents_md(config, inspection))?;
        println!("fixed AGENTS.md");
        write_domain_artifacts(config, inspection)?;
        return Ok(());
    }
    let generated = write_agents_review_artifact(config, inspection)?;
    println!(
        "AGENTS.md already exists; wrote review artifact {}",
        generated.display()
    );
    write_domain_artifacts(config, inspection)?;
    Ok(())
}

fn write_agents_review_artifact(
    config: &AgentactrConfig,
    inspection: &RepoInspection,
) -> Result<PathBuf, String> {
    let artifact_dir = PathBuf::from(&config.observability.artifact_root).join("doctor");
    fs::create_dir_all(&artifact_dir)
        .map_err(|e| format!("create doctor artifact dir {}: {e}", artifact_dir.display()))?;
    let generated = artifact_dir.join("AGENTS.md.generated");
    fs::write(&generated, render_agents_md(config, inspection))
        .map_err(|e| format!("write {}: {e}", generated.display()))?;
    Ok(generated)
}

fn write_domain_artifacts(
    config: &AgentactrConfig,
    inspection: &RepoInspection,
) -> Result<(), String> {
    let graph_path = resolve_config_path(&config.architecture.domain_graph_artifact)?;
    let artifact_dir = graph_path.parent().ok_or_else(|| {
        format!(
            "invalid domain graph artifact path {}",
            graph_path.display()
        )
    })?;
    fs::create_dir_all(artifact_dir)
        .map_err(|e| format!("create domain artifact dir {}: {e}", artifact_dir.display()))?;
    let findings_path = artifact_dir.join("domain_findings.json");
    let quality_path = artifact_dir.join("domain_quality_plan.json");
    let graph_json = domain_graph_to_json(&inspection.domain_graph);
    let findings = domain_findings(&inspection.root);
    let findings_json = domain_findings_to_json(&findings);
    let quality_json = domain_quality_plan_to_json(&inspection.domain_quality_plan);
    fs::write(
        &graph_path,
        serde_json::to_string_pretty(&graph_json)
            .map_err(|e| format!("render {}: {e}", graph_path.display()))?,
    )
    .map_err(|e| format!("write {}: {e}", graph_path.display()))?;
    fs::write(
        &findings_path,
        serde_json::to_string_pretty(&findings_json)
            .map_err(|e| format!("render {}: {e}", findings_path.display()))?,
    )
    .map_err(|e| format!("write {}: {e}", findings_path.display()))?;
    fs::write(
        &quality_path,
        serde_json::to_string_pretty(&quality_json)
            .map_err(|e| format!("render {}: {e}", quality_path.display()))?,
    )
    .map_err(|e| format!("write {}: {e}", quality_path.display()))?;
    println!("wrote domain graph artifact {}", graph_path.display());
    println!("wrote domain findings artifact {}", findings_path.display());
    println!("wrote domain quality artifact {}", quality_path.display());
    Ok(())
}

fn cmd_config(args: &mut [String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err("usage: agentactr config get [KEY] | set KEY VALUE".to_string());
    }
    match args[1].as_str() {
        "get" => {
            let content = fs::read_to_string("agentactr.toml")
                .map_err(|e| format!("read agentactr.toml: {e}"))?;
            if args.len() == 2 {
                print!("{content}");
                return Ok(());
            }
            let key = &args[2];
            if let Some(value) = find_config_value(&content, key) {
                println!("{value}");
                Ok(())
            } else {
                Err(format!("key not found: {key}"))
            }
        }
        "set" => {
            if args.len() < 4 {
                return Err("usage: agentactr config set KEY VALUE".to_string());
            }
            set_config_value("agentactr.toml", &args[2], &args[3])?;
            println!("updated {}", args[2]);
            Ok(())
        }
        other => Err(format!("unknown config subcommand `{other}`")),
    }
}

fn cmd_auth(args: &mut [String]) -> Result<(), String> {
    if args.get(1).map(String::as_str) != Some("codex") {
        return Err(
            "usage: agentactr auth codex --method chatgpt|subscription|api-key".to_string(),
        );
    }
    let config = load_agentactr_config(None)?;
    let method = flag_value(args, "--method").unwrap_or_else(|| "chatgpt".to_string());
    match method.as_str() {
        "chatgpt" | "subscription" => run_status(Command::new(&config.codex.command).arg("login")),
        "api-key" | "api_key" => {
            let env_name =
                flag_value(args, "--api-key-env").unwrap_or(config.codex.openai_api_key_env);
            env::var(&env_name).map_err(|_| {
                format!("missing {env_name}; set it before running API-key Codex auth")
            })?;
            println!(
                "ok: {env_name} is set; codex exec will use API-key auth without a stored login"
            );
            Ok(())
        }
        other => Err(format!("unsupported auth method `{other}`")),
    }
}

fn cmd_mcp(args: &mut [String]) -> Result<(), String> {
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

fn handle_mcp_json_rpc(request: &str) -> Option<String> {
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
        "agentactr.run.status" => Ok("run_status: no active in-process run registry in this milestone".to_string()),
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

fn mcp_trace_text(arguments: &serde_json::Value) -> String {
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

fn agentactr_artifact_root(arguments: &serde_json::Value) -> Result<std::path::PathBuf, String> {
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

fn agentactr_trace_path(arguments: &serde_json::Value) -> std::path::PathBuf {
    arguments
        .get("trace_path")
        .and_then(serde_json::Value::as_str)
        .map(std::path::PathBuf::from)
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

fn resolve_config_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()
            .map_err(|e| format!("resolve current directory: {e}"))?
            .join(path))
    }
}

fn validate_run_id(run_id: &str) -> Result<(), String> {
    if run_id.is_empty() {
        return Err("invalid RUN_ID: value must not be empty".to_string());
    }
    if run_id.contains('/') || run_id.contains('\\') {
        return Err(format!(
            "invalid RUN_ID `{run_id}`: path separators are not allowed"
        ));
    }
    if !run_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(format!(
            "invalid RUN_ID `{run_id}`: expected only ASCII letters, digits, '.', '_' or '-'"
        ));
    }
    if run_id == "." || run_id == ".." {
        return Err(format!(
            "invalid RUN_ID `{run_id}`: relative path segments are not allowed"
        ));
    }
    Ok(())
}

fn run_artifact_dir(config: &AgentactrConfig, run_id: &str) -> Result<PathBuf, String> {
    validate_run_id(run_id)?;
    Ok(resolve_config_path(&config.observability.artifact_root)?.join(run_id))
}

fn run_trace_path(config: &AgentactrConfig) -> Result<PathBuf, String> {
    resolve_config_path(&config.observability.jsonl)
}

fn debug_bundle_dir(config: &AgentactrConfig, run_id: &str) -> Result<PathBuf, String> {
    validate_run_id(run_id)?;
    Ok(resolve_config_path(&config.observability.debug_bundle_root)?.join(run_id))
}

pub(crate) fn current_epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn timestamp_rfc3339_millis() -> String {
    iso_timestamp_from_epoch_millis(current_epoch_millis())
}

pub(crate) fn iso_timestamp_from_epoch_millis(epoch_millis: u128) -> String {
    let epoch_seconds = (epoch_millis / 1_000).min(u64::MAX as u128) as u64;
    let millis = (epoch_millis % 1_000) as u32;
    let days = (epoch_seconds / 86_400) as i64;
    let seconds_of_day = epoch_seconds % 86_400;
    let (year, month, day) = civil_from_unix_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_unix_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };

    (year as i32, month as u32, day as u32)
}

struct RunEventContext<'a> {
    config: &'a AgentactrConfig,
    run_id: &'a str,
    repo: &'a str,
    issue: &'a str,
    agent_run_id: Option<&'a str>,
    parent_agent_run_id: Option<&'a str>,
}

impl<'a> RunEventContext<'a> {
    fn root(config: &'a AgentactrConfig, run_id: &'a str, repo: &'a str, issue: &'a str) -> Self {
        Self {
            config,
            run_id,
            repo,
            issue,
            agent_run_id: None,
            parent_agent_run_id: None,
        }
    }

    fn agent(
        config: &'a AgentactrConfig,
        run_id: &'a str,
        repo: &'a str,
        issue: &'a str,
        agent_run_id: &'a str,
        parent_agent_run_id: Option<&'a str>,
    ) -> Self {
        Self {
            config,
            run_id,
            repo,
            issue,
            agent_run_id: Some(agent_run_id),
            parent_agent_run_id,
        }
    }
}

struct RunContextArtifactInput<'a> {
    event: RunEventContext<'a>,
    agent_run_id: &'a str,
    worktree_ref: &'a agentactr_sdk::WorktreeRef,
    artifact_dir: &'a Path,
    trace_path: &'a Path,
    memory: &'a MemoryRunContext,
    child_memory: &'a [ChildMemoryAssignment],
    issue_context: &'a agentactr_sdk::Issue,
    inspection: &'a RepoInspection,
    stack_source: &'a RepositoryStackSource,
    run_policy: &'a RunPolicy,
    spawn_plan: &'a SpawnPlan,
}

#[derive(Clone, Debug)]
struct ChildMemoryAssignment {
    agent_run_id: String,
    lease: MemoryLease,
    group: PathBuf,
}

impl ChildMemoryAssignment {
    fn lease_for_request(&self) -> AgentMemoryLease {
        AgentMemoryLease {
            agent_run_id: self.agent_run_id.clone(),
            lease: self.lease.clone(),
        }
    }
}

fn append_run_event(
    context: &RunEventContext<'_>,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    let trace_path = run_trace_path(context.config)?;
    if let Some(parent) = trace_path.parent() {
        create_dir(parent)?;
    }
    let ts_unix_ms = current_epoch_millis();
    let event = serde_json::json!({
        "schema_version": "0.1",
        "ts": iso_timestamp_from_epoch_millis(ts_unix_ms),
        "ts_unix_ms": ts_unix_ms,
        "run_id": context.run_id,
        "issue_id": format!("github:{}#{}", context.repo, context.issue),
        "agent_run_id": context.agent_run_id,
        "parent_agent_run_id": context.parent_agent_run_id,
        "event_type": event_type,
        "span_id": format!("span:{}:{event_type}", context.run_id),
        "parent_span_id": serde_json::Value::Null,
        "payload": payload,
    });
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&trace_path)
        .map_err(|e| format!("open trace events {}: {e}", trace_path.display()))?;
    writeln!(file, "{event}")
        .map_err(|e| format!("write trace event {}: {e}", trace_path.display()))
}

fn write_run_context_artifacts(input: &RunContextArtifactInput<'_>) -> Result<PathBuf, String> {
    let context_manifest = input.artifact_dir.join("context_manifest.json");
    let quality_plan = input
        .inspection
        .quality_plan
        .iter()
        .map(|cmd| {
            serde_json::json!({
                "name": cmd.name,
                "command": cmd.command,
                "required": cmd.required,
                "non_mutating_final_gate": cmd.non_mutating_final_gate,
            })
        })
        .collect::<Vec<_>>();
    let spawn_children = input
        .spawn_plan
        .child_nodes
        .iter()
        .map(|node| {
            let child_memory = child_memory_group(input.child_memory, node.agent_run_id.as_str());
            serde_json::json!({
                "agent_run_id": node.agent_run_id.as_str(),
                "parent_agent_run_id": node.parent_agent_run_id.as_ref().map(|id| id.as_str()),
                "role": node.role.as_str(),
                "objective": node.objective,
                "workspace": input.worktree_ref.path.display().to_string(),
                "artifact_dir": node.artifact_dir.display().to_string(),
                "write_scope": "read_only",
                "memory_group": child_memory.map(|assignment| assignment.group.display().to_string()),
                "memory_group_id": child_memory.map(|assignment| assignment.lease.group_id.as_str().to_string()),
            })
        })
        .collect::<Vec<_>>();
    let spawn_decisions = input
        .spawn_plan
        .decisions
        .iter()
        .map(|record| {
            serde_json::json!({
                "role": record.role.as_str(),
                "objective": record.objective,
                "action": format!("{:?}", record.decision.action),
                "reason": format!("{:?}", record.decision.reason),
            })
        })
        .collect::<Vec<_>>();
    let manifest = serde_json::json!({
        "schema_version": "0.1",
        "run_id": input.event.run_id,
        "agent_run_id": input.agent_run_id,
        "repo": input.event.repo,
        "issue": input.event.issue,
        "worktree": {
            "path": input.worktree_ref.path.display().to_string(),
            "base_commit": input.worktree_ref.base_commit.clone(),
            "run_id": input.worktree_ref.run_id.clone(),
        },
        "artifacts": {
            "run_dir": input.artifact_dir.display().to_string(),
            "trace_path": input.trace_path.display().to_string(),
            "memory_status": input.memory.status_artifact.display().to_string(),
            "agent_graph": input.artifact_dir.join("agent_graph.json").display().to_string(),
            "spawn_handoffs": input.artifact_dir.join("spawn_handoffs.json").display().to_string(),
            "runtime_process_events": input
                .artifact_dir
                .join("runtime_process_events.jsonl")
                .display()
                .to_string(),
            "codex_prompt": input.artifact_dir.join("codex.prompt.txt").display().to_string(),
            "codex_prompt_metadata": input
                .artifact_dir
                .join("codex.prompt.metadata.json")
                .display()
                .to_string(),
            "codex_stdout_jsonl": input.artifact_dir.join("codex.stdout.jsonl").display().to_string(),
            "codex_stderr_log": input.artifact_dir.join("codex.stderr.log").display().to_string(),
            "github_issue_json": input
                .issue_context
                .source_artifact
                .as_ref()
                .map(|path| path.display().to_string()),
            "github_rate_limit_events": input
                .artifact_dir
                .join("github_rate_limit_events.jsonl")
                .display()
                .to_string(),
            "github_rate_limit_log": input
                .artifact_dir
                .join("github_issue.rate_limit.log")
                .display()
                .to_string(),
            "repository_context": input
                .artifact_dir
                .join("repository_context.json")
                .display()
                .to_string(),
            "adapter_version_reports": input
                .artifact_dir
                .join("adapter_version_reports.json")
                .display()
                .to_string(),
            "quality_report": input.artifact_dir.join("quality_report.txt").display().to_string(),
            "quality_status": input
                .artifact_dir
                .join("quality_report.status.json")
                .display()
                .to_string(),
            "github_lifecycle_events": input
                .artifact_dir
                .join("github_lifecycle_events.jsonl")
                .display()
                .to_string(),
            "workspace_diff": input.artifact_dir.join("workspace.diff.patch").display().to_string(),
            "workspace_diff_metadata": input
                .artifact_dir
                .join("workspace.diff.metadata.json")
                .display()
                .to_string(),
            "merge_plan": input.artifact_dir.join("merge_plan.json").display().to_string(),
            "merge_plan_metadata": input
                .artifact_dir
                .join("merge_plan.metadata.json")
                .display()
                .to_string(),
            "finalization_status": input
                .artifact_dir
                .join("finalization_status.json")
                .display()
                .to_string(),
        },
        "policy": {
            "human_intervention": input.run_policy.human_intervention_config_value(),
            "codex_approval": input.run_policy.codex_approval_config_value(),
            "github_finalization_requested": input.run_policy.github_finalization_text(),
            "github_finalization": input
                .run_policy
                .github_finalization_effective_text(&GithubRestAdapter::bootstrap_capabilities()),
        },
        "repository": {
            "detected_stack": input.inspection.detected_stack.as_str(),
            "selected_stack": input.inspection.primary_stack.as_str(),
            "stack_source": input.stack_source.as_display(),
            "selected_quality_profile": input.inspection.selected_quality_profile.as_str(),
            "confidence": input.inspection.confidence,
            "is_empty": input.inspection.is_empty,
            "evidence_files": input.inspection.evidence_files.clone(),
            "missing_prerequisites": input.inspection.missing_prerequisites.clone(),
            "setup_guidance": input.inspection.setup_guidance.clone(),
        },
        "quality_plan": quality_plan,
        "spawn": {
            "mode": if spawn_children.is_empty() { "single_writer" } else { "one_writer_parallel_read_only_helpers" },
            "writer_agent_run_id": input.spawn_plan.writer_agent_run_id.as_str(),
            "child_count": spawn_children.len(),
            "handoff_manifest": input.artifact_dir.join("spawn_handoffs.json").display().to_string(),
            "children": spawn_children,
            "decisions": spawn_decisions,
        },
        "memory": {
            "mode": if input.memory.enforce { "enforce" } else { "observe" },
            "run_group": input.memory.run_group.as_ref().map(|path| path.display().to_string()),
            "agent_group": input.memory.agent_group.as_ref().map(|path| path.display().to_string()),
            "agent_group_id": input
                .memory
                .agent_group_id
                .as_ref()
                .map(|id| id.as_str()),
            "status_artifact": input.memory.status_artifact.display().to_string(),
        },
        "mcp_context": {
            "required_server": "agentactr",
            "env_vars": [
                "AGENTACTR_ARTIFACT_ROOT",
                "AGENTACTR_REPO_ROOT",
                "AGENTACTR_RUN_ID",
                "AGENTACTR_AGENT_RUN_ID",
                "AGENTACTR_TRACE_PATH",
                "AGENTACTR_CONTEXT_MANIFEST"
            ],
            "read_tools": [
                "agentactr.issue.read",
                "agentactr.run.status",
                "agentactr.trace.read",
                "agentactr.artifact.read",
                "agentactr.vcs.status",
                "agentactr.quality.report",
                "agentactr.memory.status",
                "agentactr.policy.read"
            ]
        }
    });
    write_repository_context_artifact(input.artifact_dir, input.inspection, input.stack_source)?;
    write_file(
        &context_manifest,
        &serde_json::to_string_pretty(&manifest)
            .map_err(|e| format!("render context manifest: {e}"))?,
    )?;
    let mut agent_nodes = vec![serde_json::json!({
        "agent_run_id": input.agent_run_id,
        "parent_agent_run_id": serde_json::Value::Null,
        "role": "Implementer",
        "runtime": "codex",
        "workspace": input.worktree_ref.path.display().to_string(),
        "artifact_dir": input.artifact_dir.display().to_string(),
        "context_manifest": context_manifest.display().to_string(),
        "write_scope": "repo",
        "memory_group": input
            .memory
            .agent_group
            .as_ref()
            .map(|path| path.display().to_string()),
        "memory_group_id": input
            .memory
            .agent_group_id
            .as_ref()
            .map(|id| id.as_str())
    })];
    agent_nodes.extend(input.spawn_plan.child_nodes.iter().map(|node| {
        let child_memory = child_memory_group(input.child_memory, node.agent_run_id.as_str());
        serde_json::json!({
            "agent_run_id": node.agent_run_id.as_str(),
            "parent_agent_run_id": node.parent_agent_run_id.as_ref().map(|id| id.as_str()),
            "role": node.role.as_str(),
            "runtime": "codex",
            "workspace": input.worktree_ref.path.display().to_string(),
            "artifact_dir": node.artifact_dir.display().to_string(),
            "context_manifest": context_manifest.display().to_string(),
            "write_scope": "read_only",
            "memory_group": child_memory.map(|assignment| assignment.group.display().to_string()),
            "memory_group_id": child_memory.map(|assignment| assignment.lease.group_id.as_str().to_string())
        })
    }));
    let agent_graph = serde_json::json!({
        "schema_version": "0.1",
        "run_id": input.event.run_id,
        "spawn": {
            "mode": if input.spawn_plan.child_nodes.is_empty() { "single_writer" } else { "one_writer_parallel_read_only_helpers" },
            "reason": "provider-neutral SpawnManager plan"
        },
        "nodes": agent_nodes,
        "spawn_policy": {
            "enabled": input.event.config.spawn.enabled,
            "max_child_agents_per_issue": input.event.config.spawn.max_child_agents_per_issue,
            "max_spawn_depth": input.event.config.spawn.max_spawn_depth,
            "allow_parallel_read_only": input.event.config.spawn.allow_parallel_read_only,
            "allow_parallel_writers": input.event.config.spawn.allow_parallel_writers,
            "strategy": input.event.config.spawn.strategy,
            "max_total_uncached_input_tokens": input.event.config.spawn.max_total_uncached_input_tokens,
            "max_child_uncached_input_tokens": input.event.config.spawn.max_child_uncached_input_tokens,
            "max_child_output_tokens": input.event.config.spawn.max_child_output_tokens,
            "artifact_handoff": input.event.config.spawn.artifact_handoff,
            "pause_on_memory_pressure": input.event.config.spawn.pause_on_memory_pressure
        }
    });
    write_file(
        input.artifact_dir.join("agent_graph.json"),
        &serde_json::to_string_pretty(&agent_graph)
            .map_err(|e| format!("render agent graph: {e}"))?,
    )?;
    append_run_event(
        &input.event,
        "context.manifest.written",
        serde_json::json!({
            "context_manifest": context_manifest.display().to_string(),
            "agent_graph": input.artifact_dir.join("agent_graph.json").display().to_string(),
        }),
    )?;
    println!(
        "context artifacts: manifest={} agent_graph={} memory_status={}",
        context_manifest.display(),
        input.artifact_dir.join("agent_graph.json").display(),
        input.memory.status_artifact.display()
    );
    println!(
        "agents: count={} active={} mode={} writer=Implementer read_only_helpers={} spawn_enabled={} max_child_agents={}",
        1 + input.spawn_plan.child_nodes.len(),
        1 + input.spawn_plan.child_nodes.len(),
        if input.spawn_plan.child_nodes.is_empty() { "single_writer" } else { "parallel_read_only_helpers" },
        input.spawn_plan.child_nodes.len(),
        input.event.config.spawn.enabled,
        input.event.config.spawn.max_child_agents_per_issue
    );
    Ok(context_manifest)
}

fn write_repository_context_artifact(
    artifact_dir: &Path,
    inspection: &RepoInspection,
    stack_source: &RepositoryStackSource,
) -> Result<PathBuf, String> {
    let path = artifact_dir.join("repository_context.json");
    let payload = serde_json::json!({
        "schema_version": "0.1",
        "detected_stack": inspection.detected_stack.as_str(),
        "selected_stack": inspection.primary_stack.as_str(),
        "stack_source": stack_source.as_display(),
        "selected_quality_profile": inspection.selected_quality_profile.as_str(),
        "confidence": inspection.confidence,
        "is_empty": inspection.is_empty,
        "evidence_files": inspection.evidence_files.clone(),
        "missing_prerequisites": inspection.missing_prerequisites.clone(),
        "setup_guidance": inspection.setup_guidance.clone(),
    });
    write_file(
        &path,
        &serde_json::to_string_pretty(&payload)
            .map_err(|e| format!("render repository context artifact: {e}"))?,
    )?;
    Ok(path)
}

fn child_memory_group<'a>(
    assignments: &'a [ChildMemoryAssignment],
    agent_run_id: &str,
) -> Option<&'a ChildMemoryAssignment> {
    assignments
        .iter()
        .find(|assignment| assignment.agent_run_id == agent_run_id)
}

fn build_spawn_plan(
    config: &AgentactrConfig,
    run_id: &str,
    writer_agent_run_id: &str,
    artifact_dir: &Path,
) -> SpawnPlan {
    agentactr_sdk::default_spawn_plan(agentactr_sdk::DefaultSpawnPlanRequest {
        config,
        run_id,
        writer_agent_run_id,
        artifact_root: artifact_dir,
    })
}

struct CliRunIssueHooks<'a> {
    config: &'a AgentactrConfig,
    run_id: &'a str,
    repo: &'a str,
    issue: &'a str,
    agent_run_id: &'a str,
    artifact_dir: &'a Path,
    trace_path: &'a Path,
    memory_controller: &'a LinuxMemoryController,
    memory_context: &'a MemoryRunContext,
    child_memory: &'a [ChildMemoryAssignment],
    inspection: &'a RepoInspection,
    stack_source: &'a RepositoryStackSource,
    run_policy: &'a RunPolicy,
    spawn_plan: &'a SpawnPlan,
}

impl RunIssueHooks for CliRunIssueHooks<'_> {
    fn phase_started(&mut self, phase: &str) -> Result<(), String> {
        append_run_event(
            &RunEventContext::root(self.config, self.run_id, self.repo, self.issue),
            "phase.started",
            serde_json::json!({ "phase": phase }),
        )
    }

    fn phase_completed(&mut self, phase: &str) -> Result<(), String> {
        append_run_event(
            &RunEventContext::root(self.config, self.run_id, self.repo, self.issue),
            "phase.completed",
            serde_json::json!({ "phase": phase }),
        )
    }

    fn phase_failed(&mut self, phase: &str, error: &str) -> Result<(), String> {
        let status = format!("{phase}_failed");
        record_run_state(
            self.config,
            self.run_id,
            self.repo,
            self.issue,
            &status,
            self.artifact_dir,
        )?;
        append_run_event(
            &RunEventContext::root(self.config, self.run_id, self.repo, self.issue),
            "phase.failed",
            serde_json::json!({
                "phase": phase,
                "error": error,
            }),
        )
    }

    fn before_runtime(
        &mut self,
        context: RunIssueRuntimeContext<'_>,
    ) -> Result<AgentIssueRunRequest, String> {
        println!("prepared worktree: {}", context.worktree.path.display());
        println!("fetched GitHub issue: {}#{}", self.repo, self.issue);
        let _ = emit_github_rate_limit_trace_events(
            self.config,
            self.run_id,
            self.repo,
            self.issue,
            self.artifact_dir,
        );
        let agent_event_context = RunEventContext::agent(
            self.config,
            self.run_id,
            self.repo,
            self.issue,
            self.agent_run_id,
            None,
        );
        let context_manifest = write_run_context_artifacts(&RunContextArtifactInput {
            event: agent_event_context,
            agent_run_id: self.agent_run_id,
            worktree_ref: context.worktree,
            artifact_dir: self.artifact_dir,
            trace_path: self.trace_path,
            memory: self.memory_context,
            child_memory: self.child_memory,
            issue_context: context.issue_context,
            inspection: self.inspection,
            stack_source: self.stack_source,
            run_policy: self.run_policy,
            spawn_plan: self.spawn_plan,
        })?;
        print_quality_plan(self.inspection);
        println!("launching Codex:");
        println!(
            "  {} exec --json -c <repo-local Codex defaults> --sandbox {} -c approval_policy=\"{}\" --cd {} <prompt>",
            self.config.codex.command,
            self.config.codex.sandbox_mode,
            self.run_policy.codex_approval_cli_value(),
            context.worktree.path.display()
        );
        let execution_decision = resolve_execution_backend(&self.config.execution)?;
        require_codex_project_config_ready(
            &context.worktree.path,
            &self.config.codex.profile,
            &execution_decision,
        )?;
        append_run_event(
            &RunEventContext::agent(
                self.config,
                self.run_id,
                self.repo,
                self.issue,
                self.agent_run_id,
                None,
            ),
            "agent.started",
            serde_json::json!({
                "role": "Implementer",
                "runtime": "codex",
                "worktree": context.worktree.path.display().to_string(),
                "artifact_dir": self.artifact_dir.display().to_string(),
                "context_manifest": context_manifest.display().to_string(),
                "write_scope": "repo",
            }),
        )?;
        Ok(AgentIssueRunRequest {
            run_id: self.run_id.to_string(),
            agent_run_id: self.agent_run_id.to_string(),
            parent_agent_run_id: None,
            role: "Implementer".to_string(),
            objective: format!("Implement GitHub issue {}#{}", self.repo, self.issue),
            write_scope: "repo".to_string(),
            worktree: context.worktree.path.clone(),
            artifact_dir: self.artifact_dir.to_path_buf(),
            trace_path: self.trace_path.to_path_buf(),
            context_manifest,
            memory: self.memory_context.agent_memory_lease(),
            child_memory: self
                .child_memory
                .iter()
                .map(ChildMemoryAssignment::lease_for_request)
                .collect(),
            repo: self.repo.to_string(),
            issue: self.issue.to_string(),
            issue_context: context.issue_context.clone(),
            approval_policy: self.run_policy.as_runtime_policy(),
            spawn_plan: Some(self.spawn_plan.clone()),
        })
    }

    fn after_runtime_success(
        &mut self,
        context: RunIssuePostRuntimeContext<'_>,
    ) -> Result<(), String> {
        self.memory_controller.sample_agent(self.memory_context)?;
        println!(
            "memory: mode={} status_artifact={}",
            if self.memory_context.enforce {
                "enforce"
            } else {
                "observe"
            },
            self.memory_context.status_artifact.display()
        );
        append_run_event(
            &RunEventContext::agent(
                self.config,
                self.run_id,
                self.repo,
                self.issue,
                self.agent_run_id,
                None,
            ),
            "agent.completed",
            serde_json::json!({
                "role": "Implementer",
                "stdout_jsonl": context.runtime_report.stdout_jsonl.display().to_string(),
                "stderr_log": context.runtime_report.stderr_log.display().to_string(),
            }),
        )
    }
}

struct CodexRuntimeMemoryInput<'a> {
    config: &'a AgentactrConfig,
    memory_controller: &'a LinuxMemoryController,
    memory_context: &'a MemoryRunContext,
    run_id: &'a str,
    repo: &'a str,
    issue: &'a str,
    artifact_dir: &'a Path,
    trace_path: &'a Path,
    spawn_plan: &'a SpawnPlan,
}

fn codex_runtime_with_memory(
    input: CodexRuntimeMemoryInput<'_>,
) -> Result<(CodexRuntimeAdapter, Vec<ChildMemoryAssignment>), String> {
    let mut memory_groups = HashMap::new();
    if let (Some(lease), Some(path)) = (
        input.memory_context.agent_memory_lease(),
        input.memory_context.agent_group.clone(),
    ) {
        memory_groups.insert(lease.group_id, path);
    }

    let mut child_memory = Vec::new();
    for node in &input.spawn_plan.child_nodes {
        if let Some(prepared) = input.memory_controller.prepare_child_agent(
            input.memory_context,
            input.run_id,
            node.agent_run_id.as_str(),
            &node.artifact_dir,
        )? {
            memory_groups.insert(prepared.lease.group_id.clone(), prepared.group.clone());
            child_memory.push(ChildMemoryAssignment {
                agent_run_id: node.agent_run_id.as_str().to_string(),
                lease: prepared.lease,
                group: prepared.group,
            });
        }
    }
    let runtime = codex_runtime_adapter(input.config)?.with_process_supervisor(Arc::new(
        CliCodexMemorySupervisor::new(
            memory_groups,
            input.artifact_dir.to_path_buf(),
            input.trace_path.to_path_buf(),
            input.repo.to_string(),
            input.issue.to_string(),
        ),
    ));
    Ok((runtime, child_memory))
}

fn cmd_run(args: &mut [String]) -> Result<(), String> {
    if args.get(1).map(String::as_str) == Some("query") {
        return not_implemented("run query");
    }
    if args.get(1).map(String::as_str) != Some("issue") {
        return Err(RUN_ISSUE_USAGE.to_string());
    }
    validate_run_issue_args(args)?;
    let repo_override = flag_value(args, "--repo");
    let issue = flag_value(args, "--issue").ok_or("missing --issue")?;
    let dry_run = has_flag(args, "--dry-run");
    let config = load_agentactr_config(repo_override.as_deref())?;
    let repo = config.tracker.repo.clone();
    validate_github_repo(&repo)?;
    validate_issue_number(&issue)?;
    let run_policy = RunPolicy::from_config_and_args(&config, args)?;
    let tracker_capabilities = GithubRestAdapter::bootstrap_capabilities();
    print_run_banner(&repo, &issue, &run_policy, &tracker_capabilities);
    println!("Policy source detail: shown per setting in the run banner");
    let repo_context = effective_repository_context(
        configured_repo_inspection(Path::new("."), &config),
        &config,
        &repo,
        &issue,
    )?;
    print_repo_inspection(&repo_context.inspection);
    println!(
        "  stack_source = {}",
        repo_context.stack_source.as_display()
    );
    let _ = io::stdout().flush();
    fail_on_blocking_repo_findings(&repo_context)?;
    let execution_decision = resolve_execution_backend(&config.execution)?;
    println!("preflight: Codex availability");
    require_codex_availability(&config, &execution_decision)?;
    println!("preflight: GitHub token");
    let github_token_envs = preferred_github_token_env_names(&config.tracker.token_env);
    let github_token_env_refs = github_token_envs
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    require_env_any(&github_token_env_refs, "GitHub token")?;
    println!("preflight: MCP policy");
    let creds = detect_credentials(&config);
    print_mcp_summary(&creds);
    require_mcp_policy_ready(&execution_decision)?;
    println!("preflight: Linux memory");
    let memory_controller = LinuxMemoryController::new(&config.linux_memory);
    for line in memory_controller.status_lines() {
        println!("{line}");
    }
    println!("preflight: Execution backend");
    require_execution_backend_ready(&config)?;
    if dry_run {
        print_quality_plan(&repo_context.inspection);
        println!("dry run: skipped GitHub fetch, worktree creation, and Codex launch");
        return Ok(());
    }
    println!("preflight: VCS source checkout");
    let vcs = LocalGitAdapter;
    vcs.preflight_source_checkout(&config.vcs)?;
    let run_id = new_run_id(&issue);
    let agent_run_id = format!("agent-{run_id}-implementer");
    let artifact_dir = run_artifact_dir(&config, &run_id)?;
    let trace_path = run_trace_path(&config)?;
    create_dir(&artifact_dir)?;
    if let Some(parent) = trace_path.parent() {
        create_dir(parent)?;
    }
    record_run_state(&config, &run_id, &repo, &issue, "started", &artifact_dir)?;
    let lease_heartbeat = LocalLeaseHeartbeat::start(&config, &run_id, &repo, &issue);
    let run_result = (|| {
        let github = GithubRestAdapter::new(artifact_dir.clone(), &config.tracker);
        let memory_context = record_phase(
            &config,
            &run_id,
            &repo,
            &issue,
            &artifact_dir,
            "memory",
            || {
                let context =
                    memory_controller.prepare_run(&run_id, &agent_run_id, &artifact_dir)?;
                append_run_event(
                    &RunEventContext::agent(&config, &run_id, &repo, &issue, &agent_run_id, None),
                    "memory.cgroup.created",
                    memory_controller.trace_payload(&context),
                )?;
                Ok(context)
            },
        )?;
        let spawn_plan = build_spawn_plan(&config, &run_id, &agent_run_id, &artifact_dir);
        let (runtime, child_memory) = codex_runtime_with_memory(CodexRuntimeMemoryInput {
            config: &config,
            memory_controller: &memory_controller,
            memory_context: &memory_context,
            run_id: &run_id,
            repo: &repo,
            issue: &issue,
            artifact_dir: &artifact_dir,
            trace_path: &trace_path,
            spawn_plan: &spawn_plan,
        })?;
        let adapter_reports = print_adapter_versions(&vcs, &github, &runtime);
        record_adapter_version_reports(
            &config,
            &run_id,
            &repo,
            &issue,
            &artifact_dir,
            &adapter_reports,
        )?;
        let vcs_adapter: Arc<dyn VersionControl> = Arc::new(LocalGitAdapter);
        let github_adapter: Arc<dyn IssueTracker> = Arc::new(github);
        let runtime_adapter: Arc<dyn AgentRuntime> = Arc::new(runtime);
        let use_cases = AgentActrBuilder::new()
            .version_control(vcs_adapter)
            .issue_tracker(github_adapter)
            .runtime(runtime_adapter)
            .build()?;
        let issue_number = issue
            .parse::<u64>()
            .map_err(|e| format!("parse GitHub issue number {issue}: {e}"))?;
        let lifecycle_mode = lifecycle_mode_for_run_policy(&run_policy);
        let lifecycle_labels = lifecycle_labels_from_config(&config);
        if lifecycle_mode != IssueLifecycleMode::Disabled {
            let claim = use_cases.claim_issue(agentactr_sdk::ClaimRequest {
                repo: repo.clone(),
                issue_number,
                run_id: run_id.clone(),
                owner_id: format!("{}:{}", std::process::id(), agent_run_id),
                fencing_token: sha256_hex_bytes(format!("{run_id}:{agent_run_id}").as_bytes()),
                lease_expires_at: iso_timestamp_from_epoch_millis(
                    current_epoch_millis() + u128::from(config.scheduling.lease_ttl_ms),
                ),
                claim_label: config.tracker.claim_label.clone(),
                running_label: config.tracker.running_label.clone(),
                ignore_labels: config.tracker.ignore_labels.clone(),
                allow_pull_request: false,
            })?;
            write_lifecycle_event(
                &config,
                &run_id,
                &repo,
                &issue,
                &artifact_dir,
                "github.lifecycle.claimed",
                serde_json::json!({
                    "accepted": claim.accepted,
                    "verification_status": claim.verification_status,
                    "detail": claim.detail,
                    "artifacts": claim.source_artifacts.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
                }),
            )?;
            if !claim.accepted {
                record_finalization_status(
                    &config,
                    &run_id,
                    &repo,
                    &issue,
                    &artifact_dir,
                    "claim_not_acquired",
                )?;
                record_run_state(
                    &config,
                    &run_id,
                    &repo,
                    &issue,
                    "claim_not_acquired",
                    &artifact_dir,
                )?;
                return Err(
                    "GitHub issue claim was not accepted; stopping before runtime".to_string(),
                );
            }
        }
        let mut hooks = CliRunIssueHooks {
            config: &config,
            run_id: &run_id,
            repo: &repo,
            issue: &issue,
            agent_run_id: &agent_run_id,
            artifact_dir: &artifact_dir,
            trace_path: &trace_path,
            memory_controller: &memory_controller,
            memory_context: &memory_context,
            child_memory: &child_memory,
            inspection: &repo_context.inspection,
            stack_source: &repo_context.stack_source,
            run_policy: &run_policy,
            spawn_plan: &spawn_plan,
        };
        let run_report = use_cases.run_issue(
            RunIssueRequest {
                issue_id: IssueId(format!("{repo}#{issue}")),
                worktree: WorktreeRequest {
                    run_id: run_id.clone(),
                    repo: repo.clone(),
                    issue: issue.clone(),
                    base_ref: config.vcs.base_ref.clone(),
                    worktree_root: PathBuf::from(&config.vcs.worktree_root),
                    branch_template: config.vcs.branch_template.clone(),
                    // Runtime state is written after the source checkout preflight.
                    fail_on_dirty_source_checkout: false,
                    copy_runtime_config_to_worktree: config.vcs.copy_runtime_config_to_worktree,
                },
            },
            &mut hooks,
        );
        if run_report.is_err() {
            let _ = memory_controller.sample_agent(&memory_context);
            println!(
                "memory: mode={} status_artifact={}",
                if memory_context.enforce {
                    "enforce"
                } else {
                    "observe"
                },
                memory_context.status_artifact.display()
            );
            if let Err(err) = &run_report {
                let outcome = RunOutcomeSummary {
                    run_id: run_id.clone(),
                    repo: repo.clone(),
                    issue_number,
                    runtime_success: false,
                    quality: QualityGateSummary {
                        success: false,
                        report_path: None,
                        failed_reason: Some(err.clone()),
                    },
                    artifact_dir: artifact_dir.clone(),
                };
                if let Ok(report) = use_cases.apply_issue_lifecycle(IssueLifecycleRequest {
                    mode: if lifecycle_mode == IssueLifecycleMode::Disabled {
                        IssueLifecycleMode::Disabled
                    } else {
                        IssueLifecycleMode::Failure {
                            reason: err.clone(),
                        }
                    },
                    outcome,
                    labels: lifecycle_labels.clone(),
                    summary: format!(
                        "agentactr run `{run_id}` failed before quality gates.\n\nArtifacts: `{}`",
                        artifact_dir.display()
                    ),
                }) {
                    let _ = record_lifecycle_report(
                        &config,
                        &run_id,
                        &repo,
                        &issue,
                        &artifact_dir,
                        "github.lifecycle.failed",
                        &report,
                    );
                }
            }
        }
        let run_report = run_report?;
        let worktree_path = run_report.worktree.path.clone();
        let quality_report_path = artifact_dir.join("quality_report.txt");
        let quality_result = record_phase(
            &config,
            &run_id,
            &repo,
            &issue,
            &artifact_dir,
            "quality",
            || {
                run_quality_gates_to_report(
                    &repo_context.inspection,
                    &worktree_path,
                    &quality_report_path,
                    &config.quality.domain_gate_opt_ins,
                )
            },
        );
        let quality_summary = match &quality_result {
            Ok(()) => QualityGateSummary {
                success: true,
                report_path: Some(quality_report_path.clone()),
                failed_reason: None,
            },
            Err(err) => QualityGateSummary {
                success: false,
                report_path: Some(quality_report_path.clone()),
                failed_reason: Some(err.clone()),
            },
        };
        let outcome = RunOutcomeSummary {
            run_id: run_id.clone(),
            repo: repo.clone(),
            issue_number,
            runtime_success: true,
            quality: quality_summary,
            artifact_dir: artifact_dir.clone(),
        };
        let lifecycle_summary = recorded_run_lifecycle_summary(&run_id, &artifact_dir);
        if let Err(err) = &quality_result {
            if let Ok(report) = use_cases.apply_issue_lifecycle(IssueLifecycleRequest {
                mode: if lifecycle_mode == IssueLifecycleMode::Disabled {
                    IssueLifecycleMode::Disabled
                } else {
                    IssueLifecycleMode::Failure {
                        reason: err.clone(),
                    }
                },
                outcome: outcome.clone(),
                labels: lifecycle_labels.clone(),
                summary: lifecycle_summary.clone(),
            }) {
                let _ = record_lifecycle_report(
                    &config,
                    &run_id,
                    &repo,
                    &issue,
                    &artifact_dir,
                    "github.lifecycle.failed",
                    &report,
                );
            }
            record_run_state(
                &config,
                &run_id,
                &repo,
                &issue,
                "quality_failed",
                &artifact_dir,
            )?;
            return Err(err.clone());
        }
        let lifecycle_report = use_cases.apply_issue_lifecycle(IssueLifecycleRequest {
            mode: lifecycle_mode.clone(),
            outcome,
            labels: lifecycle_labels,
            summary: lifecycle_summary,
        })?;
        record_lifecycle_report(
            &config,
            &run_id,
            &repo,
            &issue,
            &artifact_dir,
            "github.lifecycle.finalization",
            &lifecycle_report,
        )?;
        record_run_state(
            &config,
            &run_id,
            &repo,
            &issue,
            &lifecycle_report.status,
            &artifact_dir,
        )?;
        Ok::<_, String>(())
    })();
    lease_heartbeat.stop()?;
    run_result?;
    println!("run id: {run_id}");
    println!("artifact dir: {}", artifact_dir.display());
    println!(
        "GitHub lifecycle: {}",
        run_policy.github_finalization_effective_text(&GithubRestAdapter::bootstrap_capabilities())
    );
    Ok(())
}

fn cmd_issue(args: &mut [String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("find") => cmd_issue_find(args),
        Some("draft") => cmd_issue_draft(args),
        Some("proposals") => {
            let issue_set_id = args
                .get(2)
                .ok_or("usage: agentactr issue proposals ISSUE_SET_ID")?;
            let config = load_agentactr_config(None)?;
            let context = load_issue_set_context(&config, issue_set_id)?;
            let proposals = load_issue_proposals(&context)?;
            if proposals.is_empty() {
                println!(
                    "no issue proposals found for issue set {}; expected {}",
                    issue_set_id,
                    issue_proposals_path(&context).display()
                );
                return Ok(());
            }
            println!("issue proposals for issue set {issue_set_id}:");
            for proposal in proposals {
                let parent = proposal
                    .parent_issue
                    .map(|issue| format!("#{issue}"))
                    .unwrap_or_else(|| "none".to_string());
                println!(
                    "  {} repo={} parent={} dedupe={} title={} digest={}",
                    proposal.proposal_id.as_str(),
                    proposal.repo,
                    parent,
                    proposal.dedupe.as_str(),
                    proposal.title,
                    proposal.digest
                );
            }
            Ok(())
        }
        Some("submit") => cmd_issue_submit(args),
        Some("mark") => cmd_issue_mark(args),
        _ => Err(
            "usage: agentactr issue find --repo OWNER/REPO | agentactr issue draft --repo OWNER/REPO [--prompt TEXT] --stack STACK [--codex-draft] [--codex-review] | agentactr issue proposals ISSUE_SET_ID | agentactr issue submit ISSUE_SET_ID --proposal PROPOSAL_ID --yes [--resume] [--require-codex-review] | agentactr issue mark ISSUE_SET_ID --proposal PROPOSAL_ID --dedupe unique|duplicate_blocked --reason TEXT"
                .to_string(),
        ),
    }
}

fn cmd_finalize(args: &mut [String]) -> Result<(), String> {
    let run_id = args
        .get(1)
        .ok_or("usage: agentactr finalize RUN_ID --approve [--resume] | agentactr finalize RUN_ID --reject --reason REASON [--resume]")?;
    let approve = args.iter().any(|arg| arg == "--approve");
    let reject = args.iter().any(|arg| arg == "--reject");
    let resume = args.iter().any(|arg| arg == "--resume");
    if approve == reject {
        return Err("finalize requires exactly one of --approve or --reject".to_string());
    }
    let reject_reason = if reject {
        Some(flag_value(args, "--reason").ok_or("finalize --reject requires --reason REASON")?)
    } else {
        None
    };
    let config = load_agentactr_config(None)?;
    let context = load_run_artifact_context(&config, run_id)?;
    let issue_number = context
        .issue
        .parse::<u64>()
        .map_err(|e| format!("parse issue number {}: {e}", context.issue))?;
    let tracker = GithubRestAdapter::new(&context.artifact_dir, &config.tracker);
    let decision = if approve {
        FinalizeDecision::Approve
    } else {
        FinalizeDecision::Reject
    };
    let artifacts = FsRunFinalizationArtifacts::new(context.artifact_dir.clone());
    let finalized = finalize_recorded_run_with_tracker(
        &tracker,
        &artifacts,
        RecordedRunFinalizationRequest {
            run_id: run_id.to_string(),
            repo: context.repo.clone(),
            issue_number,
            decision,
            reject_reason,
            labels: lifecycle_labels_from_config(&config),
            resume,
        },
    )?;
    write_lifecycle_event(
        &config,
        run_id,
        &context.repo,
        &context.issue,
        &context.artifact_dir,
        "github.lifecycle.finalize",
        serde_json::json!({
            "status": finalized.lifecycle.status,
            "resume": resume,
            "decision": if approve { "approve" } else { "reject" },
            "prior_status": finalized.prior_status,
            "release": finalized.lifecycle.release.as_ref().map(|release| serde_json::json!({
                "verification_status": release.verification_status,
                "final_issue_state": release.final_issue_state,
                "state_reason": release.state_reason,
                "artifacts": release.source_artifacts.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
            })),
        }),
    )?;
    record_finalization_status(
        &config,
        run_id,
        &context.repo,
        &context.issue,
        &context.artifact_dir,
        &finalized.lifecycle.status,
    )?;
    record_run_state(
        &config,
        run_id,
        &context.repo,
        &context.issue,
        &finalized.lifecycle.status,
        &context.artifact_dir,
    )?;
    println!("run id: {run_id}");
    println!("finalization_status={}", finalized.lifecycle.status);
    println!(
        "finalization_artifact={}",
        context
            .artifact_dir
            .join("finalization_status.json")
            .display()
    );
    Ok(())
}

fn cmd_issue_find(args: &mut [String]) -> Result<(), String> {
    let repo = flag_value(args, "--repo").ok_or("missing --repo OWNER/REPO for issue find")?;
    validate_github_repo(&repo)?;
    let mut config = load_agentactr_config(Some(&repo))?;
    apply_issue_artifact_root_override(&mut config, args)?;
    let query = parse_candidate_query(args, &repo)?;
    let issue_set_id = new_issue_set_id("find");
    let context = create_issue_set_context(
        &config,
        &issue_set_id,
        &repo,
        None,
        None,
        IssueSetSource::Find,
    )?;
    let tracker = GithubRestAdapter::new(&context.artifact_dir, &config.tracker);
    let candidates = tracker.fetch_candidates(query)?;
    write_issue_set_manifest(&context, &config, None)?;
    write_issue_candidates(&context, &candidates)?;
    if has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "issue_set_id": context.issue_set_id,
                "artifact_dir": context.artifact_dir,
                "candidate_count": candidates.len(),
                "manifest_path": context.manifest_path,
                "candidates_path": context.candidates_path,
            }))
            .map_err(|e| format!("render issue find JSON: {e}"))?
        );
    } else {
        println!("issue_set_id={}", context.issue_set_id);
        println!("artifact_dir={}", context.artifact_dir.display());
        println!("candidate_count={}", candidates.len());
    }
    Ok(())
}

fn cmd_issue_draft(args: &mut [String]) -> Result<(), String> {
    let repo = flag_value(args, "--repo").ok_or("missing --repo OWNER/REPO for issue draft")?;
    validate_github_repo(&repo)?;
    let mut config = load_agentactr_config(Some(&repo))?;
    apply_issue_artifact_root_override(&mut config, args)?;
    let query = parse_candidate_query(args, &repo)?;
    let issue_set_id = new_issue_set_id("draft");
    let parent_issue = flag_value(args, "--parent")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("invalid --parent `{value}`"))
        })
        .transpose()?;
    let framework = parse_framework_declaration(flag_value(args, "--framework").as_deref())?;
    let prompt = load_issue_draft_prompt(args)?;
    let local_inspection = configured_repo_inspection(Path::new("."), &config);
    let stack = flag_value(args, "--stack")
        .or_else(|| selected_repo_stack_name(&config))
        .or_else(|| discovered_repo_stack_name(&local_inspection))
        .filter(|stack| stack != "unknown");
    if stack.is_none() {
        return Err(
            "issue draft requires --stack or repository.declared_primary_stack before writing proposals"
                .to_string(),
        );
    }
    if prompt.is_none()
        && (local_inspection.is_empty
            || local_inspection.evidence_files.is_empty()
            || local_inspection.confidence < 50)
    {
        return Err(
            "issue draft without --prompt requires local repository evidence; blank projects must pass --prompt and --stack"
                .to_string(),
        );
    }
    let mut context = create_issue_set_context(
        &config,
        &issue_set_id,
        &repo,
        parent_issue,
        framework.clone(),
        IssueSetSource::Draft,
    )?;
    let tracker = GithubRestAdapter::new(&context.artifact_dir, &config.tracker);
    let candidates = tracker.fetch_candidates(query.clone())?;
    let prompt_artifacts = prompt
        .as_ref()
        .map(|prompt| write_prompt_artifacts(&context, prompt))
        .transpose()?;
    if let Some(artifacts) = prompt_artifacts.as_ref() {
        context.planner_prompt_path = Some(artifacts.redacted_path.clone());
        context.planner_metadata_path = Some(context.artifact_dir.join("planner_metadata.json"));
        let _ = &artifacts.metadata_path;
    }
    let draft_request = agentactr_sdk::IssueDraftRequest {
        issue_set_id: context.issue_set_id.clone(),
        repo: repo.clone(),
        parent_issue,
        prompt: prompt.as_ref().map(|prompt| redact_prompt(prompt)),
        framework: framework.clone(),
        stack: stack.clone(),
        candidates: candidates.clone(),
        query,
    };
    let codex_draft = if has_flag(args, "--codex-draft") {
        if prompt.is_none() {
            return Err("--codex-draft requires --prompt or --prompt-file so the LLM draft has explicit operator intent".to_string());
        }
        Some(run_codex_issue_draft_planner(
            &config,
            &context,
            &draft_request,
            prompt.as_deref().unwrap_or_default(),
            stack.as_deref().unwrap_or("unknown"),
            &local_inspection,
        )?)
    } else {
        None
    };
    let draft = if let Some(codex_draft) = codex_draft.as_ref() {
        agentactr_sdk::draft_issue_proposals_from_structured_json(
            draft_request,
            &fs::read_to_string(&codex_draft.response_path).map_err(|e| {
                format!(
                    "read Codex issue draft response {}: {e}",
                    codex_draft.response_path.display()
                )
            })?,
            "codex_read_only_structured_issue_draft_planner",
        )?
    } else {
        let planner = agentactr_sdk::DeterministicIssueDraftPlanner;
        planner.draft(draft_request)?
    };
    let codex_review = if has_flag(args, "--codex-review") {
        Some(run_codex_issue_draft_review(
            &config,
            &context,
            &draft.proposals,
            stack.as_deref().unwrap_or("unknown"),
        )?)
    } else {
        None
    };
    write_planner_metadata(
        &context,
        &draft,
        if codex_draft.is_some() {
            "codex_read_only_structured_issue_draft_planner"
        } else {
            "agentactr-sdk-deterministic-issue-draft-planner"
        },
    )?;
    write_issue_set_manifest(&context, &config, stack.as_deref())?;
    write_issue_candidates(&context, &candidates)?;
    write_issue_proposals(&context, &draft.proposals)?;
    write_issue_dedupe_report(&context, &draft.proposals, &candidates)?;
    materialize_issue_submission_pending(&config, &context.issue_set_id, &draft.proposals)?;
    if has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "issue_set_id": context.issue_set_id,
                "artifact_dir": context.artifact_dir,
                "candidate_count": candidates.len(),
                "proposal_count": draft.proposals.len(),
                "manifest_path": context.manifest_path,
                "proposals_path": context.proposals_path,
                "planner_prompt_path": context.planner_prompt_path,
                "planner_metadata_path": context.planner_metadata_path,
                "codex_draft_status_path": codex_draft.as_ref().map(|draft| draft.status_path.clone()),
                "codex_draft_response_path": codex_draft.as_ref().map(|draft| draft.response_path.clone()),
                "codex_review_status_path": codex_review.as_ref().map(|review| review.status_path.clone()),
                "codex_review_markdown_path": codex_review.as_ref().map(|review| review.review_path.clone()),
            }))
            .map_err(|e| format!("render issue draft JSON: {e}"))?
        );
    } else {
        println!("issue_set_id={}", context.issue_set_id);
        println!("artifact_dir={}", context.artifact_dir.display());
        println!("candidate_count={}", candidates.len());
        println!("proposal_count={}", draft.proposals.len());
        if let Some(artifacts) = prompt_artifacts {
            println!("prompt_artifact={}", artifacts.redacted_path.display());
        }
        if let Some(draft) = codex_draft {
            println!("codex_draft_status={}", draft.status_path.display());
            println!("codex_draft_response={}", draft.response_path.display());
        }
        if let Some(review) = codex_review {
            println!("codex_review_status={}", review.status_path.display());
            println!("codex_review={}", review.review_path.display());
        }
    }
    Ok(())
}

fn cmd_issue_submit(args: &mut [String]) -> Result<(), String> {
    let issue_set_id = args.get(2).ok_or(
        "usage: agentactr issue submit ISSUE_SET_ID --proposal PROPOSAL_ID --yes [--resume] [--require-codex-review]",
    )?;
    if !has_flag(args, "--yes") {
        return Err(
            "issue submit is review-gated; pass --yes after reviewing the proposal".to_string(),
        );
    }
    let proposal_id =
        flag_value(args, "--proposal").ok_or("missing --proposal PROPOSAL_ID for issue submit")?;
    let resume = has_flag(args, "--resume");
    let allow_possible_duplicate = has_flag(args, "--allow-possible-duplicate");
    let duplicate_reason = flag_value(args, "--reason");
    let config = load_agentactr_config(None)?;
    let context = load_issue_set_context(&config, issue_set_id)?;
    let proposal = load_issue_proposals(&context)?
        .into_iter()
        .find(|proposal| proposal.proposal_id.as_str() == proposal_id)
        .ok_or_else(|| {
            format!(
                "proposal {proposal_id} not found in {}",
                issue_proposals_path(&context).display()
            )
        })?;
    if has_flag(args, "--require-codex-review") {
        require_codex_review_for_proposal(&context, &proposal_id)?;
    }
    agentactr_sdk::validate_issue_submission_policy(
        &proposal,
        allow_possible_duplicate,
        duplicate_reason.as_deref(),
    )?;
    let ledger_entry = load_issue_submission_ledger(&config, issue_set_id, &proposal)?;
    let decision = agentactr_sdk::plan_issue_submission(
        issue_set_id,
        proposal.clone(),
        ledger_entry.as_ref(),
        resume,
    );
    match decision {
        IssueSubmissionDecision::AlreadyLinked => {
            println!("proposal {proposal_id} is already linked");
            Ok(())
        }
        IssueSubmissionDecision::Blocked(reason) => Err(reason),
        IssueSubmissionDecision::RecoverSubmitted(req) => {
            let tracker = GithubRestAdapter::new_with_github(
                &context.artifact_dir,
                &config.tracker,
                &config.github,
            );
            let recovered = tracker.recover_created_issue_by_marker(&req)?;
            let Some(create) = recovered else {
                return Err(
                    "submitted issue request is uncertain and no created issue marker was found; inspect GitHub before retrying to avoid duplicates"
                        .to_string(),
                );
            };
            let mismatches = create.metadata_mismatches();
            if !mismatches.is_empty() {
                record_issue_submission_state(
                    &config,
                    issue_set_id,
                    &proposal,
                    IssueSubmissionLedgerState::CreatedMetadataMismatch,
                    Some(&create.issue),
                    create.tracker_issue_id,
                    &format!(
                        "recovered created issue metadata mismatch: {}",
                        mismatches.join(", ")
                    ),
                )?;
                return Err(format!(
                    "recovered issue {} but metadata was dropped or changed: {}; linking skipped for human review",
                    create
                        .issue
                        .html_url
                        .clone()
                        .unwrap_or_else(|| create.issue.number.to_string()),
                    mismatches.join(", ")
                ));
            }
            record_issue_submission_state(
                &config,
                issue_set_id,
                &proposal,
                IssueSubmissionLedgerState::Created,
                Some(&create.issue),
                create.tracker_issue_id,
                "recovered created issue from marker search",
            )?;
            let Some(parent_issue) = proposal.parent_issue else {
                record_issue_submission_state(
                    &config,
                    issue_set_id,
                    &proposal,
                    IssueSubmissionLedgerState::Linked,
                    Some(&create.issue),
                    create.tracker_issue_id,
                    "recovered top-level issue",
                )?;
                println!("recovered top-level issue {}", create.issue.number);
                return Ok(());
            };
            let tracker_issue_id = create
                .tracker_issue_id
                .ok_or("recovered issue response did not include numeric GitHub issue id")?;
            let result = tracker.link_issue(agentactr_sdk::IssueLinkRequest {
                repo: proposal.repo.clone(),
                parent_issue,
                child_issue_number: create.issue.number,
                child_issue_id: tracker_issue_id,
            })?;
            record_issue_submission_state(
                &config,
                issue_set_id,
                &proposal,
                if result.linked {
                    IssueSubmissionLedgerState::Linked
                } else {
                    IssueSubmissionLedgerState::CreatedUnlinked
                },
                Some(&create.issue),
                create.tracker_issue_id,
                &result.detail,
            )?;
            println!("{}", result.detail);
            Ok(())
        }
        IssueSubmissionDecision::Create(req) => {
            let tracker = GithubRestAdapter::new_with_github(
                &context.artifact_dir,
                &config.tracker,
                &config.github,
            );
            let capabilities = tracker.capabilities();
            if !capabilities
                .supported_features
                .iter()
                .any(|feature| feature == "issue_create")
            {
                return Err(
                    "tracker adapter does not support issue_create; issue submission remains fail-closed"
                        .to_string(),
                );
            }
            ensure_issue_proposal_capabilities(&proposal, &capabilities)?;
            if allow_possible_duplicate {
                record_duplicate_override(
                    &context,
                    &proposal_id,
                    duplicate_reason.as_deref().unwrap_or_default(),
                )?;
            }
            begin_issue_submission(
                &config,
                issue_set_id,
                &proposal,
                duplicate_reason.as_deref(),
            )?;
            let create = tracker.create_issue(*req)?;
            let mismatches = create.metadata_mismatches();
            if !mismatches.is_empty() {
                record_issue_submission_state(
                    &config,
                    issue_set_id,
                    &proposal,
                    IssueSubmissionLedgerState::CreatedMetadataMismatch,
                    Some(&create.issue),
                    create.tracker_issue_id,
                    &format!("created issue metadata mismatch: {}", mismatches.join(", ")),
                )?;
                return Err(format!(
                    "created issue {} but metadata was dropped or changed: {}; linking skipped for human review",
                    create
                        .issue
                        .html_url
                        .clone()
                        .unwrap_or_else(|| create.issue.number.to_string()),
                    mismatches.join(", ")
                ));
            }
            record_issue_submission_state(
                &config,
                issue_set_id,
                &proposal,
                IssueSubmissionLedgerState::Created,
                Some(&create.issue),
                create.tracker_issue_id,
                "created issue",
            )?;
            let Some(parent_issue) = proposal.parent_issue else {
                record_issue_submission_state(
                    &config,
                    issue_set_id,
                    &proposal,
                    IssueSubmissionLedgerState::Linked,
                    Some(&create.issue),
                    create.tracker_issue_id,
                    "created top-level issue",
                )?;
                println!(
                    "created top-level issue {}",
                    create
                        .issue
                        .html_url
                        .clone()
                        .unwrap_or_else(|| create.issue.number.to_string())
                );
                return Ok(());
            };
            let tracker_issue_id = create
                .tracker_issue_id
                .ok_or("created issue response did not include numeric GitHub issue id")?;
            let link_request = agentactr_sdk::IssueLinkRequest {
                repo: proposal.repo.clone(),
                parent_issue,
                child_issue_number: create.issue.number,
                child_issue_id: tracker_issue_id,
            };
            let link = match tracker.link_issue(link_request) {
                Ok(link) => link,
                Err(err) => {
                    record_issue_submission_state(
                        &config,
                        issue_set_id,
                        &proposal,
                        IssueSubmissionLedgerState::CreatedUnlinked,
                        Some(&create.issue),
                        create.tracker_issue_id,
                        &format!("created issue but link failed: {err}"),
                    )?;
                    return Err(format!(
                        "created issue {} but linking as sub-issue failed: {err}; rerun with --resume --yes after fixing the cause",
                        create
                            .issue
                            .html_url
                            .clone()
                            .unwrap_or_else(|| create.issue.number.to_string())
                    ));
                }
            };
            record_issue_submission_state(
                &config,
                issue_set_id,
                &proposal,
                if link.linked {
                    IssueSubmissionLedgerState::Linked
                } else {
                    IssueSubmissionLedgerState::CreatedUnlinked
                },
                Some(&create.issue),
                create.tracker_issue_id,
                &link.detail,
            )?;
            println!("{}", link.detail);
            Ok(())
        }
        IssueSubmissionDecision::Link(req) => {
            let tracker = GithubRestAdapter::new(&context.artifact_dir, &config.tracker);
            let capabilities = tracker.capabilities();
            if !capabilities
                .supported_features
                .iter()
                .any(|feature| feature == "issue_link")
            {
                return Err(
                    "tracker adapter does not support issue_link; resume remains fail-closed"
                        .to_string(),
                );
            }
            let child_issue_id = req.child_issue_id;
            let result = tracker.link_issue(req)?;
            record_issue_submission_state(
                &config,
                issue_set_id,
                &proposal,
                if result.linked {
                    IssueSubmissionLedgerState::Linked
                } else {
                    IssueSubmissionLedgerState::CreatedUnlinked
                },
                None,
                Some(child_issue_id),
                &result.detail,
            )?;
            println!("{}", result.detail);
            Ok(())
        }
    }
}

fn cmd_issue_mark(args: &mut [String]) -> Result<(), String> {
    let issue_set_id = args.get(2).ok_or(
        "usage: agentactr issue mark ISSUE_SET_ID --proposal PROPOSAL_ID --dedupe unique|duplicate_blocked --reason TEXT",
    )?;
    let proposal_id =
        flag_value(args, "--proposal").ok_or("missing --proposal PROPOSAL_ID for issue mark")?;
    let dedupe = match flag_value(args, "--dedupe").as_deref() {
        Some("unique") => agentactr_sdk::IssueDedupeStatus::Unique,
        Some("duplicate_blocked") => agentactr_sdk::IssueDedupeStatus::DuplicateBlocked,
        Some(value) => return Err(format!("unsupported --dedupe `{value}`")),
        None => return Err("missing --dedupe unique|duplicate_blocked for issue mark".to_string()),
    };
    let reason = flag_value(args, "--reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("issue mark requires --reason TEXT")?;
    let config = load_agentactr_config(None)?;
    let context = load_issue_set_context(&config, issue_set_id)?;
    let mut proposals = load_issue_proposals(&context)?;
    let mut found = false;
    for proposal in &mut proposals {
        if proposal.proposal_id.as_str() == proposal_id {
            proposal.dedupe = dedupe;
            proposal.provenance.push(format!(
                "operator_dedupe_mark:{}:{}",
                dedupe.as_str(),
                sha256_hex_bytes(reason.as_bytes())
            ));
            found = true;
            break;
        }
    }
    if !found {
        return Err(format!(
            "proposal {proposal_id} not found in issue set {issue_set_id}"
        ));
    }
    write_issue_proposals(&context, &proposals)?;
    write_issue_dedupe_report_mark(&context, &proposals, &proposal_id, dedupe, &reason)?;
    println!(
        "marked proposal {} as {} in issue set {}",
        proposal_id,
        dedupe.as_str(),
        issue_set_id
    );
    Ok(())
}

fn issue_proposals_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("issue_proposals.json")
}

fn load_issue_proposals(context: &IssueSetArtifactContext) -> Result<Vec<IssueProposal>, String> {
    let path = issue_proposals_path(context);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| format!("parse {}: {e}", path.display()))?;
    let proposals = value
        .get("proposals")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .or_else(|| value.as_array().cloned())
        .ok_or_else(|| {
            format!(
                "{} must be an array or object with proposals array",
                path.display()
            )
        })?;
    proposals
        .iter()
        .map(|proposal| parse_issue_proposal(proposal, context))
        .collect()
}

fn parse_issue_proposal(
    value: &serde_json::Value,
    context: &IssueSetArtifactContext,
) -> Result<IssueProposal, String> {
    let proposal_id = json_required_str(value, "proposal_id")?;
    let title = json_required_str(value, "title")?;
    let body = json_required_str(value, "body")?;
    let digest = value
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| sha256_hex_bytes(format!("{title}\n{body}").as_bytes()));
    Ok(IssueProposal {
        proposal_id: IssueProposalId::new(proposal_id),
        repo: value
            .get("repo")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&context.repo)
            .to_string(),
        parent_issue: value
            .get("parent_issue")
            .and_then(serde_json::Value::as_u64)
            .or(context.parent_issue),
        title,
        body,
        labels: json_string_array(value, "labels"),
        assignees: json_string_array(value, "assignees"),
        milestone: value
            .get("milestone")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        issue_type: value
            .get("issue_type")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        issue_field_values: parse_issue_field_values(value)?,
        project_fields: parse_issue_project_fields(value)?,
        digest,
        dedupe: parse_issue_dedupe(value),
        framework: parse_issue_framework(value),
        related_issues: json_string_array(value, "related_issues")
            .into_iter()
            .map(IssueId)
            .collect(),
        provenance: json_string_array(value, "provenance"),
    })
}

fn load_issue_set_context(
    config: &AgentactrConfig,
    issue_set_id: &str,
) -> Result<IssueSetArtifactContext, String> {
    if let Ok(context) = load_issue_set_manifest_context(config, issue_set_id) {
        return Ok(context);
    }
    let run = load_run_artifact_context(config, issue_set_id)?;
    Ok(run_artifact_as_issue_set(&run))
}

fn run_artifact_as_issue_set(run: &RunArtifactContext) -> IssueSetArtifactContext {
    IssueSetArtifactContext {
        schema_version: 1,
        artifact_format_version: 1,
        issue_set_id: run.run_id.clone(),
        compat_run_id: Some(run.run_id.clone()),
        created_at: String::new(),
        producer: "agentactr-run-legacy".to_string(),
        source: IssueSetSource::RunLegacy,
        repo: run.repo.clone(),
        parent_issue: run.issue.parse::<u64>().ok(),
        framework: None,
        artifact_dir: run.artifact_dir.clone(),
        manifest_path: run.manifest_path.clone(),
        candidates_path: run.artifact_dir.join("issue_candidates.json"),
        proposals_path: run.artifact_dir.join("issue_proposals.json"),
        dedupe_report_path: run.artifact_dir.join("issue_dedupe_report.json"),
        planner_prompt_path: None,
        planner_metadata_path: None,
        trace_path: run.artifact_dir.join("trace.issue_set.jsonl"),
    }
}

fn load_issue_set_manifest_context(
    config: &AgentactrConfig,
    issue_set_id: &str,
) -> Result<IssueSetArtifactContext, String> {
    validate_run_id(issue_set_id)?;
    let artifact_dir = issue_set_artifact_dir(config, issue_set_id)?;
    let manifest_path = artifact_dir.join("issue_set_manifest.json");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("read issue set manifest {}: {e}", manifest_path.display()))?;
    let manifest = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| format!("parse issue set manifest {}: {e}", manifest_path.display()))?;
    let manifest_id = json_required_str(&manifest, "issue_set_id")?;
    if manifest_id != issue_set_id {
        return Err(format!(
            "issue set manifest {} is for `{manifest_id}`, not `{issue_set_id}`",
            manifest_path.display()
        ));
    }
    let repo = json_required_str(&manifest, "repo")?;
    let source = match manifest
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("run_legacy")
    {
        "find" => IssueSetSource::Find,
        "draft" => IssueSetSource::Draft,
        _ => IssueSetSource::RunLegacy,
    };
    Ok(IssueSetArtifactContext {
        schema_version: manifest
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u32,
        artifact_format_version: manifest
            .get("artifact_format_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u32,
        issue_set_id: issue_set_id.to_string(),
        compat_run_id: manifest
            .get("compat_run_id")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        created_at: manifest
            .get("created_at")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        producer: manifest
            .get("producer")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("agentactr-cli")
            .to_string(),
        source,
        repo,
        parent_issue: manifest
            .get("parent_issue")
            .and_then(serde_json::Value::as_u64),
        framework: manifest.get("framework").and_then(parse_framework_value),
        artifact_dir: artifact_dir.clone(),
        manifest_path,
        candidates_path: artifact_dir.join("issue_candidates.json"),
        proposals_path: artifact_dir.join("issue_proposals.json"),
        dedupe_report_path: artifact_dir.join("issue_dedupe_report.json"),
        planner_prompt_path: manifest
            .get("planner_prompt_path")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from),
        planner_metadata_path: manifest
            .get("planner_metadata_path")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from),
        trace_path: artifact_dir.join("trace.issue_set.jsonl"),
    })
}

fn issue_set_artifact_dir(config: &AgentactrConfig, issue_set_id: &str) -> Result<PathBuf, String> {
    validate_run_id(issue_set_id)?;
    Ok(resolve_config_path(&config.observability.artifact_root)?
        .join("issues")
        .join(issue_set_id))
}

fn create_issue_set_context(
    config: &AgentactrConfig,
    issue_set_id: &str,
    repo: &str,
    parent_issue: Option<u64>,
    framework: Option<agentactr_sdk::FrameworkDeclaration>,
    source: IssueSetSource,
) -> Result<IssueSetArtifactContext, String> {
    let artifact_dir = issue_set_artifact_dir(config, issue_set_id)?;
    create_dir(&artifact_dir)?;
    Ok(IssueSetArtifactContext {
        schema_version: 1,
        artifact_format_version: 1,
        issue_set_id: issue_set_id.to_string(),
        compat_run_id: None,
        created_at: timestamp_rfc3339_millis(),
        producer: "agentactr-cli".to_string(),
        source,
        repo: repo.to_string(),
        parent_issue,
        framework,
        manifest_path: artifact_dir.join("issue_set_manifest.json"),
        candidates_path: artifact_dir.join("issue_candidates.json"),
        proposals_path: artifact_dir.join("issue_proposals.json"),
        dedupe_report_path: artifact_dir.join("issue_dedupe_report.json"),
        planner_prompt_path: None,
        planner_metadata_path: None,
        trace_path: artifact_dir.join("trace.issue_set.jsonl"),
        artifact_dir,
    })
}

fn write_issue_set_manifest(
    context: &IssueSetArtifactContext,
    _config: &AgentactrConfig,
    stack: Option<&str>,
) -> Result<(), String> {
    let codex_review_status_path = codex_issue_review_status_path(context);
    let codex_review_markdown_path = codex_issue_review_markdown_path(context);
    let manifest = serde_json::json!({
        "schema_version": context.schema_version,
        "artifact_format_version": context.artifact_format_version,
        "issue_set_id": context.issue_set_id,
        "compat_run_id": context.compat_run_id,
        "created_at": context.created_at,
        "producer": context.producer,
        "source": context.source.as_str(),
        "repo": context.repo,
        "parent_issue": context.parent_issue,
        "stack": stack,
        "framework": context.framework.as_ref().map(framework_to_json),
        "candidates_path": context.candidates_path,
        "proposals_path": context.proposals_path,
        "dedupe_report_path": context.dedupe_report_path,
        "planner_prompt_path": context.planner_prompt_path,
        "planner_metadata_path": context.planner_metadata_path,
        "codex_review_status_path": codex_review_status_path.exists().then_some(codex_review_status_path),
        "codex_review_markdown_path": codex_review_markdown_path.exists().then_some(codex_review_markdown_path),
        "trace_path": context.trace_path,
    });
    write_file(
        &context.manifest_path,
        &serde_json::to_string_pretty(&manifest)
            .map_err(|e| format!("render issue set manifest: {e}"))?,
    )
}

fn write_issue_candidates(
    context: &IssueSetArtifactContext,
    candidates: &[agentactr_sdk::Issue],
) -> Result<(), String> {
    let items = candidates
        .iter()
        .map(issue_to_json)
        .collect::<Vec<serde_json::Value>>();
    let value = serde_json::json!({
        "issue_set_id": context.issue_set_id,
        "repo": context.repo,
        "candidate_count": candidates.len(),
        "candidates": items,
    });
    write_file(
        &context.candidates_path,
        &serde_json::to_string_pretty(&value)
            .map_err(|e| format!("render issue candidates: {e}"))?,
    )
}

fn write_issue_proposals(
    context: &IssueSetArtifactContext,
    proposals: &[IssueProposal],
) -> Result<(), String> {
    let items = proposals
        .iter()
        .map(issue_proposal_to_json)
        .collect::<Vec<serde_json::Value>>();
    let value = serde_json::json!({
        "issue_set_id": context.issue_set_id,
        "repo": context.repo,
        "proposal_count": proposals.len(),
        "proposals": items,
    });
    write_file(
        &context.proposals_path,
        &serde_json::to_string_pretty(&value)
            .map_err(|e| format!("render issue proposals: {e}"))?,
    )
}

fn write_issue_dedupe_report(
    context: &IssueSetArtifactContext,
    proposals: &[IssueProposal],
    candidates: &[agentactr_sdk::Issue],
) -> Result<(), String> {
    let value = serde_json::json!({
        "issue_set_id": context.issue_set_id,
        "normalization": {
            "title": "trim, unicode-preserving lowercase, collapse ascii whitespace",
            "version": 1
        },
        "candidate_count": candidates.len(),
        "proposals": proposals.iter().map(|proposal| serde_json::json!({
            "proposal_id": proposal.proposal_id.as_str(),
            "dedupe": proposal.dedupe.as_str(),
            "related_issues": proposal.related_issues.iter().map(|id| id.0.clone()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    write_file(
        &context.dedupe_report_path,
        &serde_json::to_string_pretty(&value)
            .map_err(|e| format!("render issue dedupe report: {e}"))?,
    )
}

fn write_issue_dedupe_report_mark(
    context: &IssueSetArtifactContext,
    proposals: &[IssueProposal],
    proposal_id: &str,
    dedupe: agentactr_sdk::IssueDedupeStatus,
    reason: &str,
) -> Result<(), String> {
    let value = serde_json::json!({
        "issue_set_id": context.issue_set_id,
        "normalization": {
            "title": "trim, unicode-preserving lowercase, collapse ascii whitespace",
            "version": 1
        },
        "candidate_count": null,
        "operator_override": {
            "proposal_id": proposal_id,
            "dedupe": dedupe.as_str(),
            "reason_sha256": sha256_hex_bytes(reason.as_bytes()),
        },
        "proposals": proposals.iter().map(|proposal| serde_json::json!({
            "proposal_id": proposal.proposal_id.as_str(),
            "dedupe": proposal.dedupe.as_str(),
            "related_issues": proposal.related_issues.iter().map(|id| id.0.clone()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    write_file(
        &context.dedupe_report_path,
        &serde_json::to_string_pretty(&value)
            .map_err(|e| format!("render issue dedupe report: {e}"))?,
    )
}

fn record_duplicate_override(
    context: &IssueSetArtifactContext,
    proposal_id: &str,
    reason: &str,
) -> Result<(), String> {
    if reason.trim().is_empty() {
        return Ok(());
    }
    let path = context.artifact_dir.join("issue_duplicate_override.json");
    let value = serde_json::json!({
        "issue_set_id": context.issue_set_id,
        "proposal_id": proposal_id,
        "reason_sha256": sha256_hex_bytes(reason.as_bytes()),
        "reason_recorded": true,
    });
    write_file(
        path,
        &serde_json::to_string_pretty(&value)
            .map_err(|e| format!("render duplicate override: {e}"))?,
    )
}

fn issue_to_json(issue: &agentactr_sdk::Issue) -> serde_json::Value {
    serde_json::json!({
        "id": issue.id,
        "repo": issue.repo,
        "number": issue.number,
        "title": issue.title,
        "body": issue.body,
        "state": issue.state,
        "author": issue.author,
        "labels": issue.labels,
        "created_at": issue.created_at,
        "updated_at": issue.updated_at,
        "is_pull_request": issue.is_pull_request,
        "html_url": issue.html_url,
    })
}

fn issue_proposal_to_json(proposal: &IssueProposal) -> serde_json::Value {
    serde_json::json!({
        "proposal_id": proposal.proposal_id.as_str(),
        "repo": proposal.repo,
        "parent_issue": proposal.parent_issue,
        "title": proposal.title,
        "body": proposal.body,
        "labels": proposal.labels,
        "assignees": proposal.assignees,
        "milestone": proposal.milestone,
        "issue_type": proposal.issue_type,
        "issue_field_values": proposal.issue_field_values.iter().map(|field| serde_json::json!({
            "field_id": field.field_id,
            "value": field.value,
            "type": field.value_type,
        })).collect::<Vec<_>>(),
        "project_fields": proposal.project_fields.iter().map(|field| serde_json::json!({
            "field_name": field.field_name,
            "value": field.value,
        })).collect::<Vec<_>>(),
        "digest": proposal.digest,
        "dedupe": proposal.dedupe.as_str(),
        "framework": proposal.framework.as_ref().map(|framework| serde_json::json!({
            "ecosystem": framework.ecosystem,
            "id": framework.id,
            "version_or_profile": framework.version_or_profile,
        })),
        "related_issues": proposal.related_issues.iter().map(|id| id.0.clone()).collect::<Vec<_>>(),
        "provenance": proposal.provenance,
    })
}

fn parse_candidate_query(
    args: &[String],
    repo: &str,
) -> Result<agentactr_sdk::CandidateQuery, String> {
    let state = match flag_value(args, "--state").as_deref().unwrap_or("open") {
        "open" => agentactr_sdk::CandidateState::Open,
        "closed" => agentactr_sdk::CandidateState::Closed,
        "all" => agentactr_sdk::CandidateState::All,
        value => return Err(format!("unsupported --state `{value}`")),
    };
    let limit = flag_value(args, "--limit")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(50)
        .clamp(1, 1000);
    let per_page = flag_value(args, "--per-page")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(50)
        .clamp(1, 100);
    let page = flag_value(args, "--page")
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| format!("invalid --page `{value}`"))
        })
        .transpose()?;
    let sort = match flag_value(args, "--sort").as_deref().unwrap_or("updated") {
        "created" => agentactr_sdk::CandidateSort::Created,
        "updated" => agentactr_sdk::CandidateSort::Updated,
        "comments" => agentactr_sdk::CandidateSort::Comments,
        value => return Err(format!("unsupported --sort `{value}`")),
    };
    let direction = match flag_value(args, "--direction").as_deref().unwrap_or("desc") {
        "asc" => agentactr_sdk::SortDirection::Asc,
        "desc" => agentactr_sdk::SortDirection::Desc,
        value => return Err(format!("unsupported --direction `{value}`")),
    };
    Ok(agentactr_sdk::CandidateQuery {
        repo: repo.to_string(),
        state,
        labels: flag_values(args, "--label"),
        assignee: flag_value(args, "--assignee"),
        author: flag_value(args, "--author"),
        since: flag_value(args, "--since"),
        text_query: flag_value(args, "--query").or_else(|| flag_value(args, "--search")),
        include_pull_requests: has_flag(args, "--include-pull-requests"),
        sort,
        direction,
        page,
        limit,
        per_page,
    })
}

fn apply_issue_artifact_root_override(
    config: &mut AgentactrConfig,
    args: &[String],
) -> Result<(), String> {
    if let Some(root) = flag_value(args, "--artifact-root") {
        if root.trim().is_empty() {
            return Err("--artifact-root cannot be empty".to_string());
        }
        config.observability.artifact_root = root;
    }
    Ok(())
}

fn load_issue_draft_prompt(args: &[String]) -> Result<Option<String>, String> {
    let prompt = flag_value(args, "--prompt");
    let prompt_file = flag_value(args, "--prompt-file");
    match (prompt, prompt_file) {
        (Some(_), Some(_)) => Err("use either --prompt or --prompt-file, not both".to_string()),
        (Some(prompt), None) => Ok(Some(prompt)),
        (None, Some(path)) => {
            let text = fs::read_to_string(&path)
                .map_err(|e| format!("read issue draft prompt file {path}: {e}"))?;
            Ok(Some(text))
        }
        (None, None) => Ok(None),
    }
}

fn selected_repo_stack_name(config: &AgentactrConfig) -> Option<String> {
    let value = config.repository.declared_primary_stack.trim();
    if value.is_empty() || value == "auto" {
        None
    } else {
        Some(value.to_string())
    }
}

fn discovered_repo_stack_name(inspection: &RepoInspection) -> Option<String> {
    match inspection.primary_stack {
        StackKind::TypeScript | StackKind::Rust | StackKind::Golang | StackKind::Python => {
            Some(inspection.primary_stack.as_str().to_string())
        }
        StackKind::Mixed | StackKind::Unknown => None,
    }
}

fn parse_framework_declaration(
    value: Option<&str>,
) -> Result<Option<agentactr_sdk::FrameworkDeclaration>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        "none" => Ok(None),
        "nextjs" => Ok(Some(agentactr_sdk::FrameworkDeclaration {
            ecosystem: "typescript".to_string(),
            id: "nextjs".to_string(),
            version_or_profile: None,
        })),
        other => Err(format!(
            "unsupported --framework `{other}`; expected nextjs or none"
        )),
    }
}

struct PromptArtifacts {
    redacted_path: PathBuf,
    metadata_path: PathBuf,
}

fn write_prompt_artifacts(
    context: &IssueSetArtifactContext,
    prompt: &str,
) -> Result<PromptArtifacts, String> {
    let metadata_path = context.artifact_dir.join("planner_prompt_metadata.json");
    let redacted_path = context.artifact_dir.join("planner_prompt.redacted.txt");
    let metadata = serde_json::json!({
        "issue_set_id": context.issue_set_id,
        "prompt_sha256": sha256_hex_bytes(prompt.as_bytes()),
        "prompt_bytes": prompt.len(),
        "prompt_chars": prompt.chars().count(),
        "source_kind": "inline_or_file",
        "raw_prompt_persisted": false,
        "redaction_policy": "redacted prompt only; raw prompt persistence is disabled by default",
        "redacted_prompt_path": redacted_path,
    });
    write_file(
        &metadata_path,
        &serde_json::to_string_pretty(&metadata)
            .map_err(|e| format!("render prompt metadata: {e}"))?,
    )?;
    write_file(&redacted_path, &redact_prompt(prompt))?;
    Ok(PromptArtifacts {
        redacted_path,
        metadata_path,
    })
}

fn redact_prompt(prompt: &str) -> String {
    prompt
        .lines()
        .map(|line| {
            if line.contains("TOKEN=")
                || line.contains("KEY=")
                || line.to_ascii_lowercase().contains("secret")
            {
                "[redacted]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_planner_metadata(
    context: &IssueSetArtifactContext,
    draft: &agentactr_sdk::IssueDraftResult,
    planner: &str,
) -> Result<(), String> {
    if let Some(path) = context.planner_metadata_path.as_ref() {
        let value = serde_json::json!({
            "issue_set_id": context.issue_set_id,
            "planner": planner,
            "schema_validated": true,
            "discarded_partial_output": draft.discarded_partial_output,
            "proposal_count": draft.proposals.len(),
            "detail": draft.detail,
            "planner_prompt_path": context.planner_prompt_path,
            "planner_prompt_metadata_path": context.artifact_dir.join("planner_prompt_metadata.json"),
            "capabilities": {
                "read_only": true,
                "github_mutation": false,
                "worktree_creation": false,
                "source_mutation": false,
                "quality_gates": false
            }
        });
        write_file(
            path,
            &serde_json::to_string_pretty(&value)
                .map_err(|e| format!("render planner metadata: {e}"))?,
        )?;
    }
    Ok(())
}

struct CodexIssueReviewArtifacts {
    status_path: PathBuf,
    review_path: PathBuf,
}

struct CodexIssueDraftArtifacts {
    status_path: PathBuf,
    response_path: PathBuf,
}

struct CodexIssueDraftStatusPaths<'a> {
    prompt_path: &'a Path,
    schema_path: &'a Path,
    response_path: &'a Path,
    stdout_path: &'a Path,
    stderr_path: &'a Path,
}

fn codex_issue_draft_status_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_draft_status.json")
}

fn codex_issue_draft_response_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_draft_response.json")
}

fn codex_issue_draft_stdout_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_draft.stdout.jsonl")
}

fn codex_issue_draft_stderr_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_draft.stderr.log")
}

fn codex_issue_draft_prompt_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_draft_prompt.txt")
}

fn codex_issue_draft_schema_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_draft_schema.json")
}

fn codex_issue_review_status_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_review_status.json")
}

fn codex_issue_review_markdown_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_review.md")
}

fn codex_issue_review_stdout_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_review.stdout.jsonl")
}

fn codex_issue_review_stderr_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_review.stderr.log")
}

fn codex_issue_review_prompt_path(context: &IssueSetArtifactContext) -> PathBuf {
    context.artifact_dir.join("codex_issue_review_prompt.txt")
}

fn run_codex_issue_draft_planner(
    config: &AgentactrConfig,
    context: &IssueSetArtifactContext,
    draft_request: &agentactr_sdk::IssueDraftRequest,
    operator_prompt: &str,
    stack: &str,
    local_inspection: &RepoInspection,
) -> Result<CodexIssueDraftArtifacts, String> {
    require_codex_exec_auth(&config.codex.command, &config.codex.openai_api_key_env)?;
    let prompt_path = codex_issue_draft_prompt_path(context);
    let schema_path = codex_issue_draft_schema_path(context);
    let response_path = codex_issue_draft_response_path(context);
    let stdout_path = codex_issue_draft_stdout_path(context);
    let stderr_path = codex_issue_draft_stderr_path(context);
    let status_path = codex_issue_draft_status_path(context);
    write_file(&schema_path, &codex_issue_draft_schema())?;
    let prompt = codex_issue_draft_prompt(
        context,
        draft_request,
        operator_prompt,
        stack,
        local_inspection,
    )?;
    write_file(&prompt_path, &prompt)?;
    let mut command = Command::new(&config.codex.command);
    command
        .arg("exec")
        .arg("--json")
        .arg("--sandbox")
        .arg("read-only")
        .arg("-c")
        .arg("approval_policy=\"never\"")
        .arg("--cd")
        .arg(".")
        .arg("--output-schema")
        .arg(&schema_path)
        .arg("--output-last-message")
        .arg(&response_path);
    append_codex_project_profile_overrides(&mut command, Path::new("."), &config.codex.profile)?;
    command
        .arg(prompt)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    forward_codex_api_key_env(&mut command, &config.codex.openai_api_key_env);
    let output = run_command_capture_timeout(command, Duration::from_secs(10 * 60))?;
    write_file(&stdout_path, &output.stdout)?;
    write_file(&stderr_path, &output.stderr)?;
    if !output.status.success() {
        let diagnostic = format!("{}\n{}", output.stdout, output.stderr);
        write_codex_issue_draft_status(
            context,
            "failed",
            CodexIssueDraftStatusPaths {
                prompt_path: &prompt_path,
                schema_path: &schema_path,
                response_path: &response_path,
                stdout_path: &stdout_path,
                stderr_path: &stderr_path,
            },
            Some(&classify_codex_exec_failure(&diagnostic)),
        )?;
        return Err(format!(
            "Codex issue draft failed: {}",
            classify_codex_exec_failure(&diagnostic)
        ));
    }
    if codex_probe_output_has_error_event(&output.stdout) {
        write_codex_issue_draft_status(
            context,
            "failed",
            CodexIssueDraftStatusPaths {
                prompt_path: &prompt_path,
                schema_path: &schema_path,
                response_path: &response_path,
                stdout_path: &stdout_path,
                stderr_path: &stderr_path,
            },
            Some("Codex issue draft emitted an error event"),
        )?;
        return Err(format!(
            "Codex issue draft emitted an error event; stdout_jsonl={}",
            stdout_path.display()
        ));
    }
    let raw_response = fs::read_to_string(&response_path)
        .map_err(|e| format!("read Codex issue draft {}: {e}", response_path.display()))?;
    agentactr_sdk::draft_issue_proposals_from_structured_json(
        draft_request.clone(),
        &raw_response,
        "codex_read_only_structured_issue_draft_planner",
    )
    .map_err(|e| format!("Codex issue draft output failed schema validation: {e}"))?;
    write_codex_issue_draft_status(
        context,
        "schema_valid",
        CodexIssueDraftStatusPaths {
            prompt_path: &prompt_path,
            schema_path: &schema_path,
            response_path: &response_path,
            stdout_path: &stdout_path,
            stderr_path: &stderr_path,
        },
        None,
    )?;
    Ok(CodexIssueDraftArtifacts {
        status_path,
        response_path,
    })
}

fn write_codex_issue_draft_status(
    context: &IssueSetArtifactContext,
    status: &str,
    paths: CodexIssueDraftStatusPaths<'_>,
    error: Option<&str>,
) -> Result<(), String> {
    let value = serde_json::json!({
        "schema_version": "0.1",
        "issue_set_id": context.issue_set_id,
        "status": status,
        "planner": "codex_read_only_structured_issue_draft_planner",
        "prompt_path": paths.prompt_path,
        "schema_path": paths.schema_path,
        "response_path": paths.response_path,
        "stdout_jsonl_path": paths.stdout_path,
        "stderr_log_path": paths.stderr_path,
        "read_only": true,
        "github_mutation": false,
        "workspace_mutation": false,
        "error": error,
    });
    write_file(
        codex_issue_draft_status_path(context),
        &serde_json::to_string_pretty(&value)
            .map_err(|e| format!("render Codex issue draft status: {e}"))?,
    )
}

fn codex_issue_draft_prompt(
    context: &IssueSetArtifactContext,
    draft_request: &agentactr_sdk::IssueDraftRequest,
    operator_prompt: &str,
    stack: &str,
    local_inspection: &RepoInspection,
) -> Result<String, String> {
    let candidates_json = serde_json::to_string_pretty(
        &draft_request
            .candidates
            .iter()
            .take(25)
            .map(|issue| {
                serde_json::json!({
                    "id": issue.id,
                    "repo": issue.repo,
                    "number": issue.number,
                    "title": issue.title,
                    "state": issue.state,
                    "labels": issue.labels,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|e| format!("render issue candidate context: {e}"))?;
    let evidence_files = local_inspection
        .evidence_files
        .iter()
        .take(80)
        .cloned()
        .collect::<Vec<_>>();
    let evidence_json = serde_json::to_string_pretty(&serde_json::json!({
        "detected_stack": local_inspection.primary_stack.as_str(),
        "confidence": local_inspection.confidence,
        "is_empty": local_inspection.is_empty,
        "evidence_files": evidence_files,
    }))
    .map_err(|e| format!("render repository evidence context: {e}"))?;
    Ok(format!(
        r#"You are a read-only GitHub issue drafting planner for agentactr.

Repository: {repo}
Stack: {stack}
Framework: {framework}
Issue set: {issue_set_id}
Parent issue: {parent_issue}

Operator request:
{operator_prompt}

Current repository evidence:
```json
{evidence_json}
```

Existing issue candidates for dedupe context:
```json
{candidates_json}
```

Strict rules:
- Inspect the current checkout read-only before drafting.
- Do not modify files.
- Do not create GitHub issues.
- Do not run implementation work.
- Do not assume the repository is blank when files/routes/packages already exist.
- Draft small, independently implementable issues that are specific to the current repository state.
- Avoid generic architecture/bootstrap issues unless repository evidence proves the gap exists.
- Prefer concrete missing operations, consistency fixes, guard/authorization gaps, tests, observability gaps, or UI/API parity gaps.
- Return `labels`, `assignees`, and `provenance` as arrays. Use empty arrays when no values are appropriate.
- Return `milestone` and `issue_type` as strings or null. Use null when not explicitly requested.
- Return `project_fields` only for explicit ProjectV2 metadata. Use `Priority` values `P0`, `P1`, or `P2`; use `Size` values `XS`, `S`, `M`, `L`, or `XL`.
- Return only JSON matching the provided output schema.

Each proposal body must include:
- Problem
- Scope
- Acceptance criteria
- Suggested files or areas to inspect
- Out-of-scope notes when useful
"#,
        repo = context.repo,
        stack = stack,
        framework = context
            .framework
            .as_ref()
            .map(|framework| format!("{}/{}", framework.ecosystem, framework.id))
            .unwrap_or_else(|| "none".to_string()),
        issue_set_id = context.issue_set_id,
        parent_issue = context
            .parent_issue
            .map(|issue| format!("#{issue}"))
            .unwrap_or_else(|| "none".to_string()),
    ))
}

fn codex_issue_draft_schema() -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["proposals"],
        "properties": {
            "proposals": {
                "type": "array",
                "minItems": 1,
                "maxItems": 50,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [
                        "title",
                        "body",
                        "labels",
                        "assignees",
                        "milestone",
                        "issue_type",
                        "project_fields",
                        "provenance"
                    ],
                    "properties": {
                        "title": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": 180
                        },
                        "body": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": 60000
                        },
                        "labels": {
                            "type": "array",
                            "items": { "type": "string", "maxLength": 50 },
                            "maxItems": 50
                        },
                        "assignees": {
                            "type": "array",
                            "items": { "type": "string", "maxLength": 100 },
                            "maxItems": 20
                        },
                        "milestone": {
                            "type": ["string", "null"],
                            "maxLength": 100
                        },
                        "issue_type": {
                            "type": ["string", "null"],
                            "maxLength": 100
                        },
                        "project_fields": {
                            "type": "array",
                            "maxItems": 20,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["field_name", "value"],
                                "properties": {
                                    "field_name": {
                                        "type": "string",
                                        "minLength": 1,
                                        "maxLength": 80
                                    },
                                    "value": {
                                        "type": "string",
                                        "minLength": 1,
                                        "maxLength": 80
                                    }
                                }
                            }
                        },
                        "provenance": {
                            "type": "array",
                            "items": { "type": "string", "maxLength": 200 },
                            "maxItems": 20
                        }
                    }
                }
            }
        }
    }))
    .expect("static Codex issue draft schema renders")
}

fn run_codex_issue_draft_review(
    config: &AgentactrConfig,
    context: &IssueSetArtifactContext,
    proposals: &[IssueProposal],
    stack: &str,
) -> Result<CodexIssueReviewArtifacts, String> {
    require_codex_exec_auth(&config.codex.command, &config.codex.openai_api_key_env)?;
    let prompt_path = codex_issue_review_prompt_path(context);
    let review_path = codex_issue_review_markdown_path(context);
    let stdout_path = codex_issue_review_stdout_path(context);
    let stderr_path = codex_issue_review_stderr_path(context);
    let status_path = codex_issue_review_status_path(context);
    let proposal_json = serde_json::to_string_pretty(
        &proposals
            .iter()
            .map(issue_proposal_to_json)
            .collect::<Vec<_>>(),
    )
    .map_err(|e| format!("render issue proposals for Codex review: {e}"))?;
    let prompt = format!(
        r#"You are a read-only issue proposal reviewer for agentactr.

Repository: {repo}
Stack: {stack}
Framework: {framework}
Issue set: {issue_set_id}

Review the proposed GitHub issues against the current repository checkout.

Strict rules:
- Do not modify files.
- Do not create GitHub issues.
- Do not run implementation work.
- Use read-only inspection only.
- Check whether the proposals are specific, actionable, and appropriate for this repository.
- If proposals are acceptable for GitHub creation, your final answer must include exactly this line: VERDICT: APPROVED
- If any proposal should not be created, use: VERDICT: NEEDS_REVISION
- Include proposal IDs and concise reasons.

Proposals JSON:
```json
{proposal_json}
```
"#,
        repo = context.repo,
        stack = stack,
        framework = context
            .framework
            .as_ref()
            .map(|framework| format!("{}/{}", framework.ecosystem, framework.id))
            .unwrap_or_else(|| "none".to_string()),
        issue_set_id = context.issue_set_id,
    );
    write_file(&prompt_path, &prompt)?;
    let mut command = Command::new(&config.codex.command);
    command
        .arg("exec")
        .arg("--json")
        .arg("--sandbox")
        .arg("read-only")
        .arg("-c")
        .arg("approval_policy=\"never\"")
        .arg("--cd")
        .arg(".")
        .arg("--output-last-message")
        .arg(&review_path);
    append_codex_project_profile_overrides(&mut command, Path::new("."), &config.codex.profile)?;
    command
        .arg(prompt)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    forward_codex_api_key_env(&mut command, &config.codex.openai_api_key_env);
    let output = run_command_capture_timeout(command, Duration::from_secs(10 * 60))?;
    write_file(&stdout_path, &output.stdout)?;
    write_file(&stderr_path, &output.stderr)?;
    if !output.status.success() {
        let diagnostic = format!("{}\n{}", output.stdout, output.stderr);
        return Err(format!(
            "Codex proposal review failed: {}",
            classify_codex_exec_failure(&diagnostic)
        ));
    }
    if codex_probe_output_has_error_event(&output.stdout) {
        return Err(format!(
            "Codex proposal review emitted an error event; stdout_jsonl={}",
            stdout_path.display()
        ));
    }
    let review = fs::read_to_string(&review_path)
        .map_err(|e| format!("read Codex issue review {}: {e}", review_path.display()))?;
    if !review.contains("VERDICT: APPROVED") {
        write_codex_issue_review_status(
            context,
            proposals,
            "needs_revision",
            &prompt_path,
            &review_path,
            &stdout_path,
            &stderr_path,
        )?;
        return Err(format!(
            "Codex proposal review did not approve issue creation; inspect {}",
            review_path.display()
        ));
    }
    write_codex_issue_review_status(
        context,
        proposals,
        "approved",
        &prompt_path,
        &review_path,
        &stdout_path,
        &stderr_path,
    )?;
    Ok(CodexIssueReviewArtifacts {
        status_path,
        review_path,
    })
}

fn write_codex_issue_review_status(
    context: &IssueSetArtifactContext,
    proposals: &[IssueProposal],
    status: &str,
    prompt_path: &Path,
    review_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<(), String> {
    let value = serde_json::json!({
        "issue_set_id": context.issue_set_id,
        "reviewer": "codex-cli-json",
        "status": status,
        "sandbox": "read-only",
        "approval_policy": "never",
        "reviewed_proposal_ids": proposals.iter().map(|proposal| proposal.proposal_id.as_str()).collect::<Vec<_>>(),
        "prompt_path": prompt_path,
        "review_path": review_path,
        "stdout_jsonl_path": stdout_path,
        "stderr_log_path": stderr_path,
    });
    write_file(
        codex_issue_review_status_path(context),
        &serde_json::to_string_pretty(&value)
            .map_err(|e| format!("render Codex issue review status: {e}"))?,
    )
}

fn require_codex_review_for_proposal(
    context: &IssueSetArtifactContext,
    proposal_id: &str,
) -> Result<(), String> {
    let status_path = codex_issue_review_status_path(context);
    let raw = fs::read_to_string(&status_path).map_err(|e| {
        format!(
            "missing Codex issue proposal review {}; rerun `agentactr issue draft ... --codex-review` or omit --require-codex-review after explicit human review: {e}",
            status_path.display()
        )
    })?;
    let parsed = serde_json::from_str::<serde_json::Value>(&raw).map_err(|e| {
        format!(
            "parse Codex issue review status {}: {e}",
            status_path.display()
        )
    })?;
    if parsed.get("status").and_then(serde_json::Value::as_str) != Some("approved") {
        return Err(format!(
            "Codex issue proposal review is not approved in {}",
            status_path.display()
        ));
    }
    let reviewed = parsed
        .get("reviewed_proposal_ids")
        .and_then(serde_json::Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(serde_json::Value::as_str)
                .any(|id| id == proposal_id)
        })
        .unwrap_or(false);
    if !reviewed {
        return Err(format!(
            "proposal {proposal_id} was not covered by Codex review {}",
            status_path.display()
        ));
    }
    Ok(())
}

fn parse_issue_dedupe(value: &serde_json::Value) -> agentactr_sdk::IssueDedupeStatus {
    match value
        .get("dedupe")
        .or_else(|| value.get("dedupe_status"))
        .and_then(serde_json::Value::as_str)
    {
        Some("duplicate_blocked") => agentactr_sdk::IssueDedupeStatus::DuplicateBlocked,
        Some("possible_duplicate") => agentactr_sdk::IssueDedupeStatus::PossibleDuplicate,
        _ => agentactr_sdk::IssueDedupeStatus::Unique,
    }
}

fn parse_issue_framework(value: &serde_json::Value) -> Option<agentactr_sdk::FrameworkDeclaration> {
    let framework = value.get("framework")?;
    parse_framework_value(framework)
}

fn parse_framework_value(value: &serde_json::Value) -> Option<agentactr_sdk::FrameworkDeclaration> {
    if !value.is_object() {
        return None;
    }
    Some(agentactr_sdk::FrameworkDeclaration {
        ecosystem: value
            .get("ecosystem")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        id: value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        version_or_profile: value
            .get("version_or_profile")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
    })
}

fn framework_to_json(framework: &agentactr_sdk::FrameworkDeclaration) -> serde_json::Value {
    serde_json::json!({
        "ecosystem": framework.ecosystem,
        "id": framework.id,
        "version_or_profile": framework.version_or_profile,
    })
}

fn new_issue_set_id(kind: &str) -> String {
    new_run_id(kind)
}

fn ensure_issue_proposal_capabilities(
    proposal: &IssueProposal,
    capabilities: &AdapterCapabilities,
) -> Result<(), String> {
    let supported = |feature: &str| {
        capabilities
            .supported_features
            .iter()
            .any(|supported| supported == feature)
    };
    if !proposal.labels.is_empty() && !supported("issue_labels") {
        return Err("tracker adapter does not support issue_labels for this proposal".to_string());
    }
    if !proposal.assignees.is_empty() && !supported("issue_assignees") {
        return Err(
            "tracker adapter does not support issue_assignees for this proposal".to_string(),
        );
    }
    if proposal.milestone.is_some() && !supported("issue_milestone") {
        return Err(
            "tracker adapter does not support issue_milestone for this proposal".to_string(),
        );
    }
    if proposal.issue_type.is_some() && !supported("issue_type") {
        return Err("tracker adapter does not support issue_type for this proposal".to_string());
    }
    if !proposal.issue_field_values.is_empty() && !supported("issue_field_values") {
        return Err(
            "tracker adapter does not support issue_field_values for this proposal".to_string(),
        );
    }
    if !proposal.project_fields.is_empty() && !supported("github_projects_v2") {
        return Err(
            "tracker adapter does not support github_projects_v2 for this proposal".to_string(),
        );
    }
    Ok(())
}

fn parse_issue_field_values(value: &serde_json::Value) -> Result<Vec<IssueFieldValue>, String> {
    let Some(fields) = value
        .get("issue_field_values")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(Vec::new());
    };
    fields
        .iter()
        .map(|field| {
            let field_id = field
                .get("field_id")
                .and_then(serde_json::Value::as_i64)
                .ok_or("issue_field_values entries require integer field_id")?;
            let value = field
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or("issue_field_values entries require string value")?
                .to_string();
            let value_type = field
                .get("type")
                .or_else(|| field.get("value_type"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            Ok(IssueFieldValue {
                field_id,
                value,
                value_type,
            })
        })
        .collect()
}

fn parse_issue_project_fields(
    value: &serde_json::Value,
) -> Result<Vec<IssueProjectFieldValue>, String> {
    let Some(fields) = value
        .get("project_fields")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(Vec::new());
    };
    if fields.len() > 20 {
        return Err("project_fields exceeds 20 entries".to_string());
    }
    let mut parsed = Vec::new();
    for field in fields {
        let field_name = field
            .get("field_name")
            .or_else(|| field.get("name"))
            .and_then(serde_json::Value::as_str)
            .ok_or("project_fields entries require string field_name")?
            .trim();
        let value = field
            .get("value")
            .and_then(serde_json::Value::as_str)
            .ok_or("project_fields entries require string value")?
            .trim();
        if field_name.is_empty() || value.is_empty() {
            continue;
        }
        validate_issue_project_field_value(field_name, value)?;
        parsed.retain(|existing: &IssueProjectFieldValue| existing.field_name != field_name);
        parsed.push(IssueProjectFieldValue {
            field_name: field_name.to_string(),
            value: value.to_string(),
        });
    }
    parsed.sort_by(|a, b| a.field_name.cmp(&b.field_name));
    Ok(parsed)
}

fn validate_issue_project_field_value(field_name: &str, value: &str) -> Result<(), String> {
    match field_name {
        "Priority" if !matches!(value, "P0" | "P1" | "P2") => {
            Err("project field `Priority` must be P0, P1, or P2".to_string())
        }
        "Size" if !matches!(value, "XS" | "S" | "M" | "L" | "XL") => {
            Err("project field `Size` must be XS, S, M, L, or XL".to_string())
        }
        _ => Ok(()),
    }
}

fn json_required_str(value: &serde_json::Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| format!("issue proposal is missing string field `{key}`"))
}

fn json_string_array(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn emit_github_rate_limit_trace_events(
    config: &AgentactrConfig,
    run_id: &str,
    repo: &str,
    issue: &str,
    artifact_dir: &Path,
) -> Result<(), String> {
    let path = artifact_dir.join("github_rate_limit_events.jsonl");
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let payload = serde_json::from_str::<serde_json::Value>(line).map_err(|e| {
            format!(
                "parse GitHub rate-limit event {} line {}: {e}",
                path.display(),
                index + 1
            )
        })?;
        append_run_event(
            &RunEventContext::root(config, run_id, repo, issue),
            "github.rate_limit.updated",
            payload,
        )?;
    }
    Ok(())
}

fn lifecycle_mode_for_run_policy(policy: &RunPolicy) -> IssueLifecycleMode {
    match policy.github_finalization {
        GithubFinalizationSetting::Disabled
        | GithubFinalizationSetting::DisabledInThisMilestone => IssueLifecycleMode::Disabled,
        GithubFinalizationSetting::RequireHumanReview => IssueLifecycleMode::RequireHumanReview,
        GithubFinalizationSetting::AutomaticAfterQualityGates => {
            if policy.human_intervention == HumanInterventionSetting::ReviewRequired {
                IssueLifecycleMode::RequireHumanReview
            } else {
                IssueLifecycleMode::AutomaticAfterQualityGates
            }
        }
    }
}

fn lifecycle_labels_from_config(config: &AgentactrConfig) -> LifecycleLabels {
    LifecycleLabels {
        claim_label: config.tracker.claim_label.clone(),
        running_label: config.tracker.running_label.clone(),
        failed_label: config.tracker.failed_label.clone(),
        done_label: config.tracker.done_label.clone(),
    }
}

fn write_quality_status(
    report_path: &Path,
    success: bool,
    failed_reason: Option<&str>,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "schema_version": "0.1",
        "success": success,
        "report_path": report_path.display().to_string(),
        "failed_reason": failed_reason,
    });
    write_file(
        agentactr_sdk::quality_status_path(report_path),
        &serde_json::to_string_pretty(&payload)
            .map_err(|e| format!("render quality status: {e}"))?,
    )
}

fn record_finalization_status(
    config: &AgentactrConfig,
    run_id: &str,
    repo: &str,
    issue: &str,
    artifact_dir: &Path,
    status: &str,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "schema_version": "0.1",
        "run_id": run_id,
        "repo": repo,
        "issue": issue,
        "status": status,
        "mode": config.github.finalization,
    });
    write_file(
        artifact_dir.join("finalization_status.json"),
        &serde_json::to_string_pretty(&payload)
            .map_err(|e| format!("render finalization status: {e}"))?,
    )?;
    append_run_event(
        &RunEventContext::root(config, run_id, repo, issue),
        "finalization.status",
        payload,
    )
}

fn write_lifecycle_event(
    config: &AgentactrConfig,
    run_id: &str,
    repo: &str,
    issue: &str,
    artifact_dir: &Path,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    let event = serde_json::json!({
        "schema_version": "0.1",
        "ts": iso_timestamp_from_epoch_millis(current_epoch_millis()),
        "run_id": run_id,
        "repo": repo,
        "issue": issue,
        "event_type": event_type,
        "payload": payload,
    });
    let lifecycle_path = artifact_dir.join("github_lifecycle_events.jsonl");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&lifecycle_path)
        .map_err(|e| {
            format!(
                "open GitHub lifecycle events {}: {e}",
                lifecycle_path.display()
            )
        })?;
    writeln!(file, "{event}").map_err(|e| {
        format!(
            "write GitHub lifecycle event {}: {e}",
            lifecycle_path.display()
        )
    })?;
    append_run_event(
        &RunEventContext::root(config, run_id, repo, issue),
        event_type,
        event,
    )
}

fn record_lifecycle_report(
    config: &AgentactrConfig,
    run_id: &str,
    repo: &str,
    issue: &str,
    artifact_dir: &Path,
    event_type: &str,
    report: &agentactr_sdk::IssueLifecycleReport,
) -> Result<(), String> {
    write_lifecycle_event(
        config,
        run_id,
        repo,
        issue,
        artifact_dir,
        event_type,
        serde_json::json!({
            "status": report.status,
            "release": report.release.as_ref().map(|release| serde_json::json!({
                "verification_status": release.verification_status,
                "final_issue_state": release.final_issue_state,
                "state_reason": release.state_reason,
                "artifacts": release.source_artifacts.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
            })),
        }),
    )?;
    record_finalization_status(config, run_id, repo, issue, artifact_dir, &report.status)
}

struct QualityCommandOutput {
    executed_command: String,
    status: String,
    success: bool,
    stdout: String,
    stderr: String,
}

struct TimedCommandOutput {
    status: Option<std::process::ExitStatus>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

impl TimedCommandOutput {
    fn status_text(&self) -> String {
        if self.timed_out {
            format!("timed out after {}s", quality_gate_timeout().as_secs())
        } else {
            self.status
                .map(|status| status.to_string())
                .unwrap_or_else(|| "terminated without exit status".to_string())
        }
    }

    fn success(&self) -> bool {
        !self.timed_out && self.status.is_some_and(|status| status.success())
    }
}

fn run_quality_command(
    name: &str,
    command: &str,
    worktree: &Path,
) -> Result<QualityCommandOutput, String> {
    if let Some(output) = run_go_logical_quality_command(command, worktree)? {
        return Ok(output);
    }

    let mut process = Command::new("sh");
    process.arg("-lc").arg(command).current_dir(worktree);
    let output = run_command_with_timeout(&mut process, name, quality_gate_timeout())?;
    Ok(QualityCommandOutput {
        executed_command: command.to_string(),
        status: output.status_text(),
        success: output.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn quality_gate_timeout() -> Duration {
    Duration::from_secs(30 * 60)
}

fn run_command_with_timeout(
    command: &mut Command,
    name: &str,
    timeout: Duration,
) -> Result<TimedCommandOutput, String> {
    configure_quality_process_group(command);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("start quality gate `{name}`: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("quality gate `{name}` stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("quality gate `{name}` stderr unavailable"))?;
    let stdout_thread = thread::spawn(move || read_stream_to_string(stdout));
    let stderr_thread = thread::spawn(move || read_stream_to_string(stderr));
    let (status, timed_out) = wait_quality_child(&mut child, timeout)?;
    let stdout = stdout_thread
        .join()
        .map_err(|_| format!("quality gate `{name}` stdout reader panicked"))??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| format!("quality gate `{name}` stderr reader panicked"))??;
    Ok(TimedCommandOutput {
        status,
        stdout,
        stderr,
        timed_out,
    })
}

fn read_stream_to_string(mut stream: impl Read) -> Result<String, String> {
    let mut output = Vec::new();
    stream
        .read_to_end(&mut output)
        .map_err(|e| format!("read command stream: {e}"))?;
    Ok(String::from_utf8_lossy(&output).to_string())
}

fn wait_quality_child(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<(Option<std::process::ExitStatus>, bool), String> {
    let start = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("poll quality gate: {e}"))?
        {
            return Ok((Some(status), false));
        }
        if start.elapsed() >= timeout {
            terminate_child(child, Duration::from_secs(2));
            return Ok((None, true));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn run_gofmt_check(worktree: &Path) -> Result<QualityCommandOutput, String> {
    run_gofmt_check_with_executed_command(worktree, "gofmt -l <go files>".to_string())
}

fn run_gofmt_check_with_executed_command(
    worktree: &Path,
    executed_command: String,
) -> Result<QualityCommandOutput, String> {
    let files = collect_go_files(worktree)?;
    if files.is_empty() {
        return Ok(QualityCommandOutput {
            executed_command,
            status: "skipped: no Go files".to_string(),
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        });
    }
    let mut process = Command::new("gofmt");
    process.arg("-l").args(&files).current_dir(worktree);
    let output = run_command_with_timeout(&mut process, "gofmt", quality_gate_timeout())?;
    let success = output.success() && output.stdout.trim().is_empty();
    let status = if success || output.timed_out {
        output.status_text()
    } else {
        format!("{}; files need gofmt", output.status_text())
    };
    Ok(QualityCommandOutput {
        executed_command,
        status,
        success,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn run_go_mod_tidy_check(worktree: &Path) -> Result<QualityCommandOutput, String> {
    run_go_mod_tidy_check_for_module(worktree, worktree, ".".to_string())
}

fn run_go_mod_tidy_check_for_module(
    repo_root: &Path,
    module_root: &Path,
    module_display: String,
) -> Result<QualityCommandOutput, String> {
    let executed_command = if module_display == "." {
        "go mod tidy in temporary copy".to_string()
    } else {
        format!("cd {module_display} && go mod tidy in temporary copy")
    };
    if !module_root.join("go.mod").exists() {
        return Ok(QualityCommandOutput {
            executed_command,
            status: "failed: go.mod missing".to_string(),
            success: false,
            stdout: String::new(),
            stderr: "go mod tidy-check requires go.mod".to_string(),
        });
    }
    let temp_root = env::temp_dir().join(format!(
        "agentactr-go-tidy-check-{}-{}",
        std::process::id(),
        current_epoch_millis()
    ));
    let copy_result = copy_worktree_for_quality_check(repo_root, &temp_root);
    if let Err(err) = copy_result {
        let _ = fs::remove_dir_all(&temp_root);
        return Err(err);
    }
    let module_rel = module_root
        .strip_prefix(repo_root)
        .map_err(|e| {
            let _ = fs::remove_dir_all(&temp_root);
            format!(
                "strip module root {} from repo root {}: {e}",
                module_root.display(),
                repo_root.display()
            )
        })?
        .to_path_buf();
    let temp_module_root = temp_root.join(&module_rel);
    let mut process = Command::new("go");
    process
        .arg("mod")
        .arg("tidy")
        .current_dir(&temp_module_root);
    let output =
        match run_command_with_timeout(&mut process, "go mod tidy-check", quality_gate_timeout()) {
            Ok(output) => output,
            Err(err) => {
                let _ = fs::remove_dir_all(&temp_root);
                return Err(err);
            }
        };
    let changed = module_file_changed(module_root, &temp_module_root, "go.mod")
        || module_file_changed(module_root, &temp_module_root, "go.sum");
    let success = output.success() && !changed;
    let status = if success || output.timed_out {
        output.status_text()
    } else if changed {
        format!("{}; go.mod/go.sum would change", output.status_text())
    } else {
        output.status_text()
    };
    let _ = fs::remove_dir_all(&temp_root);
    Ok(QualityCommandOutput {
        executed_command,
        status,
        success,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn run_go_logical_quality_command(
    command: &str,
    worktree: &Path,
) -> Result<Option<QualityCommandOutput>, String> {
    match command {
        "gofmt-check" => return run_gofmt_check(worktree).map(Some),
        "go mod tidy-check" => return run_go_mod_tidy_check(worktree).map(Some),
        _ => {}
    }

    let Some((scope, logical_command)) = parse_scoped_go_logical_command(command)? else {
        return Ok(None);
    };
    let scoped_worktree = resolve_scoped_quality_dir(worktree, &scope)?;
    let scoped_prefix = format!("cd {scope}");
    match logical_command {
        "gofmt-check" => run_gofmt_check_with_executed_command(
            &scoped_worktree,
            format!("{scoped_prefix} && gofmt -l <go files>"),
        )
        .map(Some),
        "go mod tidy-check" => {
            run_go_mod_tidy_check_for_module(worktree, &scoped_worktree, scope).map(Some)
        }
        _ => Ok(None),
    }
}

fn parse_scoped_go_logical_command(
    command: &str,
) -> Result<Option<(String, &'static str)>, String> {
    let Some((cd_part, logical_command)) = command
        .strip_suffix(" && gofmt-check")
        .map(|prefix| (prefix, "gofmt-check"))
        .or_else(|| {
            command
                .strip_suffix(" && go mod tidy-check")
                .map(|prefix| (prefix, "go mod tidy-check"))
        })
    else {
        return Ok(None);
    };
    let Some(scope_word) = cd_part.strip_prefix("cd ") else {
        return Ok(None);
    };
    let scope = unquote_shell_word(scope_word)
        .ok_or_else(|| format!("unsupported scoped Go quality command `{command}`"))?;
    Ok(Some((scope, logical_command)))
}

fn unquote_shell_word(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-'))
    {
        return Some(value.to_string());
    }

    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\'' {
            loop {
                match chars.next() {
                    Some('\'') => break,
                    Some(quoted) => output.push(quoted),
                    None => return None,
                }
            }
        } else if ch == '\\' && chars.peek() == Some(&'\'') {
            chars.next();
            output.push('\'');
        } else {
            return None;
        }
    }
    Some(output)
}

fn resolve_scoped_quality_dir(worktree: &Path, scope: &str) -> Result<PathBuf, String> {
    let relative = Path::new(scope);
    if scope.trim().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!("invalid scoped Go quality directory `{scope}`"));
    }
    let canonical_worktree = fs::canonicalize(worktree)
        .map_err(|e| format!("canonicalize worktree {}: {e}", worktree.display()))?;
    let scoped = fs::canonicalize(worktree.join(relative)).map_err(|e| {
        format!(
            "canonicalize scoped Go quality directory {}: {e}",
            worktree.join(relative).display()
        )
    })?;
    if !scoped.starts_with(&canonical_worktree) {
        return Err(format!(
            "scoped Go quality directory `{scope}` escapes worktree {}",
            worktree.display()
        ));
    }
    Ok(scoped)
}

fn collect_go_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_go_files_inner(root, root, &mut files)?;
    Ok(files)
}

fn collect_go_files_inner(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read {} entry: {e}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("read {} type: {e}", path.display()))?;
        if file_type.is_dir() {
            if should_skip_quality_dir(&path) {
                continue;
            }
            collect_go_files_inner(root, &path, files)?;
        } else if file_type.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("go")
        {
            files.push(
                path.strip_prefix(root)
                    .map_err(|e| format!("strip {} prefix: {e}", path.display()))?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

fn should_skip_quality_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|value| value.to_str()),
        Some(".git" | ".agentactr" | "target" | "node_modules")
    )
}

fn copy_worktree_for_quality_check(source: &Path, target: &Path) -> Result<(), String> {
    create_dir(target)?;
    copy_worktree_dir(source, source, target)
}

fn copy_worktree_dir(root: &Path, source: &Path, target_root: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|e| format!("read {}: {e}", source.display()))? {
        let entry = entry.map_err(|e| format!("read {} entry: {e}", source.display()))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|e| format!("strip {} prefix: {e}", path.display()))?;
        if should_skip_quality_dir(&path) {
            continue;
        }
        let target = target_root.join(relative);
        let file_type = entry
            .file_type()
            .map_err(|e| format!("read {} type: {e}", path.display()))?;
        if file_type.is_dir() {
            create_dir(&target)?;
            copy_worktree_dir(root, &path, target_root)?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                create_dir(parent)?;
            }
            fs::copy(&path, &target)
                .map_err(|e| format!("copy {} to {}: {e}", path.display(), target.display()))?;
        }
    }
    Ok(())
}

fn module_file_changed(original_root: &Path, temp_root: &Path, file: &str) -> bool {
    let original_path = original_root.join(file);
    let temp_path = temp_root.join(file);
    let original = fs::read(&original_path).ok();
    let temp = fs::read(&temp_path).ok();
    original != temp
}

fn run_quality_gates_to_report(
    inspection: &RepoInspection,
    worktree: &Path,
    report_path: &Path,
    domain_gate_opt_ins: &[String],
) -> Result<(), String> {
    let effective = inspection;
    let mut report = String::new();
    report.push_str(&format!(
        "stack={}\nconfidence={}\nworktree={}\n\n",
        effective.primary_stack.as_str(),
        effective.confidence,
        worktree.display()
    ));
    if effective.quality_plan.is_empty() && effective.domain_quality_plan.is_empty() {
        report.push_str("quality_plan=empty\n");
        write_file(report_path, &report)?;
        let err = "strict quality gate failed: no quality plan for detected stack".to_string();
        write_quality_status(report_path, false, Some(&err))?;
        return Err(err);
    }
    let typed_language_gates_present = effective
        .domain_quality_plan
        .iter()
        .any(|gate| gate.domain.starts_with("language.") && gate.command.as_deref().is_some());
    if typed_language_gates_present && !effective.quality_plan.is_empty() {
        report.push_str("legacy_stack_quality_plan=migrated_to_typed_domain_gates\n\n");
        write_file(report_path, &report)?;
    } else {
        for cmd in &effective.quality_plan {
            println!("quality gate: {} -> {}", cmd.name, cmd.command);
            let output = match run_quality_command(&cmd.name, &cmd.command, worktree) {
                Ok(output) => output,
                Err(err) => {
                    write_quality_status(report_path, false, Some(&err))?;
                    return Err(err);
                }
            };
            report.push_str(&format!(
                "## {}\ncommand={}\nstatus={}\nrequired={}\nfinal_gate={}\n\nstdout:\n{}\n\nstderr:\n{}\n\n",
                cmd.name,
                output.executed_command,
                output.status,
                cmd.required,
                cmd.non_mutating_final_gate,
                output.stdout,
                output.stderr
            ));
            write_file(report_path, &report)?;
            if cmd.required && !output.success {
                let report_ref = repo_relative_agentactr_path(report_path);
                let network_guidance =
                    quality_network_failure_guidance(&cmd.name, &output.stdout, &output.stderr);
                if let Some(guidance) = network_guidance.as_ref() {
                    report.push_str("\nnetwork_guidance:\n");
                    report.push_str(guidance);
                    report.push('\n');
                    write_file(report_path, &report)?;
                }
                let mut err = format!(
                    "strict quality gate failed: {} exited with {}; report={}",
                    cmd.name, output.status, report_ref
                );
                if let Some(guidance) = network_guidance {
                    err.push_str("; ");
                    err.push_str(&guidance);
                }
                write_quality_status(report_path, false, Some(&err))?;
                return Err(err);
            }
        }
    }
    for gate in &effective.domain_quality_plan {
        let Some(command) = gate.command.as_deref() else {
            report.push_str(&format!(
                "## domain:{}\ndomain={}\ntool={}\nstatus=finding-only\nrequired={}\nsetup_guidance={}\n\n",
                gate.name,
                gate.domain,
                gate.tool,
                gate.required,
                gate.setup_guidance.join(" | ")
            ));
            write_file(report_path, &report)?;
            continue;
        };
        let enabled_by_config = domain_gate_enabled_by_config(gate, domain_gate_opt_ins);
        if (gate.opt_in_required || gate.network_required || gate.credential_required)
            && !enabled_by_config
        {
            println!(
                "domain quality gate skipped: {} -> {} (opt_in={} network={} credentials={})",
                gate.name,
                command,
                gate.opt_in_required,
                gate.network_required,
                gate.credential_required
            );
            report.push_str(&format!(
                "## domain:{}\ndomain={}\ntool={}\ncommand={}\nstatus=skipped\nrequired={}\nopt_in_required={}\nnetwork_required={}\ncredential_required={}\nenabled_by_config=false\nsetup_guidance={}\n\n",
                gate.name,
                gate.domain,
                gate.tool,
                command,
                gate.required,
                gate.opt_in_required,
                gate.network_required,
                gate.credential_required,
                gate.setup_guidance.join(" | ")
            ));
            write_file(report_path, &report)?;
            continue;
        }
        println!("domain quality gate: {} -> {}", gate.name, command);
        let output = match run_quality_command(&gate.name, command, worktree) {
            Ok(output) => output,
            Err(err) => {
                write_quality_status(report_path, false, Some(&err))?;
                return Err(err);
            }
        };
        report.push_str(&format!(
            "## domain:{}\ndomain={}\ntool={}\ncommand={}\nstatus={}\nrequired={}\nmutates={}\nenabled_by_config={}\n\nstdout:\n{}\n\nstderr:\n{}\n\n",
            gate.name,
            gate.domain,
            gate.tool,
            output.executed_command,
            output.status,
            gate.required,
            gate.mutates,
            enabled_by_config,
            output.stdout,
            output.stderr
        ));
        write_file(report_path, &report)?;
        if gate.required && !output.success {
            let report_ref = repo_relative_agentactr_path(report_path);
            let mut err = format!(
                "strict domain quality gate failed: {} exited with {}; report={}",
                gate.name, output.status, report_ref
            );
            if gate.degraded_if_missing {
                err.push_str("; domain governance is degraded until the required toolchain/configuration is present");
            }
            write_quality_status(report_path, false, Some(&err))?;
            return Err(err);
        }
    }
    write_quality_status(report_path, true, None)?;
    Ok(())
}

fn domain_gate_enabled_by_config(
    gate: &agentactr_sdk::DomainQualityGate,
    opt_ins: &[String],
) -> bool {
    let gate_key = format!("{}:{}", gate.domain, gate.name);
    opt_ins.iter().any(|value| {
        let normalized = value.trim().to_ascii_lowercase();
        normalized == "all"
            || normalized == gate.domain
            || normalized == gate.name
            || normalized == gate_key
            || normalized == format!("{}:*", gate.domain)
    })
}

fn repo_relative_agentactr_path(path: &Path) -> String {
    let components: Vec<_> = path.components().collect();
    for (idx, component) in components.iter().enumerate() {
        if component.as_os_str() == ".agentactr" {
            let mut out = PathBuf::new();
            for component in &components[idx..] {
                out.push(component.as_os_str());
            }
            return out.display().to_string();
        }
    }
    path.display().to_string()
}

fn quality_network_failure_guidance(name: &str, stdout: &str, stderr: &str) -> Option<String> {
    let output = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    let network_markers = [
        "connectionrefused",
        "failedtoopensocket",
        "failed to open socket",
        "could not resolve",
        "couldn't resolve",
        "temporary failure in name resolution",
        "name or service not known",
        "network is unreachable",
        "failed to connect",
        "econnrefused",
        "enotfound",
        "etimedout",
        "timed out connecting",
        "dial tcp",
        "lookup ",
        "unable to access 'https://",
        "error sending request",
        "download of config.json failed",
        "downloading package manifest",
    ];
    if !network_markers.iter().any(|marker| output.contains(marker)) {
        return None;
    }
    Some(format!(
        "quality gate `{name}` appears to require network access; fail-closed/non-interactive runs do not request approval. Preinstall/cache dependencies or rerun with `--human-intervention interactive --codex-approval on-request`."
    ))
}

fn record_run_state(
    config: &AgentactrConfig,
    run_id: &str,
    repo: &str,
    issue: &str,
    status: &str,
    artifact_dir: &Path,
) -> Result<(), String> {
    use sqlx_core::row::Row;
    use sqlx_sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    let sqlite_path = Path::new(&config.observability.sqlite);
    if let Some(parent) = sqlite_path.parent() {
        create_dir(parent)?;
    }
    let url = format!("sqlite://{}", sqlite_path.display());
    let options = SqliteConnectOptions::from_str(&url)
        .map_err(|e| format!("configure SQLite run store {url}: {e}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("start Tokio runtime for run store: {e}"))?;
    runtime.block_on(async {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|e| format!("open SQLite run store {}: {e}", sqlite_path.display()))?;
        sqlx_core::query::query(
            r#"CREATE TABLE IF NOT EXISTS runs (
                run_id TEXT PRIMARY KEY,
                repo TEXT NOT NULL,
                issue TEXT NOT NULL,
                status TEXT NOT NULL,
                artifact_dir TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| format!("create runs table: {e}"))?;
        sqlx_core::query::query(
            r#"CREATE TABLE IF NOT EXISTS local_leases (
                repo TEXT NOT NULL,
                issue TEXT NOT NULL,
                run_id TEXT NOT NULL,
                fencing_token TEXT NOT NULL,
                status TEXT NOT NULL,
                expires_at_ms INTEGER NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                PRIMARY KEY (repo, issue)
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| format!("create local leases table: {e}"))?;
        sqlx_core::query::query(
            r#"CREATE UNIQUE INDEX IF NOT EXISTS local_leases_one_active_issue
               ON local_leases(repo, issue)
               WHERE status = 'active'"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| format!("create local lease uniqueness index: {e}"))?;
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| format!("begin run-state transaction: {e}"))?;
        sqlx_core::query::query(
            r#"INSERT INTO runs (run_id, repo, issue, status, artifact_dir)
               VALUES (?1, ?2, ?3, ?4, ?5)
               ON CONFLICT(run_id) DO UPDATE SET
                   status = excluded.status,
                   artifact_dir = excluded.artifact_dir,
                   updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')"#,
        )
        .bind(run_id)
        .bind(repo)
        .bind(issue)
        .bind(status)
        .bind(artifact_dir.display().to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("record run state: {e}"))?;
        let now_ms = i64::try_from(current_epoch_millis()).unwrap_or(i64::MAX);
        let lease_ttl_ms = i64::try_from(config.scheduling.lease_ttl_ms).unwrap_or(i64::MAX);
        let expires_at_ms = now_ms.saturating_add(lease_ttl_ms);
        sqlx_core::query::query(
            r#"UPDATE local_leases
               SET status = 'expired',
                   updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
               WHERE status = 'active' AND expires_at_ms <= ?1"#,
        )
        .bind(now_ms)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("expire local leases: {e}"))?;
        if status == "started" {
            if let Some(row) = sqlx_core::query::query(
                r#"SELECT run_id, expires_at_ms FROM local_leases
                   WHERE repo = ?1 AND issue = ?2 AND status = 'active' AND expires_at_ms > ?3
                   ORDER BY expires_at_ms DESC LIMIT 1"#,
            )
            .bind(repo)
            .bind(issue)
            .bind(now_ms)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| format!("check local lease: {e}"))?
            {
                let existing_run: String = row
                    .try_get("run_id")
                    .map_err(|e| format!("read local lease run_id: {e}"))?;
                if existing_run != run_id {
                    tx.rollback()
                        .await
                        .map_err(|e| format!("rollback local lease conflict: {e}"))?;
                    return Err(format!(
                        "local SQLite lease already active for {repo}#{issue} by run {existing_run}; this only coordinates local dispatch and does not prevent duplicate dispatch across machines"
                    ));
                }
            }
            println!(
                "warning: using local SQLite lease for {repo}#{issue}; duplicate dispatch is still possible across machines until GitHub claim mutation is enabled"
            );
            sqlx_core::query::query(
                r#"INSERT OR REPLACE INTO local_leases
                   (repo, issue, run_id, fencing_token, status, expires_at_ms)
                   VALUES (?1, ?2, ?3, ?4, 'active', ?5)"#,
            )
            .bind(repo)
            .bind(issue)
            .bind(run_id)
            .bind(format!("{run_id}:{now_ms}"))
            .bind(expires_at_ms)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("record local lease: {e}"))?;
        } else if status.ends_with("deferred") || status.ends_with("failed") || status == "completed" {
            sqlx_core::query::query(
                r#"UPDATE local_leases
                   SET status = ?4,
                       updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                   WHERE repo = ?1 AND issue = ?2 AND run_id = ?3"#,
            )
            .bind(repo)
            .bind(issue)
            .bind(run_id)
            .bind(status)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("update local lease: {e}"))?;
        }
        tx.commit()
            .await
            .map_err(|e| format!("commit run-state transaction: {e}"))?;
        Ok::<_, String>(())
    })?;
    append_run_event(
        &RunEventContext::root(config, run_id, repo, issue),
        "run.status.updated",
        serde_json::json!({
            "status": status,
            "artifact_dir": artifact_dir.display().to_string(),
        }),
    )?;
    info!(run_id, repo, issue, status, "recorded run state");
    Ok(())
}

fn load_issue_submission_ledger(
    config: &AgentactrConfig,
    issue_set_id: &str,
    proposal: &IssueProposal,
) -> Result<Option<IssueSubmissionLedgerEntry>, String> {
    use sqlx_core::row::Row;

    let key = agentactr_sdk::issue_submission_key(issue_set_id, proposal);
    with_issue_ledger_pool(config, |runtime, pool| {
        runtime.block_on(async {
            ensure_issue_submission_ledger_table(&pool).await?;
            let row = sqlx_core::query::query(
                r#"SELECT state, created_issue_number, created_issue_id, created_issue_url, detail
                   FROM issue_submission_ledger
                   WHERE issue_set_id = ?1 AND proposal_id = ?2 AND repo = ?3
                     AND parent_issue_key = ?4 AND proposal_digest = ?5"#,
            )
            .bind(&key.issue_set_id)
            .bind(key.proposal_id.as_str())
            .bind(&key.repo)
            .bind(&key.parent_issue_key)
            .bind(&key.proposal_digest)
            .fetch_optional(&pool)
            .await
            .map_err(|e| format!("read issue submission ledger: {e}"))?;
            let Some(row) = row else {
                return Ok(None);
            };
            let state: String = row
                .try_get("state")
                .map_err(|e| format!("read issue ledger state: {e}"))?;
            Ok(Some(IssueSubmissionLedgerEntry {
                key,
                state: parse_issue_submission_state(&state)?,
                created_issue_number: row
                    .try_get::<Option<i64>, _>("created_issue_number")
                    .map_err(|e| format!("read issue ledger number: {e}"))?
                    .and_then(|value| u64::try_from(value).ok()),
                created_issue_id: row
                    .try_get::<Option<i64>, _>("created_issue_id")
                    .map_err(|e| format!("read issue ledger id: {e}"))?
                    .and_then(|value| u64::try_from(value).ok()),
                created_issue_url: row
                    .try_get("created_issue_url")
                    .map_err(|e| format!("read issue ledger url: {e}"))?,
                detail: row
                    .try_get("detail")
                    .map_err(|e| format!("read issue ledger detail: {e}"))?,
            }))
        })
    })
}

fn begin_issue_submission(
    config: &AgentactrConfig,
    issue_set_id: &str,
    proposal: &IssueProposal,
    duplicate_reason: Option<&str>,
) -> Result<(), String> {
    let key = agentactr_sdk::issue_submission_key(issue_set_id, proposal);
    with_issue_ledger_pool(config, |runtime, pool| {
        runtime.block_on(async {
            ensure_issue_submission_ledger_table(&pool).await?;
            let mut tx = pool
                .begin()
                .await
                .map_err(|e| format!("begin issue submission ledger transaction: {e}"))?;
            let existing = sqlx_core::query::query(
                r#"SELECT state FROM issue_submission_ledger
                   WHERE issue_set_id = ?1 AND proposal_id = ?2 AND repo = ?3
                     AND parent_issue_key = ?4 AND proposal_digest = ?5"#,
            )
            .bind(&key.issue_set_id)
            .bind(key.proposal_id.as_str())
            .bind(&key.repo)
            .bind(&key.parent_issue_key)
            .bind(&key.proposal_digest)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| format!("read issue submission ledger for lock: {e}"))?;
            let submit_detail = duplicate_reason
                .filter(|reason| !reason.trim().is_empty())
                .map(|reason| format!(
                    "request submitted; duplicate_override_reason_sha256={}",
                    sha256_hex_bytes(reason.as_bytes())
                ))
                .unwrap_or_else(|| "request submitted".to_string());
            let begin_decision = if let Some(row) = existing {
                use sqlx_core::row::Row;
                let state: String = row
                    .try_get("state")
                    .map_err(|e| format!("read issue ledger state: {e}"))?;
                let entry = IssueSubmissionLedgerEntry {
                    key: key.clone(),
                    state: parse_issue_submission_state(&state)?,
                    created_issue_number: None,
                    created_issue_id: None,
                    created_issue_url: None,
                    detail: String::new(),
                };
                agentactr_sdk::plan_issue_submission_begin(Some(&entry))
            } else {
                agentactr_sdk::plan_issue_submission_begin(None)
            };
            match begin_decision {
                agentactr_sdk::IssueSubmissionBeginDecision::InsertSubmitted => {
                    sqlx_core::query::query(
                        r#"INSERT INTO issue_submission_ledger
                           (run_id, issue_set_id, proposal_id, repo, parent_issue, parent_issue_key, proposal_digest, state, detail)
                           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'submitted', ?8)"#,
                    )
                    .bind(&key.run_id)
                    .bind(&key.issue_set_id)
                    .bind(key.proposal_id.as_str())
                    .bind(&key.repo)
                    .bind(ledger_parent_issue_value(key.parent_issue))
                    .bind(&key.parent_issue_key)
                    .bind(&key.proposal_digest)
                    .bind(&submit_detail)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| format!("insert issue submission ledger: {e}"))?;
                }
                agentactr_sdk::IssueSubmissionBeginDecision::TransitionPendingToSubmitted => {
                    let affected = sqlx_core::query::query(
                        r#"UPDATE issue_submission_ledger
                           SET state = 'submitted',
                               detail = ?6,
                               updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                           WHERE issue_set_id = ?1 AND proposal_id = ?2 AND repo = ?3
                             AND parent_issue_key = ?4 AND proposal_digest = ?5
                             AND state = 'pending'"#,
                    )
                    .bind(&key.issue_set_id)
                    .bind(key.proposal_id.as_str())
                    .bind(&key.repo)
                    .bind(&key.parent_issue_key)
                    .bind(&key.proposal_digest)
                    .bind(&submit_detail)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| format!("transition pending issue submission ledger: {e}"))?
                    .rows_affected();
                    if affected != 1 {
                        tx.rollback()
                            .await
                            .map_err(|e| format!("rollback issue submission ledger CAS: {e}"))?;
                        return Err(
                            "issue submission ledger compare-and-set failed; retry after inspecting current ledger state"
                                .to_string(),
                        );
                    }
                }
                agentactr_sdk::IssueSubmissionBeginDecision::Blocked(reason) => {
                    tx.rollback()
                        .await
                        .map_err(|e| format!("rollback issue submission ledger lock: {e}"))?;
                    return Err(reason);
                }
            }
            tx.commit()
                .await
                .map_err(|e| format!("commit issue submission ledger lock: {e}"))
        })
    })
}

fn materialize_issue_submission_pending(
    config: &AgentactrConfig,
    issue_set_id: &str,
    proposals: &[IssueProposal],
) -> Result<(), String> {
    with_issue_ledger_pool(config, |runtime, pool| {
        runtime.block_on(async {
            ensure_issue_submission_ledger_table(&pool).await?;
            let mut tx = pool
                .begin()
                .await
                .map_err(|e| format!("begin issue proposal ledger materialization: {e}"))?;
            for proposal in proposals {
                let key = agentactr_sdk::issue_submission_key(issue_set_id, proposal);
                sqlx_core::query::query(
                    r#"INSERT OR IGNORE INTO issue_submission_ledger
                       (run_id, issue_set_id, proposal_id, repo, parent_issue, parent_issue_key, proposal_digest, state, detail)
                       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', 'proposal materialized')"#,
                )
                .bind(&key.run_id)
                .bind(&key.issue_set_id)
                .bind(key.proposal_id.as_str())
                .bind(&key.repo)
                .bind(ledger_parent_issue_value(key.parent_issue))
                .bind(&key.parent_issue_key)
                .bind(&key.proposal_digest)
                .execute(&mut *tx)
                .await
                .map_err(|e| format!("materialize issue submission ledger pending row: {e}"))?;
            }
            tx.commit()
                .await
                .map_err(|e| format!("commit issue proposal ledger materialization: {e}"))
        })
    })
}

fn record_issue_submission_state(
    config: &AgentactrConfig,
    issue_set_id: &str,
    proposal: &IssueProposal,
    state: IssueSubmissionLedgerState,
    issue: Option<&agentactr_sdk::Issue>,
    tracker_issue_id: Option<u64>,
    detail: &str,
) -> Result<(), String> {
    let key = agentactr_sdk::issue_submission_key(issue_set_id, proposal);
    with_issue_ledger_pool(config, |runtime, pool| {
        runtime.block_on(async {
            ensure_issue_submission_ledger_table(&pool).await?;
            sqlx_core::query::query(
                r#"UPDATE issue_submission_ledger
                   SET state = ?6,
                       created_issue_number = COALESCE(?7, created_issue_number),
                       created_issue_id = COALESCE(?8, created_issue_id),
                       created_issue_url = COALESCE(?9, created_issue_url),
                       detail = ?10,
                       updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                   WHERE issue_set_id = ?1 AND proposal_id = ?2 AND repo = ?3
                     AND parent_issue_key = ?4 AND proposal_digest = ?5"#,
            )
            .bind(&key.issue_set_id)
            .bind(key.proposal_id.as_str())
            .bind(&key.repo)
            .bind(&key.parent_issue_key)
            .bind(&key.proposal_digest)
            .bind(state.as_str())
            .bind(issue.and_then(|issue| i64::try_from(issue.number).ok()))
            .bind(tracker_issue_id.and_then(|id| i64::try_from(id).ok()))
            .bind(issue.and_then(|issue| issue.html_url.clone()))
            .bind(detail)
            .execute(&pool)
            .await
            .map_err(|e| format!("record issue submission ledger state: {e}"))?;
            Ok(())
        })
    })
}

fn with_issue_ledger_pool<T>(
    config: &AgentactrConfig,
    f: impl FnOnce(tokio::runtime::Runtime, sqlx_sqlite::SqlitePool) -> Result<T, String>,
) -> Result<T, String> {
    use sqlx_sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    let sqlite_path = Path::new(&config.observability.sqlite);
    if let Some(parent) = sqlite_path.parent() {
        create_dir(parent)?;
    }
    let url = format!("sqlite://{}", sqlite_path.display());
    let options = SqliteConnectOptions::from_str(&url)
        .map_err(|e| format!("configure SQLite issue ledger {url}: {e}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("start Tokio runtime for issue ledger: {e}"))?;
    let pool = runtime.block_on(async {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|e| format!("open SQLite issue ledger {}: {e}", sqlite_path.display()))
    })?;
    f(runtime, pool)
}

async fn ensure_issue_submission_ledger_table(
    pool: &sqlx_sqlite::SqlitePool,
) -> Result<(), String> {
    sqlx_core::query::query(
        r#"CREATE TABLE IF NOT EXISTS issue_submission_ledger (
            run_id TEXT NOT NULL,
            proposal_id TEXT NOT NULL,
            repo TEXT NOT NULL,
            parent_issue INTEGER NOT NULL,
            proposal_digest TEXT NOT NULL,
            state TEXT NOT NULL,
            created_issue_number INTEGER,
            created_issue_id INTEGER,
            created_issue_url TEXT,
            detail TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (run_id, proposal_id, repo, parent_issue, proposal_digest)
        )"#,
    )
    .execute(pool)
    .await
    .map_err(|e| format!("create issue submission ledger table: {e}"))?;
    add_issue_ledger_column(pool, "issue_set_id", "TEXT").await?;
    add_issue_ledger_column(pool, "parent_issue_key", "TEXT").await?;
    sqlx_core::query::query(
        r#"UPDATE issue_submission_ledger
           SET issue_set_id = COALESCE(issue_set_id, run_id),
               parent_issue_key = COALESCE(
                   parent_issue_key,
                   CASE
                       WHEN parent_issue = 0 THEN 'top_level'
                       ELSE 'parent:' || CAST(parent_issue AS TEXT)
                   END
               )
           WHERE issue_set_id IS NULL OR parent_issue_key IS NULL"#,
    )
    .execute(pool)
    .await
    .map_err(|e| format!("backfill issue submission ledger columns: {e}"))?;
    Ok(())
}

async fn add_issue_ledger_column(
    pool: &sqlx_sqlite::SqlitePool,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let escaped = column.replace('\'', "''");
    use sqlx_core::row::Row;

    let row = sqlx_core::query::query(&format!(
        "SELECT COUNT(*) FROM pragma_table_info('issue_submission_ledger') WHERE name = '{escaped}'"
    ))
    .fetch_one(pool)
    .await
    .map_err(|e| format!("inspect issue submission ledger schema: {e}"))?;
    let exists: i64 = row
        .try_get(0)
        .map_err(|e| format!("read issue submission ledger schema count: {e}"))?;
    if exists == 0 {
        sqlx_core::query::query(&format!(
            "ALTER TABLE issue_submission_ledger ADD COLUMN {column} {definition}"
        ))
        .execute(pool)
        .await
        .map_err(|e| format!("migrate issue submission ledger column {column}: {e}"))?;
    }
    Ok(())
}

fn ledger_parent_issue_value(parent_issue: Option<u64>) -> i64 {
    parent_issue
        .and_then(|issue| i64::try_from(issue).ok())
        .unwrap_or(0)
}

fn parse_issue_submission_state(value: &str) -> Result<IssueSubmissionLedgerState, String> {
    match value {
        "pending" => Ok(IssueSubmissionLedgerState::Pending),
        "submitted" => Ok(IssueSubmissionLedgerState::Submitted),
        "created" => Ok(IssueSubmissionLedgerState::Created),
        "linked" => Ok(IssueSubmissionLedgerState::Linked),
        "created_unlinked" => Ok(IssueSubmissionLedgerState::CreatedUnlinked),
        "created_metadata_mismatch" => Ok(IssueSubmissionLedgerState::CreatedMetadataMismatch),
        "failed" => Ok(IssueSubmissionLedgerState::Failed),
        _ => Err(format!("unknown issue submission ledger state `{value}`")),
    }
}

struct LocalLeaseHeartbeat {
    stop: mpsc::Sender<()>,
    failure: Arc<Mutex<Option<String>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl LocalLeaseHeartbeat {
    fn start(config: &AgentactrConfig, run_id: &str, repo: &str, issue: &str) -> Self {
        let (stop, stop_rx) = mpsc::channel();
        let failure = Arc::new(Mutex::new(None));
        let worker_failure = Arc::clone(&failure);
        let config = config.clone();
        let run_id = run_id.to_string();
        let repo = repo.to_string();
        let issue = issue.to_string();
        let interval = lease_heartbeat_interval(config.scheduling.lease_ttl_ms);
        let handle = thread::spawn(move || loop {
            match stop_rx.recv_timeout(interval) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Err(err) = refresh_local_lease(&config, &run_id, &repo, &issue) {
                        if let Ok(mut failure) = worker_failure.lock() {
                            *failure = Some(err);
                        }
                        break;
                    }
                }
            }
        });
        Self {
            stop,
            failure,
            handle: Some(handle),
        }
    }

    fn stop(mut self) -> Result<(), String> {
        let _ = self.stop.send(());
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| "local lease heartbeat thread panicked".to_string())?;
        }
        if let Some(err) = self.failure.lock().ok().and_then(|failure| failure.clone()) {
            Err(format!("local lease heartbeat failed: {err}"))
        } else {
            Ok(())
        }
    }
}

fn lease_heartbeat_interval(lease_ttl_ms: u64) -> Duration {
    let interval_ms = (lease_ttl_ms / 3).clamp(1_000, 60_000);
    Duration::from_millis(interval_ms)
}

fn refresh_local_lease(
    config: &AgentactrConfig,
    run_id: &str,
    repo: &str,
    issue: &str,
) -> Result<(), String> {
    use sqlx_sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    let sqlite_path = Path::new(&config.observability.sqlite);
    let url = format!("sqlite://{}", sqlite_path.display());
    let options = SqliteConnectOptions::from_str(&url)
        .map_err(|e| format!("configure SQLite run store {url}: {e}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("start Tokio runtime for lease refresh: {e}"))?;
    runtime.block_on(async {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|e| format!("open SQLite run store {}: {e}", sqlite_path.display()))?;
        let now_ms = i64::try_from(current_epoch_millis()).unwrap_or(i64::MAX);
        let lease_ttl_ms = i64::try_from(config.scheduling.lease_ttl_ms).unwrap_or(i64::MAX);
        let expires_at_ms = now_ms.saturating_add(lease_ttl_ms);
        sqlx_core::query::query(
            r#"UPDATE local_leases
               SET expires_at_ms = ?4,
                   updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
               WHERE repo = ?1 AND issue = ?2 AND run_id = ?3 AND status = 'active'"#,
        )
        .bind(repo)
        .bind(issue)
        .bind(run_id)
        .bind(expires_at_ms)
        .execute(&pool)
        .await
        .map_err(|e| format!("refresh local lease: {e}"))?;
        Ok::<_, String>(())
    })
}

fn record_phase<T>(
    config: &AgentactrConfig,
    run_id: &str,
    repo: &str,
    issue: &str,
    artifact_dir: &Path,
    phase: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    append_run_event(
        &RunEventContext::root(config, run_id, repo, issue),
        "phase.started",
        serde_json::json!({ "phase": phase }),
    )?;
    match operation() {
        Ok(value) => {
            append_run_event(
                &RunEventContext::root(config, run_id, repo, issue),
                "phase.completed",
                serde_json::json!({ "phase": phase }),
            )?;
            Ok(value)
        }
        Err(err) => {
            let status = format!("{phase}_failed");
            match record_run_state(config, run_id, repo, issue, &status, artifact_dir) {
                Ok(()) => {
                    append_run_event(
                        &RunEventContext::root(config, run_id, repo, issue),
                        "phase.failed",
                        serde_json::json!({
                            "phase": phase,
                            "error": err.clone(),
                        }),
                    )?;
                    Err(err)
                }
                Err(record_err) => Err(format!(
                    "{err}; additionally failed to record run status `{status}`: {record_err}"
                )),
            }
        }
    }
}

#[derive(Clone)]
struct RunArtifactContext {
    run_id: String,
    repo: String,
    issue: String,
    artifact_dir: PathBuf,
    manifest_path: PathBuf,
    worktree: PathBuf,
    base_commit: String,
}

fn load_run_artifact_context(
    config: &AgentactrConfig,
    run_id: &str,
) -> Result<RunArtifactContext, String> {
    let artifact_dir = run_artifact_dir(config, run_id)?;
    let manifest_path = artifact_dir.join("context_manifest.json");
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("read run context manifest {}: {e}", manifest_path.display()))?;
    let manifest = serde_json::from_str::<serde_json::Value>(&manifest_text).map_err(|e| {
        format!(
            "parse run context manifest {}: {e}",
            manifest_path.display()
        )
    })?;
    let manifest_run_id = required_manifest_string(&manifest, "/run_id")?;
    if manifest_run_id != run_id {
        return Err(format!(
            "run context manifest {} is for run_id `{manifest_run_id}`, not `{run_id}`",
            manifest_path.display()
        ));
    }
    let repo = required_manifest_string(&manifest, "/repo")?;
    let issue = required_manifest_string(&manifest, "/issue")?;
    let worktree = PathBuf::from(required_manifest_string(&manifest, "/worktree/path")?);
    let base_commit = required_manifest_string(&manifest, "/worktree/base_commit")?;
    if let Some(manifest_worktree_run_id) = manifest
        .pointer("/worktree/run_id")
        .and_then(serde_json::Value::as_str)
    {
        if manifest_worktree_run_id != run_id {
            return Err(format!(
                "run context manifest {} has worktree.run_id `{manifest_worktree_run_id}`, not `{run_id}`",
                manifest_path.display()
            ));
        }
    }
    Ok(RunArtifactContext {
        run_id: run_id.to_string(),
        repo,
        issue,
        artifact_dir,
        manifest_path,
        worktree,
        base_commit,
    })
}

fn required_manifest_string(manifest: &serde_json::Value, pointer: &str) -> Result<String, String> {
    manifest
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| format!("run context manifest is missing string field `{pointer}`"))
}

fn validate_run_worktree_scope(
    config: &AgentactrConfig,
    context: &RunArtifactContext,
) -> Result<PathBuf, String> {
    let worktree_root = resolve_config_path(&config.vcs.worktree_root)?;
    let worktree = resolve_path_against_cwd(&context.worktree)?;
    let lexical_root = normalize_path_lexically(&worktree_root).ok_or_else(|| {
        format!(
            "invalid configured vcs.worktree_root `{}`",
            worktree_root.display()
        )
    })?;
    let lexical_worktree = normalize_path_lexically(&worktree).ok_or_else(|| {
        format!(
            "invalid worktree path `{}` in {}",
            context.worktree.display(),
            context.manifest_path.display()
        )
    })?;
    if !lexical_worktree.starts_with(&lexical_root) {
        return Err(format!(
            "run worktree {} is outside configured vcs.worktree_root {}",
            context.worktree.display(),
            worktree_root.display()
        ));
    }
    let canonical_root = worktree_root.canonicalize().map_err(|e| {
        format!(
            "canonicalize vcs.worktree_root {}: {e}",
            worktree_root.display()
        )
    })?;
    let canonical_worktree = lexical_worktree.canonicalize().map_err(|e| {
        format!(
            "canonicalize run worktree {} from {}: {e}",
            context.worktree.display(),
            context.manifest_path.display()
        )
    })?;
    if !canonical_worktree.starts_with(&canonical_root) {
        return Err(format!(
            "run worktree {} resolves outside configured vcs.worktree_root {}",
            context.worktree.display(),
            worktree_root.display()
        ));
    }
    if !canonical_worktree.is_dir() {
        return Err(format!(
            "run worktree {} is not a directory",
            canonical_worktree.display()
        ));
    }
    Ok(canonical_worktree)
}

fn resolve_path_against_cwd(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()
            .map_err(|e| format!("resolve current directory: {e}"))?
            .join(path))
    }
}

struct VcsStatus {
    run_id: String,
    repo: String,
    issue: String,
    artifact_dir: PathBuf,
    manifest_path: PathBuf,
    worktree: PathBuf,
    base_commit: String,
    current_commit: String,
    branch_name: String,
    touched_files: Vec<String>,
    source_checkout_clean_at_prepare: Option<bool>,
}

impl VcsStatus {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "run_id": self.run_id,
            "repo": self.repo,
            "issue": self.issue,
            "artifact_dir": self.artifact_dir.display().to_string(),
            "context_manifest": self.manifest_path.display().to_string(),
            "worktree": self.worktree.display().to_string(),
            "base_commit": self.base_commit,
            "current_commit": self.current_commit,
            "branch_name": self.branch_name,
            "touched_files": self.touched_files,
            "touched_file_count": self.touched_files.len(),
            "source_checkout_clean_at_prepare": self.source_checkout_clean_at_prepare,
            "cross_issue_overlap": "not_implemented_in_this_milestone",
            "merge_plan": "implemented_read_only",
        })
    }
}

struct VcsInventoryEntry {
    run_id: String,
    repo: Option<String>,
    issue: Option<String>,
    artifact_dir: PathBuf,
    context_manifest: PathBuf,
    worktree: Option<PathBuf>,
    base_commit: Option<String>,
    current_commit: Option<String>,
    branch_name: Option<String>,
    touched_file_count: Option<usize>,
    source_checkout_clean_at_prepare: Option<bool>,
    last_run_status: String,
    valid: bool,
    error: Option<String>,
}

impl VcsInventoryEntry {
    fn from_status(status: VcsStatus, last_run_status: String) -> Self {
        Self {
            run_id: status.run_id,
            repo: Some(status.repo),
            issue: Some(status.issue),
            artifact_dir: status.artifact_dir,
            context_manifest: status.manifest_path,
            worktree: Some(status.worktree),
            base_commit: Some(status.base_commit),
            current_commit: Some(status.current_commit),
            branch_name: Some(status.branch_name),
            touched_file_count: Some(status.touched_files.len()),
            source_checkout_clean_at_prepare: status.source_checkout_clean_at_prepare,
            last_run_status,
            valid: true,
            error: None,
        }
    }

    fn invalid(
        run_id: String,
        artifact_dir: PathBuf,
        context_manifest: PathBuf,
        last_run_status: String,
        error: String,
    ) -> Self {
        Self {
            run_id,
            repo: None,
            issue: None,
            artifact_dir,
            context_manifest,
            worktree: None,
            base_commit: None,
            current_commit: None,
            branch_name: None,
            touched_file_count: None,
            source_checkout_clean_at_prepare: None,
            last_run_status,
            valid: false,
            error: Some(error),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "run_id": self.run_id,
            "repo": self.repo,
            "issue": self.issue,
            "artifact_dir": self.artifact_dir.display().to_string(),
            "context_manifest": self.context_manifest.display().to_string(),
            "worktree": self.worktree.as_ref().map(|path| path.display().to_string()),
            "base_commit": self.base_commit,
            "current_commit": self.current_commit,
            "branch_name": self.branch_name,
            "touched_file_count": self.touched_file_count,
            "source_checkout_clean_at_prepare": self.source_checkout_clean_at_prepare,
            "last_run_status": self.last_run_status,
            "valid": self.valid,
            "error": self.error,
        })
    }
}

fn collect_vcs_status(context: &RunArtifactContext) -> Result<VcsStatus, String> {
    if !context.worktree.is_dir() {
        return Err(format!(
            "run {} worktree is missing or not a directory: {}",
            context.run_id,
            context.worktree.display()
        ));
    }
    let current_commit = git_output_in_dir(&context.worktree, &["rev-parse", "HEAD"])?;
    let branch_name = read_worktree_metadata_string(&context.worktree, "branch_name")?
        .unwrap_or_else(|| {
            git_output_in_dir(&context.worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap_or_else(|_| "unknown".to_string())
        });
    let source_checkout_clean_at_prepare =
        read_worktree_metadata_bool(&context.worktree, "source_checkout_clean_at_prepare")?;
    let touched_files = git_touched_files(&context.worktree)?;
    Ok(VcsStatus {
        run_id: context.run_id.clone(),
        repo: context.repo.clone(),
        issue: context.issue.clone(),
        artifact_dir: context.artifact_dir.clone(),
        manifest_path: context.manifest_path.clone(),
        worktree: context.worktree.clone(),
        base_commit: context.base_commit.clone(),
        current_commit,
        branch_name,
        touched_files,
        source_checkout_clean_at_prepare,
    })
}

fn collect_workspace_diff(context: &RunArtifactContext) -> Result<WorkspaceDiff, String> {
    LocalGitAdapter.diff(&WorktreeRef {
        path: context.worktree.clone(),
        base_commit: context.base_commit.clone(),
        run_id: context.run_id.clone(),
    })
}

fn record_workspace_diff_artifacts(
    config: &AgentactrConfig,
    context: &RunArtifactContext,
    diff: &WorkspaceDiff,
    output_path: Option<&str>,
) -> Result<(PathBuf, PathBuf), String> {
    let patch_path = match output_path {
        Some(path) => resolve_vcs_output_path(path)?,
        None => context.artifact_dir.join("workspace.diff.patch"),
    };
    let metadata_path = context.artifact_dir.join("workspace.diff.metadata.json");
    if let Some(parent) = patch_path.parent() {
        create_dir(parent)?;
    }
    write_file(&patch_path, &diff.patch)?;
    let metadata = workspace_diff_metadata(context, diff, &patch_path, &metadata_path);
    write_file(
        &metadata_path,
        &serde_json::to_string_pretty(&metadata)
            .map_err(|e| format!("render workspace diff metadata: {e}"))?,
    )?;
    append_run_event(
        &RunEventContext::root(config, &context.run_id, &context.repo, &context.issue),
        "vcs.diff.recorded",
        metadata,
    )?;
    Ok((patch_path, metadata_path))
}

fn workspace_diff_metadata(
    context: &RunArtifactContext,
    diff: &WorkspaceDiff,
    patch_path: &Path,
    metadata_path: &Path,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": "0.1",
        "run_id": context.run_id.as_str(),
        "repo": context.repo.as_str(),
        "issue": context.issue.as_str(),
        "artifact": patch_path.display().to_string(),
        "metadata_artifact": metadata_path.display().to_string(),
        "context_manifest": context.manifest_path.display().to_string(),
        "worktree": diff.worktree.display().to_string(),
        "base_commit": diff.base_commit.as_str(),
        "current_commit": diff.current_commit.as_str(),
        "patch_bytes": diff.patch.len(),
        "patch_sha256": format!("sha256:{}", sha256_hex_bytes(diff.patch.as_bytes())),
        "touched_files": diff.touched_files.clone(),
        "touched_file_count": diff.touched_files.len(),
        "untracked_files": diff.untracked_files.clone(),
        "untracked_file_count": diff.untracked_files.len(),
        "is_empty": diff.is_empty,
        "includes_untracked_file_bodies": true,
        "diff_kind": "git_diff_binary_base_to_worktree",
        "side_effect_level": "writes_diff_artifact_and_trace",
    })
}

fn resolve_vcs_output_path(path: &str) -> Result<PathBuf, String> {
    let raw = Path::new(path);
    if path.trim().is_empty() {
        return Err("--output requires a non-empty path".to_string());
    }
    if raw.is_dir() {
        return Err(format!("--output path is a directory: {}", raw.display()));
    }
    if raw.exists() {
        let metadata = fs::symlink_metadata(raw)
            .map_err(|e| format!("inspect --output {}: {e}", raw.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "--output path must not be a symlink: {}",
                raw.display()
            ));
        }
    }
    let cwd = env::current_dir().map_err(|e| format!("resolve current directory: {e}"))?;
    let resolved = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        cwd.join(raw)
    };
    let lexical_cwd = normalize_path_lexically(&cwd)
        .ok_or_else(|| "current directory is not a valid path".to_string())?;
    let lexical_resolved = normalize_path_lexically(&resolved)
        .ok_or_else(|| format!("invalid --output path `{path}`"))?;
    if !lexical_resolved.starts_with(&lexical_cwd) {
        return Err(format!(
            "--output path {} is outside the current repository checkout",
            raw.display()
        ));
    }
    Ok(lexical_resolved)
}

fn collect_merge_plan(
    config: &AgentactrConfig,
    context: &RunArtifactContext,
) -> Result<MergePlan, String> {
    LocalGitAdapter.merge_plan(MergePlanRequest {
        worktree: WorktreeRef {
            path: context.worktree.clone(),
            base_commit: context.base_commit.clone(),
            run_id: context.run_id.clone(),
        },
        base_ref: config.vcs.base_ref.clone(),
        merge_mode: config.merge.mode.clone(),
        workspace_diff_artifact: Some(context.artifact_dir.join("workspace.diff.patch")),
    })
}

fn record_merge_plan_artifact(
    config: &AgentactrConfig,
    context: &RunArtifactContext,
    plan: &MergePlan,
) -> Result<PathBuf, String> {
    let path = context.artifact_dir.join("merge_plan.json");
    let metadata_path = context.artifact_dir.join("merge_plan.metadata.json");
    let payload = merge_plan_payload(context, plan, &path);
    let rendered =
        serde_json::to_string_pretty(&payload).map_err(|e| format!("render merge plan: {e}"))?;
    let metadata = merge_plan_metadata(context, plan, &path, &metadata_path, &rendered);
    write_file(&path, &rendered)?;
    write_file(
        &metadata_path,
        &serde_json::to_string_pretty(&metadata)
            .map_err(|e| format!("render merge plan metadata: {e}"))?,
    )?;
    append_run_event(
        &RunEventContext::root(config, &context.run_id, &context.repo, &context.issue),
        "vcs.merge_plan.recorded",
        metadata,
    )?;
    Ok(path)
}

fn merge_plan_payload(
    context: &RunArtifactContext,
    plan: &MergePlan,
    artifact_path: &Path,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": "0.1",
        "run_id": context.run_id.as_str(),
        "repo": context.repo.as_str(),
        "issue": context.issue.as_str(),
        "artifact": artifact_path.display().to_string(),
        "context_manifest": context.manifest_path.display().to_string(),
        "worktree": plan.worktree.display().to_string(),
        "base_ref": plan.base_ref.as_str(),
        "base_commit": plan.base_commit.as_str(),
        "current_commit": plan.current_commit.as_str(),
        "base_ref_current_commit": plan.base_ref_current_commit.as_str(),
        "base_ref_drifted": plan.base_ref_drifted,
        "head_contains_base_ref": plan.head_contains_base_ref,
        "merge_mode": plan.merge_mode.as_str(),
        "merge_enabled": plan.merge_enabled,
        "workspace_diff_artifact": plan
            .workspace_diff_artifact
            .as_ref()
            .map(|path| path.display().to_string()),
        "workspace_diff_exists": plan.workspace_diff_exists,
        "touched_files": plan.touched_files.clone(),
        "touched_file_count": plan.touched_files.len(),
        "blockers": plan.blockers.clone(),
        "warnings": plan.warnings.clone(),
        "recommendation": plan.recommendation.as_str(),
        "side_effect_level": "writes_merge_plan_artifact_and_trace",
    })
}

fn merge_plan_metadata(
    context: &RunArtifactContext,
    plan: &MergePlan,
    artifact_path: &Path,
    metadata_path: &Path,
    rendered_plan: &str,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": "0.1",
        "run_id": context.run_id.as_str(),
        "repo": context.repo.as_str(),
        "issue": context.issue.as_str(),
        "artifact": artifact_path.display().to_string(),
        "metadata_artifact": metadata_path.display().to_string(),
        "context_manifest": context.manifest_path.display().to_string(),
        "artifact_sha256": format!("sha256:{}", sha256_hex_bytes(rendered_plan.as_bytes())),
        "artifact_bytes": rendered_plan.len(),
        "artifact_chars": rendered_plan.chars().count(),
        "recommendation": plan.recommendation.as_str(),
        "blocker_count": plan.blockers.len(),
        "warning_count": plan.warnings.len(),
        "merge_mode": plan.merge_mode.as_str(),
        "merge_enabled": plan.merge_enabled,
        "workspace_diff_exists": plan.workspace_diff_exists,
        "required": false,
        "side_effect_level": "writes_merge_plan_artifact_and_trace",
    })
}

fn print_merge_plan(plan: &MergePlan, artifact_path: &Path) {
    println!("merge_plan={}", artifact_path.display());
    println!("run_id={}", plan.run_id);
    println!("worktree={}", plan.worktree.display());
    println!("base_ref={}", plan.base_ref);
    println!("base_commit={}", plan.base_commit);
    println!("current_commit={}", plan.current_commit);
    println!("base_ref_current_commit={}", plan.base_ref_current_commit);
    println!("base_ref_drifted={}", plan.base_ref_drifted);
    println!("head_contains_base_ref={}", plan.head_contains_base_ref);
    println!("merge_mode={}", plan.merge_mode);
    println!("merge_enabled={}", plan.merge_enabled);
    println!("workspace_diff_exists={}", plan.workspace_diff_exists);
    println!("touched_file_count={}", plan.touched_files.len());
    println!("recommendation={}", plan.recommendation);
    for blocker in &plan.blockers {
        println!("blocker={blocker}");
    }
    for warning in &plan.warnings {
        println!("warning={warning}");
    }
}

fn print_merge_plan_json(
    context: &RunArtifactContext,
    plan: &MergePlan,
    artifact_path: &Path,
) -> Result<(), String> {
    let payload = merge_plan_payload(context, plan, artifact_path);
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).map_err(|e| format!("render merge plan: {e}"))?
    );
    Ok(())
}

fn print_vcs_status(status: &VcsStatus) {
    println!("run_id={}", status.run_id);
    println!("repo={}", status.repo);
    println!("issue={}", status.issue);
    println!("artifact_dir={}", status.artifact_dir.display());
    println!("context_manifest={}", status.manifest_path.display());
    println!("worktree={}", status.worktree.display());
    println!("base_commit={}", status.base_commit);
    println!("current_commit={}", status.current_commit);
    println!("branch_name={}", status.branch_name);
    println!(
        "source_checkout_clean_at_prepare={}",
        status
            .source_checkout_clean_at_prepare
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
    println!("touched_file_count={}", status.touched_files.len());
    for path in &status.touched_files {
        println!("touched_file={path}");
    }
    println!("cross_issue_overlap=not_implemented_in_this_milestone");
    println!("merge_plan=implemented_read_only");
}

fn git_output_in_dir(worktree: &Path, args: &[&str]) -> Result<String, String> {
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
            "git -C {} {} exited with {}: {}",
            worktree.display(),
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn collect_vcs_inventory(config: &AgentactrConfig) -> Result<Vec<VcsInventoryEntry>, String> {
    let artifact_root = resolve_config_path(&config.observability.artifact_root)?;
    let trace_records = read_trace_records(&run_trace_path(config)?)?;
    let run_statuses = latest_run_statuses(&trace_records);
    let entries = match fs::read_dir(&artifact_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(format!(
                "read artifact root {} for VCS inventory: {err}",
                artifact_root.display()
            ))
        }
    };
    let mut run_ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read artifact root entry: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("read artifact entry type {}: {e}", entry.path().display()))?;
        if !file_type.is_dir() {
            continue;
        }
        if let Some(run_id) = entry.file_name().to_str() {
            run_ids.push(run_id.to_string());
        }
    }
    run_ids.sort();

    let mut out = Vec::new();
    for run_id in run_ids {
        let last_run_status = run_statuses
            .get(&run_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let artifact_dir = artifact_root.join(&run_id);
        let context_manifest = artifact_dir.join("context_manifest.json");
        if let Err(err) = validate_run_id(&run_id) {
            out.push(VcsInventoryEntry::invalid(
                run_id,
                artifact_dir,
                context_manifest,
                last_run_status,
                err,
            ));
            continue;
        }
        let mut context = match load_run_artifact_context(config, &run_id) {
            Ok(context) => context,
            Err(err) => {
                out.push(VcsInventoryEntry::invalid(
                    run_id,
                    artifact_dir,
                    context_manifest,
                    last_run_status,
                    err,
                ));
                continue;
            }
        };
        context.worktree = match validate_run_worktree_scope(config, &context) {
            Ok(worktree) => worktree,
            Err(err) => {
                out.push(VcsInventoryEntry::invalid(
                    run_id,
                    context.artifact_dir,
                    context.manifest_path,
                    last_run_status,
                    err,
                ));
                continue;
            }
        };
        match collect_vcs_status(&context) {
            Ok(status) => out.push(VcsInventoryEntry::from_status(status, last_run_status)),
            Err(err) => out.push(VcsInventoryEntry::invalid(
                run_id,
                context.artifact_dir,
                context.manifest_path,
                last_run_status,
                err,
            )),
        }
    }
    Ok(out)
}

fn latest_run_statuses(records: &[TraceRecord]) -> HashMap<String, String> {
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

fn latest_run_status(records: &[TraceRecord], run_id: &str) -> String {
    records
        .iter()
        .rev()
        .find(|record| record.run_id == run_id && record.event_type == "run.status.updated")
        .and_then(|record| record.value.pointer("/payload/status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

fn git_touched_files(worktree: &Path) -> Result<Vec<String>, String> {
    let output = git_output_in_dir(worktree, &["status", "--porcelain"])?;
    Ok(output
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
        .collect())
}

fn read_worktree_metadata_string(worktree: &Path, key: &str) -> Result<Option<String>, String> {
    read_worktree_metadata_value(worktree, key)
        .map(|value| value.and_then(|value| value.as_str().map(ToString::to_string)))
}

fn read_worktree_metadata_bool(worktree: &Path, key: &str) -> Result<Option<bool>, String> {
    read_worktree_metadata_value(worktree, key).map(|value| value.and_then(|value| value.as_bool()))
}

fn read_worktree_metadata_value(worktree: &Path, key: &str) -> Result<Option<toml::Value>, String> {
    Ok(read_worktree_metadata_document(worktree)?.and_then(|parsed| parsed.get(key).cloned()))
}

fn read_worktree_metadata_document(worktree: &Path) -> Result<Option<toml::Value>, String> {
    let path = worktree.join(".agentactr-run.toml");
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("read worktree metadata {}: {e}", path.display()))?;
    let parsed = parse_toml_document(&content)
        .map_err(|e| format!("parse worktree metadata {}: {e}", path.display()))?;
    Ok(Some(parsed))
}

#[derive(Clone, Debug)]
struct TraceRecord {
    line_number: usize,
    value: serde_json::Value,
    run_id: String,
    issue_id: Option<String>,
    agent_run_id: Option<String>,
    event_type: String,
    ts: Option<String>,
    ts_unix_ms: Option<u128>,
}

#[derive(Clone, Debug)]
struct TraceRunSummary {
    run_id: String,
    issue_id: Option<String>,
    event_count: usize,
    first_ts: Option<String>,
    last_ts: Option<String>,
    last_event_type: Option<String>,
    last_ts_unix_ms: Option<u128>,
}

fn cmd_trace(args: &mut [String]) -> Result<(), String> {
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
                .and_then(|context| collect_artifact_integrity(&context))
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

fn read_trace_records(trace_path: &Path) -> Result<Vec<TraceRecord>, String> {
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

fn summarize_trace_runs(records: &[TraceRecord]) -> Vec<TraceRunSummary> {
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

fn print_trace_show(
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

fn json_display(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(serde_json::Value::Number(value)) => value.to_string(),
        Some(serde_json::Value::Bool(value)) => value.to_string(),
        Some(serde_json::Value::Null) | None => "none".to_string(),
        Some(value) => compact_json(value),
    }
}

fn compact_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "unrenderable".to_string())
}

struct DebugBundleReport {
    bundle_dir: PathBuf,
    copied_files: Vec<String>,
    trace_events: usize,
    redacted: bool,
}

fn cmd_debug(args: &mut [String]) -> Result<(), String> {
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

fn create_debug_bundle(
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

    let artifact_integrity = collect_artifact_integrity(&context)?;
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

fn collect_artifact_integrity(context: &RunArtifactContext) -> Result<serde_json::Value, String> {
    artifacts::collect_artifact_integrity(&ArtifactIntegrityContext {
        run_id: &context.run_id,
        artifact_dir: &context.artifact_dir,
    })
}

#[cfg(test)]
fn verify_artifact_digest(
    artifact_root: &Path,
    artifact_path: &Path,
    expected_sha256: Option<&str>,
    extra: serde_json::Value,
) -> serde_json::Value {
    artifacts::verify_artifact_digest(artifact_root, artifact_path, expected_sha256, extra)
}

fn normalize_path_lexically(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) => return None,
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            std::path::Component::Normal(segment) => normalized.push(segment),
        }
    }
    Some(normalized)
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

fn render_vcs_status_text(status: &VcsStatus) -> String {
    let mut out = String::new();
    out.push_str(&format!("run_id={}\n", status.run_id));
    out.push_str(&format!("repo={}\n", status.repo));
    out.push_str(&format!("issue={}\n", status.issue));
    out.push_str(&format!("artifact_dir={}\n", status.artifact_dir.display()));
    out.push_str(&format!(
        "context_manifest={}\n",
        status.manifest_path.display()
    ));
    out.push_str(&format!("worktree={}\n", status.worktree.display()));
    out.push_str(&format!("base_commit={}\n", status.base_commit));
    out.push_str(&format!("current_commit={}\n", status.current_commit));
    out.push_str(&format!("branch_name={}\n", status.branch_name));
    out.push_str(&format!(
        "source_checkout_clean_at_prepare={}\n",
        status
            .source_checkout_clean_at_prepare
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    ));
    out.push_str(&format!(
        "touched_file_count={}\n",
        status.touched_files.len()
    ));
    for path in &status.touched_files {
        out.push_str(&format!("touched_file={path}\n"));
    }
    out.push_str("cross_issue_overlap=not_implemented_in_this_milestone\n");
    out.push_str("merge_plan=implemented_read_only\n");
    out
}

fn print_vcs_inventory(entries: &[VcsInventoryEntry]) {
    println!("vcs_worktrees={}", entries.len());
    for entry in entries {
        println!(
            "run_id={} issue={} branch_name={} worktree={} current_commit={} last_run_status={} valid={}",
            entry.run_id,
            entry.issue.as_deref().unwrap_or("unknown"),
            entry.branch_name.as_deref().unwrap_or("unknown"),
            entry
                .worktree
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            entry.current_commit.as_deref().unwrap_or("unknown"),
            entry.last_run_status,
            entry.valid,
        );
        println!("  artifact_dir={}", entry.artifact_dir.display());
        println!("  context_manifest={}", entry.context_manifest.display());
        if let Some(repo) = &entry.repo {
            println!("  repo={repo}");
        }
        if let Some(base_commit) = &entry.base_commit {
            println!("  base_commit={base_commit}");
        }
        if let Some(clean) = entry.source_checkout_clean_at_prepare {
            println!("  source_checkout_clean_at_prepare={clean}");
        }
        if let Some(count) = entry.touched_file_count {
            println!("  touched_file_count={count}");
        }
        if let Some(error) = &entry.error {
            println!("  error={error}");
        }
    }
}

fn print_vcs_inventory_json(entries: &[VcsInventoryEntry]) -> Result<(), String> {
    let payload = serde_json::json!({
        "schema_version": "0.1",
        "worktrees": entries.iter().map(VcsInventoryEntry::to_json).collect::<Vec<_>>(),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).map_err(|e| format!("render VCS inventory: {e}"))?
    );
    Ok(())
}

fn vcs_show_payload(
    config: &AgentactrConfig,
    status: &VcsStatus,
    last_run_status: &str,
) -> Result<serde_json::Value, String> {
    let metadata_path = status.worktree.join(".agentactr-run.toml");
    let metadata = read_worktree_metadata_document(&status.worktree)?
        .map(|value| {
            serde_json::to_value(value).map_err(|e| format!("render worktree metadata: {e}"))
        })
        .transpose()?;
    Ok(serde_json::json!({
        "schema_version": "0.1",
        "run_status": last_run_status,
        "vcs_status": status.to_json(),
        "worktree_metadata_artifact": metadata_path.display().to_string(),
        "worktree_metadata": metadata,
        "vcs_policy": {
            "kind": config.vcs.kind,
            "workspace_strategy": config.vcs.workspace_strategy,
            "base_ref": config.vcs.base_ref,
            "worktree_root": config.vcs.worktree_root,
            "branch_template": config.vcs.branch_template,
            "record_base_commit": config.vcs.record_base_commit,
            "fail_on_dirty_source_checkout": config.vcs.fail_on_dirty_source_checkout,
            "copy_runtime_config_to_worktree": config.vcs.copy_runtime_config_to_worktree,
            "detect_cross_issue_file_overlap": config.vcs.detect_cross_issue_file_overlap,
            "overlap_policy": config.vcs.overlap_policy,
        },
        "milestone_status": {
            "diff": "implemented",
            "commit": "not_implemented_in_this_milestone",
            "cleanup": "not_implemented_in_this_milestone",
            "merge_plan": "implemented_read_only",
        },
    }))
}

fn print_vcs_show(
    config: &AgentactrConfig,
    status: &VcsStatus,
    last_run_status: &str,
) -> Result<(), String> {
    print!("{}", render_vcs_status_text(status));
    println!("run_status={last_run_status}");
    println!(
        "worktree_metadata_artifact={}",
        status.worktree.join(".agentactr-run.toml").display()
    );
    println!("vcs_policy.kind={}", config.vcs.kind);
    println!(
        "vcs_policy.workspace_strategy={}",
        config.vcs.workspace_strategy
    );
    println!("vcs_policy.base_ref={}", config.vcs.base_ref);
    println!("vcs_policy.worktree_root={}", config.vcs.worktree_root);
    println!("vcs_policy.branch_template={}", config.vcs.branch_template);
    println!(
        "vcs_policy.fail_on_dirty_source_checkout={}",
        config.vcs.fail_on_dirty_source_checkout
    );
    println!(
        "vcs_policy.copy_runtime_config_to_worktree={}",
        config.vcs.copy_runtime_config_to_worktree
    );
    println!(
        "vcs_policy.detect_cross_issue_file_overlap={}",
        config.vcs.detect_cross_issue_file_overlap
    );
    println!("vcs_policy.overlap_policy={}", config.vcs.overlap_policy);
    println!("diff=implemented");
    println!("commit=not_implemented_in_this_milestone");
    println!("cleanup=not_implemented_in_this_milestone");
    println!("merge_plan=implemented_read_only");
    Ok(())
}

fn print_vcs_show_json(
    config: &AgentactrConfig,
    status: &VcsStatus,
    last_run_status: &str,
) -> Result<(), String> {
    let payload = vcs_show_payload(config, status, last_run_status)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).map_err(|e| format!("render VCS detail: {e}"))?
    );
    Ok(())
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

fn cmd_repo(args: &mut [String]) -> Result<(), String> {
    if args.get(1).map(String::as_str) != Some("inspect") {
        return Err("usage: agentactr repo inspect".to_string());
    }
    let config = load_agentactr_config(None)?;
    let inspection = configured_repo_inspection(Path::new("."), &config);
    print_repo_inspection(&inspection);
    Ok(())
}

fn cmd_quality(args: &mut [String]) -> Result<(), String> {
    if args.get(1).map(String::as_str) == Some("run") {
        let run_id = args.get(2).ok_or("usage: agentactr quality run RUN_ID")?;
        let config = load_agentactr_config(None)?;
        let mut context = load_run_artifact_context(&config, run_id)?;
        context.worktree = validate_run_worktree_scope(&config, &context)?;
        let inspection = configured_repo_inspection(&context.worktree, &config);
        let ts = current_epoch_millis();
        let report_path = context
            .artifact_dir
            .join(format!("quality_report.rerun.{ts}.txt"));
        let result = run_quality_gates_to_report(
            &inspection,
            &context.worktree,
            &report_path,
            &config.quality.domain_gate_opt_ins,
        );
        let event_type = if result.is_ok() {
            "quality.rerun.completed"
        } else {
            "quality.rerun.failed"
        };
        append_run_event(
            &RunEventContext::root(&config, &context.run_id, &context.repo, &context.issue),
            event_type,
            serde_json::json!({
                "worktree": context.worktree.display().to_string(),
                "report": report_path.display().to_string(),
                "selected_stack": inspection.primary_stack.as_str(),
                "quality_gate_count": inspection.quality_plan.len(),
                "error": result.as_ref().err().cloned(),
            }),
        )?;
        return match result {
            Ok(()) => {
                println!("quality report: {}", report_path.display());
                Ok(())
            }
            Err(err) => Err(err),
        };
    }
    if args.get(1).map(String::as_str) != Some("plan") {
        return Err("usage: agentactr quality plan | quality run RUN_ID".to_string());
    }
    let config = load_agentactr_config(None)?;
    let inspection = configured_repo_inspection(Path::new("."), &config);
    print_repo_inspection(&inspection);
    if inspection.quality_plan.is_empty() {
        println!("quality plan: none; supported stack was not detected");
    } else {
        print_quality_plan(&inspection);
    }
    Ok(())
}

fn cmd_merge(args: &mut [String]) -> Result<(), String> {
    if args.get(1).map(String::as_str) != Some("plan") {
        return Err("usage: agentactr merge plan RUN_ID [--json]".to_string());
    }
    let run_id = args
        .get(2)
        .ok_or("usage: agentactr merge plan RUN_ID [--json]")?;
    if args.len() != 3 && !(args.len() == 4 && args.get(3).map(String::as_str) == Some("--json")) {
        return Err("usage: agentactr merge plan RUN_ID [--json]".to_string());
    }
    let config = load_agentactr_config(None)?;
    let mut context = load_run_artifact_context(&config, run_id)?;
    context.worktree = validate_run_worktree_scope(&config, &context)?;
    let plan = collect_merge_plan(&config, &context)?;
    let artifact_path = record_merge_plan_artifact(&config, &context, &plan)?;
    match args.get(3).map(String::as_str) {
        None => {
            print_merge_plan(&plan, &artifact_path);
            Ok(())
        }
        Some("--json") => print_merge_plan_json(&context, &plan, &artifact_path),
        _ => Err("usage: agentactr merge plan RUN_ID [--json]".to_string()),
    }
}

fn cmd_vcs(args: &mut [String]) -> Result<(), String> {
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
    if let Some(command @ ("commit" | "cleanup")) = args.get(1).map(String::as_str) {
        return not_implemented(&format!("vcs {command}"));
    }
    if args.get(1).map(String::as_str) != Some("prepare") {
        return Err(
            "usage: agentactr vcs prepare --issue 123 [--repo OWNER/REPO] | vcs list [--json] | vcs show RUN_ID [--json] | vcs status RUN_ID | vcs diff RUN_ID [--output PATH]"
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

fn cmd_memory(args: &mut [String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("status") => {
            print_memory_status();
            Ok(())
        }
        Some("pressure") => {
            print_memory_pressure();
            Ok(())
        }
        _ => Err("usage: agentactr memory status|pressure".to_string()),
    }
}

fn not_implemented(command: &str) -> Result<(), String> {
    Err(format!(
        "`agentactr {command}` is specified but not implemented in this milestone"
    ))
}

fn cmd_status() -> Result<(), String> {
    println!("agentactr status: local bootstrap CLI installed");
    Ok(())
}

fn create_dir(path: impl AsRef<Path>) -> Result<(), String> {
    fs::create_dir_all(path.as_ref())
        .map_err(|e| format!("create {}: {e}", path.as_ref().display()))
}

fn write_file(path: impl AsRef<Path>, content: &str) -> Result<(), String> {
    fs::write(path.as_ref(), content).map_err(|e| format!("write {}: {e}", path.as_ref().display()))
}

fn append_gitignore(content: &str) -> Result<(), String> {
    let path = Path::new(".gitignore");
    let existing = fs::read_to_string(path).unwrap_or_default();
    if let Some(updated) = merge_gitignore_additions(&existing, content) {
        fs::write(path, updated).map_err(|e| format!("write .gitignore: {e}"))?;
    }
    Ok(())
}

fn merge_gitignore_additions(existing: &str, generated: &str) -> Option<String> {
    let existing_lines = existing
        .lines()
        .map(str::trim)
        .collect::<std::collections::HashSet<_>>();
    let missing_entries = generated
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| !existing_lines.contains(line))
        .collect::<Vec<_>>();
    if missing_entries.is_empty() {
        return None;
    }

    let mut updated = existing.to_string();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    if !updated.is_empty() {
        updated.push('\n');
    }
    if !existing_lines.contains("# agentactr generated runtime state") {
        updated.push_str("# agentactr generated runtime state\n");
    }
    updated.push_str(&missing_entries.join("\n"));
    updated.push('\n');
    Some(updated)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find_map(|w| (w[0] == flag).then(|| w[1].clone()))
}

fn flag_values(args: &[String], flag: &str) -> Vec<String> {
    args.windows(2)
        .filter(|w| w[0] == flag)
        .map(|w| w[1].clone())
        .collect()
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn validate_run_issue_args(args: &[String]) -> Result<(), String> {
    let mut index = 2;
    while index < args.len() {
        let arg = &args[index];
        if RUN_ISSUE_BOOL_FLAGS.contains(&arg.as_str()) {
            index += 1;
            continue;
        }
        if RUN_ISSUE_VALUE_FLAGS.contains(&arg.as_str()) {
            let Some(value) = args.get(index + 1) else {
                return Err(format!("{arg} requires a value; {RUN_ISSUE_USAGE}"));
            };
            if value.starts_with("--") {
                return Err(format!(
                    "{arg} requires a value, got flag `{value}`; {RUN_ISSUE_USAGE}"
                ));
            }
            index += 2;
            continue;
        }
        if arg.starts_with("--") {
            return Err(format!(
                "unknown agentactr run issue flag `{arg}`; {RUN_ISSUE_USAGE}"
            ));
        }
        return Err(format!(
            "unexpected agentactr run issue argument `{arg}`; {RUN_ISSUE_USAGE}"
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct RunPolicy {
    human_intervention: HumanInterventionSetting,
    codex_approval: CodexApprovalSetting,
    github_finalization: GithubFinalizationSetting,
    human_intervention_source: PolicyValueSource,
    codex_approval_source: PolicyValueSource,
    github_finalization_source: PolicyValueSource,
}

impl RunPolicy {
    fn from_config_and_args(config: &AgentactrConfig, args: &[String]) -> Result<Self, String> {
        let human_intervention = flag_value(args, "--human-intervention")
            .map(|value| HumanInterventionSetting::parse(&value))
            .transpose()?
            .unwrap_or(HumanInterventionSetting::parse(
                &config.human_intervention.mode,
            )?);
        let codex_approval = flag_value(args, "--codex-approval")
            .map(|value| CodexApprovalSetting::parse(&value))
            .transpose()?
            .unwrap_or(CodexApprovalSetting::parse(&config.codex.approval_policy)?);
        let github_finalization = flag_value(args, "--github-finalization")
            .map(|value| GithubFinalizationSetting::parse(&value))
            .transpose()?
            .unwrap_or(GithubFinalizationSetting::parse(
                &config.github.finalization,
            )?);
        let mut policy = Self::new(human_intervention, codex_approval, github_finalization)?;
        policy.human_intervention_source = policy_source(
            args,
            "--human-intervention",
            "AGENTACTR_HUMAN_INTERVENTION",
            "human_intervention.mode",
        );
        policy.codex_approval_source = policy_source(
            args,
            "--codex-approval",
            "AGENTACTR_CODEX_APPROVAL",
            "codex.approval_policy",
        );
        policy.github_finalization_source = policy_source(
            args,
            "--github-finalization",
            "AGENTACTR_GITHUB_FINALIZATION",
            "github.finalization",
        );
        Ok(policy)
    }

    #[cfg(test)]
    fn from_args(args: &[String]) -> Result<Self, String> {
        let human_intervention = flag_value(args, "--human-intervention")
            .map(|value| HumanInterventionSetting::parse(&value))
            .transpose()?
            .unwrap_or(HumanInterventionSetting::FailClosed);
        let codex_approval = flag_value(args, "--codex-approval")
            .map(|value| CodexApprovalSetting::parse(&value))
            .transpose()?
            .unwrap_or(match human_intervention {
                HumanInterventionSetting::Interactive => CodexApprovalSetting::OnRequest,
                _ => CodexApprovalSetting::Never,
            });
        Self::new(
            human_intervention,
            codex_approval,
            GithubFinalizationSetting::AutomaticAfterQualityGates,
        )
    }

    fn new(
        human_intervention: HumanInterventionSetting,
        codex_approval: CodexApprovalSetting,
        github_finalization: GithubFinalizationSetting,
    ) -> Result<Self, String> {
        if matches!(
            human_intervention,
            HumanInterventionSetting::FailClosed | HumanInterventionSetting::ReviewRequired
        ) && codex_approval != CodexApprovalSetting::Never
        {
            return Err(
                "--codex-approval on-request requires --human-intervention interactive; fail-closed and review-required are unattended".to_string(),
            );
        }
        Ok(Self {
            human_intervention,
            codex_approval,
            github_finalization,
            human_intervention_source: PolicyValueSource {
                effective: PolicySourceKind::Default,
                base: PolicySourceKind::Default,
            },
            codex_approval_source: PolicyValueSource {
                effective: PolicySourceKind::Default,
                base: PolicySourceKind::Default,
            },
            github_finalization_source: PolicyValueSource {
                effective: PolicySourceKind::Default,
                base: PolicySourceKind::Default,
            },
        })
    }

    fn runtime_prompting(&self) -> &'static str {
        match self.human_intervention {
            HumanInterventionSetting::Interactive => "enabled",
            HumanInterventionSetting::FailClosed | HumanInterventionSetting::ReviewRequired => {
                "disabled"
            }
        }
    }

    pub(crate) fn codex_approval_cli_value(&self) -> &'static str {
        self.codex_approval.as_cli()
    }

    pub(crate) fn codex_approval_config_value(&self) -> &'static str {
        self.codex_approval.as_config()
    }

    pub(crate) fn human_intervention_config_value(&self) -> &'static str {
        self.human_intervention.as_config()
    }

    fn as_runtime_policy(&self) -> RuntimeApprovalPolicy {
        match self.codex_approval {
            CodexApprovalSetting::Never => RuntimeApprovalPolicy::Never,
            CodexApprovalSetting::OnRequest => RuntimeApprovalPolicy::OnRequest,
        }
    }

    fn github_finalization_text(&self) -> &'static str {
        match (
            self.github_finalization,
            self.human_intervention == HumanInterventionSetting::ReviewRequired,
        ) {
            (_, true) => "require human review before terminal finalization/close",
            (GithubFinalizationSetting::AutomaticAfterQualityGates, false) => {
                "automatic after quality gates"
            }
            (GithubFinalizationSetting::RequireHumanReview, false) => {
                "require human review before terminal finalization/close"
            }
            (GithubFinalizationSetting::Disabled, false) => "disabled",
            (GithubFinalizationSetting::DisabledInThisMilestone, false) => {
                "disabled in this milestone"
            }
        }
    }

    fn github_finalization_effective_text(
        &self,
        tracker_capabilities: &AdapterCapabilities,
    ) -> &'static str {
        if self.github_finalization == GithubFinalizationSetting::Disabled {
            return "disabled";
        }
        if !tracker_supports_github_finalization(tracker_capabilities) {
            return "disabled in this milestone";
        }
        self.github_finalization_text()
    }
}

fn tracker_supports_github_finalization(capabilities: &AdapterCapabilities) -> bool {
    ["claim_mutation", "comment_create", "label_set"]
        .iter()
        .all(|required| {
            capabilities
                .supported_features
                .iter()
                .any(|feature| feature == required)
                && !capabilities
                    .degraded_features
                    .iter()
                    .any(|feature| feature == required)
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PolicySourceKind {
    Default,
    Toml,
    Env,
    Cli,
}

impl PolicySourceKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Toml => "agentactr.toml",
            Self::Env => "environment",
            Self::Cli => "CLI override",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PolicyValueSource {
    effective: PolicySourceKind,
    base: PolicySourceKind,
}

impl PolicyValueSource {
    fn as_text(&self) -> String {
        if self.effective == PolicySourceKind::Cli {
            format!("CLI override over {}", self.base.as_str())
        } else {
            self.effective.as_str().to_string()
        }
    }
}

fn policy_source(
    args: &[String],
    cli_flag: &str,
    env_name: &str,
    config_key: &str,
) -> PolicyValueSource {
    let base = if env::var(env_name).is_ok() {
        PolicySourceKind::Env
    } else if config_key_present(config_key) {
        PolicySourceKind::Toml
    } else {
        PolicySourceKind::Default
    };
    if has_flag(args, cli_flag) {
        return PolicyValueSource {
            effective: PolicySourceKind::Cli,
            base,
        };
    }
    PolicyValueSource {
        effective: base,
        base,
    }
}

fn config_key_present(dotted_key: &str) -> bool {
    let Ok(content) = fs::read_to_string("agentactr.toml") else {
        return false;
    };
    let Ok(parsed) = parse_toml_document(&content) else {
        return false;
    };
    toml_path(&parsed, dotted_key).is_some()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HumanInterventionSetting {
    FailClosed,
    Interactive,
    ReviewRequired,
}

impl HumanInterventionSetting {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "fail-closed" | "fail_closed" => Ok(Self::FailClosed),
            "interactive" => Ok(Self::Interactive),
            "review-required" | "review_required" => Ok(Self::ReviewRequired),
            other => Err(format!("unsupported --human-intervention value `{other}`")),
        }
    }

    fn as_config(&self) -> &'static str {
        match self {
            Self::FailClosed => "fail_closed",
            Self::Interactive => "interactive",
            Self::ReviewRequired => "review_required",
        }
    }

    fn as_cli(&self) -> &'static str {
        match self {
            Self::FailClosed => "fail-closed",
            Self::Interactive => "interactive",
            Self::ReviewRequired => "review-required",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodexApprovalSetting {
    Never,
    OnRequest,
}

impl CodexApprovalSetting {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "never" => Ok(Self::Never),
            "on-request" | "on_request" => Ok(Self::OnRequest),
            other => Err(format!("unsupported --codex-approval value `{other}`")),
        }
    }

    fn as_cli(&self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OnRequest => "on-request",
        }
    }

    fn as_config(&self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OnRequest => "on-request",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GithubFinalizationSetting {
    AutomaticAfterQualityGates,
    RequireHumanReview,
    Disabled,
    DisabledInThisMilestone,
}

impl GithubFinalizationSetting {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "automatic_after_quality_gates" | "automatic-after-quality-gates" | "automatic" => {
                Ok(Self::AutomaticAfterQualityGates)
            }
            "require_human_review" | "require-human-review" | "review_required" => {
                Ok(Self::RequireHumanReview)
            }
            "disabled" => Ok(Self::Disabled),
            "disabled_in_this_milestone" | "disabled-in-this-milestone" => {
                Ok(Self::DisabledInThisMilestone)
            }
            other => Err(format!("unsupported github.finalization value `{other}`")),
        }
    }

    fn as_config(&self) -> &'static str {
        match self {
            Self::AutomaticAfterQualityGates => "automatic_after_quality_gates",
            Self::RequireHumanReview => "require_human_review",
            Self::Disabled => "disabled",
            Self::DisabledInThisMilestone => "disabled_in_this_milestone",
        }
    }
}

fn detect_credentials(config: &AgentactrConfig) -> DetectedCredentials {
    DetectedCredentials {
        github_token: env::var("GITHUB_TOKEN").is_ok(),
        gh_token: env::var("GH_TOKEN").is_ok(),
        configured_github_token: env::var(&config.tracker.token_env).is_ok(),
        google_api_key: env::var("GOOGLE_API_KEY").is_ok(),
        hf_token: env::var("HF_TOKEN").is_ok(),
        openai_api_key: env::var(&config.codex.openai_api_key_env).is_ok(),
        codex_google_mcp: codex_mcp_server_configured("GoogleDeveloperAPI"),
        codex_hf_mcp: codex_mcp_server_configured("hf-mcp-server"),
        codex_github_remote_mcp: codex_mcp_server_configured("github_remote"),
    }
}

fn codex_mcp_server_configured(server: &str) -> bool {
    codex_config_paths()
        .iter()
        .any(|path| codex_config_mcp_server_enabled(path, server))
}

fn codex_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(path) = codex_user_config_path() {
        paths.push(path);
    }
    paths.push(PathBuf::from(".codex").join("config.toml"));
    paths
}

fn codex_user_config_path() -> Result<PathBuf, String> {
    if let Some(codex_home) = env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(codex_home).join("config.toml"));
    }
    let home = env::var_os("HOME").ok_or("HOME is not set; cannot locate Codex user config")?;
    Ok(PathBuf::from(home).join(".codex").join("config.toml"))
}

fn codex_config_mcp_server_enabled(path: &Path, server: &str) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(parsed) = parse_toml_document(&content) else {
        return false;
    };
    let Some(server_config) = toml_path(&parsed, &format!("mcp_servers.{server}")) else {
        return false;
    };
    server_config
        .get("enabled")
        .and_then(toml::Value::as_bool)
        .unwrap_or(true)
}

fn print_mcp_summary(creds: &DetectedCredentials) {
    println!("MCP:");
    println!("  agentactr: enabled, required");
    println!("  openaiDeveloperDocs: enabled, no auth");
    println!(
        "  GoogleDeveloperAPI: {}",
        if creds.google_api_key {
            "enabled, GOOGLE_API_KEY detected"
        } else if creds.codex_google_mcp {
            "enabled, existing Codex MCP config detected"
        } else {
            "disabled, missing GOOGLE_API_KEY"
        }
    );
    println!(
        "  hf-mcp-server: {}",
        if creds.hf_token {
            "enabled, HF_TOKEN detected"
        } else if creds.codex_hf_mcp {
            "enabled, existing Codex MCP config detected"
        } else {
            "disabled, missing HF_TOKEN/OAuth"
        }
    );
    println!(
        "  github_remote: {}",
        if creds.github_any() {
            "enabled read-only, GitHub token detected"
        } else if creds.codex_github_remote_mcp {
            "enabled read-only, existing Codex MCP config detected"
        } else {
            "disabled, missing GITHUB_TOKEN/GH_TOKEN"
        }
    );
    println!("  github_remote write tools: disabled");
}

fn print_security_summary(config: &AgentactrConfig) {
    println!("Security defaults:");
    println!(
        "  human_intervention.mode = {}",
        config.human_intervention.mode
    );
    println!("  codex.approval_policy = {}", config.codex.approval_policy);
    println!("  github.finalization = {}", config.github.finalization);
    println!("  merge.mode = {}", config.merge.mode);
    println!("  merge.push = {}", config.merge.push);
    println!(
        "  remote GitHub write MCP tools = {}",
        config.mcp.remote_github_write_tools
    );
}

fn print_memory_status() {
    println!("Linux memory:");
    let config = load_agentactr_config(None)
        .map(|config| config.linux_memory)
        .unwrap_or_else(|_| AgentactrConfig::strict_defaults("OWNER/REPO").linux_memory);
    for line in LinuxMemoryController::new(&config).status_lines() {
        println!("{line}");
    }
}

fn print_execution_status(config: &AgentactrConfig) {
    println!("Execution backend:");
    match resolve_execution_backend(&config.execution) {
        Ok(decision) => {
            println!("  configured = {}", decision.configured);
            println!("  effective = {}", decision.effective.as_str());
            println!(
                "  strict_memory_required = {}",
                decision.strict_memory_required
            );
            println!("  reason = {}", decision.reason);
            if decision.effective == ExecutionBackend::DockerLinuxVm {
                check_command(&config.execution.docker.command, &["version"]);
                check_docker_info(config);
                check_docker_runtime_tools(config);
            }
        }
        Err(err) => println!("  error: {err}"),
    }
}

fn require_execution_backend_ready(config: &AgentactrConfig) -> Result<(), String> {
    let decision = resolve_execution_backend(&config.execution)?;
    println!("  configured = {}", decision.configured);
    println!("  effective = {}", decision.effective.as_str());
    println!(
        "  strict_memory_required = {}",
        decision.strict_memory_required
    );
    println!("  reason = {}", decision.reason);
    match decision.effective {
        ExecutionBackend::DockerLinuxVm => require_docker_backend_ready(config),
        ExecutionBackend::NativeMacosObserveOnly | ExecutionBackend::ObserveOnly
            if decision.strict_memory_required =>
        {
            Err(format!(
                "execution.backend={} cannot satisfy strict memory enforcement; set execution.backend=\"docker_linux_vm\" on macOS or explicitly set execution.strict_memory_required=false for a degraded local run",
                decision.effective.as_str()
            ))
        }
        ExecutionBackend::NativeLinuxCgroupV2
        | ExecutionBackend::NativeMacosObserveOnly
        | ExecutionBackend::ObserveOnly => Ok(()),
    }
}

fn require_docker_backend_ready(config: &AgentactrConfig) -> Result<(), String> {
    let docker = &config.execution.docker.command;
    require_command(docker, &["version"])?;
    let info = Command::new(docker)
        .arg("info")
        .arg("--format")
        .arg("{{.OSType}}")
        .output()
        .map_err(|e| format!("run `{docker} info`: {e}"))?;
    if !info.status.success() {
        return Err(format!(
            "Docker Linux backend requires a reachable Docker daemon: {}",
            String::from_utf8_lossy(&info.stderr).trim()
        ));
    }
    let os_type = String::from_utf8_lossy(&info.stdout).trim().to_string();
    if os_type != "linux" {
        return Err(format!(
            "Docker backend must report a Linux engine, got {os_type:?}"
        ));
    }
    ensure_docker_image_ready(config)?;
    require_docker_linux_memory_files(config)?;
    require_docker_runtime_tools(config)?;
    if env::var(&config.codex.openai_api_key_env).is_err() && env::var("CODEX_API_KEY").is_err() {
        return Err(format!(
            "Docker execution backend forwards API-key auth only in this milestone; set {} or CODEX_API_KEY before running inside the Linux container",
            config.codex.openai_api_key_env
        ));
    }
    println!("  ok: Docker Linux engine");
    println!("  image = {}", config.execution.docker.image);
    println!("  network = {}", config.execution.docker.network);
    println!(
        "  per-agent memory = high {} max {}",
        config.linux_memory.per_agent_memory_high, config.linux_memory.per_agent_memory_max
    );
    Ok(())
}

fn require_docker_runtime_tools(config: &AgentactrConfig) -> Result<(), String> {
    docker_runtime_tools_probe(config).map(|_| println!("  ok: Docker runtime tools"))
}

fn check_docker_runtime_tools(config: &AgentactrConfig) {
    match docker_runtime_tools_probe(config) {
        Ok(()) => println!("  ok: Docker runtime tools include codex and agentactr"),
        Err(err) => println!("  warning: {err}"),
    }
}

fn docker_runtime_tools_probe(config: &AgentactrConfig) -> Result<(), String> {
    let docker = &config.execution.docker.command;
    let image = &config.execution.docker.image;
    let output = Command::new(docker)
        .arg("run")
        .arg("--rm")
        .arg("--network")
        .arg("none")
        .arg("--entrypoint")
        .arg("sh")
        .arg(image)
        .arg("-lc")
        .arg("command -v codex >/dev/null && codex --version >/dev/null && command -v agentactr >/dev/null && agentactr --help >/dev/null")
        .output()
        .map_err(|e| format!("probe Docker runtime tools with image {image}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(format!(
            "Docker runtime image {image} must contain runnable `codex` and `agentactr` commands for Codex and required local MCP; stdout={stdout:?} stderr={stderr:?}"
        ))
    }
}

fn require_docker_linux_memory_files(config: &AgentactrConfig) -> Result<(), String> {
    let docker = &config.execution.docker.command;
    let image = &config.execution.docker.image;
    let output = Command::new(docker)
        .arg("run")
        .arg("--rm")
        .arg("--network")
        .arg("none")
        .arg("--entrypoint")
        .arg("sh")
        .arg(image)
        .arg("-lc")
        .arg("test -f /sys/fs/cgroup/cgroup.controllers && test -f /proc/pressure/memory")
        .output()
        .map_err(|e| format!("probe Docker Linux memory files with image {image}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Docker Linux backend image {image} does not expose required cgroup v2 and PSI files: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn ensure_docker_image_ready(config: &AgentactrConfig) -> Result<(), String> {
    let docker = &config.execution.docker.command;
    let image = &config.execution.docker.image;
    let pull_policy = config.execution.docker.pull_policy.as_str();
    let inspect = Command::new(docker)
        .arg("image")
        .arg("inspect")
        .arg(image)
        .output()
        .map_err(|e| format!("run `{docker} image inspect {image}`: {e}"))?;
    match docker_image_readiness_action(pull_policy, inspect.status.success())? {
        DockerImageReadinessAction::UseExisting => Ok(()),
        DockerImageReadinessAction::MissingForbidden => Err(format!(
            "Docker runtime image {image} is missing and execution.docker.pull_policy forbids pulling"
        )),
        DockerImageReadinessAction::Pull => {
            println!("  pulling Docker runtime image {image} ({pull_policy})");
            let status = Command::new(docker)
                .arg("pull")
                .arg(image)
                .status()
                .map_err(|e| format!("run `{docker} pull {image}`: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("Docker pull failed for runtime image {image}: {status}"))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DockerImageReadinessAction {
    UseExisting,
    Pull,
    MissingForbidden,
}

fn docker_image_readiness_action(
    pull_policy: &str,
    image_present: bool,
) -> Result<DockerImageReadinessAction, String> {
    match pull_policy {
        "always" => Ok(DockerImageReadinessAction::Pull),
        "if_missing" | "if-not-present" | "if_not_present" if image_present => {
            Ok(DockerImageReadinessAction::UseExisting)
        }
        "if_missing" | "if-not-present" | "if_not_present" => {
            Ok(DockerImageReadinessAction::Pull)
        }
        "never" | "missing_is_error" if image_present => {
            Ok(DockerImageReadinessAction::UseExisting)
        }
        "never" | "missing_is_error" => Ok(DockerImageReadinessAction::MissingForbidden),
        other => Err(format!(
            "unsupported execution.docker.pull_policy={other}; expected if_missing, always, or never"
        )),
    }
}

fn check_docker_info(config: &AgentactrConfig) {
    let docker = &config.execution.docker.command;
    match Command::new(docker)
        .arg("info")
        .arg("--format")
        .arg("{{.OSType}}")
        .output()
    {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if value == "linux" {
                println!("  ok: Docker engine OS linux");
            } else {
                println!("  warning: Docker engine OS is {value}, expected linux");
            }
        }
        Ok(output) => println!(
            "  warning: Docker daemon unavailable: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(err) => println!("  warning: Docker daemon unavailable: {err}"),
    }
}

fn print_memory_pressure() {
    if cfg!(target_os = "linux") {
        match fs::read_to_string("/proc/pressure/memory") {
            Ok(value) => print!("{value}"),
            Err(e) => println!("memory pressure unavailable: {e}"),
        }
    } else {
        println!("memory pressure unavailable: PSI is Linux-specific");
    }
}

fn print_quality_plan(inspection: &RepoInspection) {
    println!("quality plan:");
    for cmd in &inspection.quality_plan {
        println!(
            "  {}: required={} gate={} command={}",
            cmd.name, cmd.required, cmd.non_mutating_final_gate, cmd.command
        );
    }
    if !inspection.domain_quality_plan.is_empty() {
        println!("domain quality plan:");
        for gate in &inspection.domain_quality_plan {
            let command = gate.command.as_deref().unwrap_or("<finding-only>");
            println!(
                "  {}: domain={} tool={} required={} mutates={} network={} credentials={} opt_in={} command={}",
                gate.name,
                gate.domain,
                gate.tool,
                gate.required,
                gate.mutates,
                gate.network_required,
                gate.credential_required,
                gate.opt_in_required,
                command
            );
        }
    }
}

fn print_domain_summary(config: &AgentactrConfig, inspection: &RepoInspection) {
    println!("Domain graph:");
    println!(
        "  schema_version = {}",
        inspection.domain_graph.schema_version
    );
    println!("  domains = {}", inspection.domain_profiles.len());
    println!("  quality_gates = {}", inspection.domain_quality_plan.len());
    println!("  artifact = {}", config.architecture.domain_graph_artifact);
    if inspection.domain_profiles.is_empty() {
        println!("  detected = none");
    } else {
        for profile in &inspection.domain_profiles {
            println!(
                "  detected: {} kind={} confidence={} evidence={}",
                profile.id,
                profile.kind,
                profile.confidence,
                profile.evidence.len()
            );
        }
    }
    let agents_policy = if Path::new("AGENTS.md").exists() {
        "present"
    } else {
        "absent; run agentactr doctor --fix-agents to generate"
    };
    println!("AGENTS.md: {agents_policy}");
}

fn print_adapter_versions(
    vcs: &dyn VersionControl,
    tracker: &dyn IssueTracker,
    runtime: &dyn AgentRuntime,
) -> Vec<AdapterVersionReport> {
    let reports = vec![
        vcs.version_report(),
        tracker.version_report(),
        runtime.version_report(),
    ];
    print_adapter_version_reports(&reports);
    for capabilities in [vcs.capabilities(), tracker.capabilities()] {
        println!(
            "adapter capabilities: kind={} supported={} degraded={} required_actions={}",
            capabilities.adapter_kind,
            capabilities.supported_features.join("|"),
            capabilities.degraded_features.join("|"),
            capabilities.required_actions.join("|")
        );
    }
    let runtime_capabilities = runtime.capabilities();
    println!(
        "adapter capabilities: kind=agent_runtime single_shot_issue_run={} session_start={} turn_streaming={} cancellation={} exec_json={} app_server={} codex_sdk={} child_agent_execution={} parallel_read_only_child_agents={}",
        runtime_capabilities.single_shot_issue_run,
        runtime_capabilities.session_start,
        runtime_capabilities.turn_streaming,
        runtime_capabilities.cancellation,
        runtime_capabilities.exec_json,
        runtime_capabilities.app_server,
        runtime_capabilities.codex_sdk,
        runtime_capabilities.child_agent_execution,
        runtime_capabilities.parallel_read_only_child_agents
    );
    reports
}

fn print_doctor_adapter_versions(config: &AgentactrConfig) -> Result<(), String> {
    let reports = configured_adapter_version_reports(config)?;
    print_adapter_version_reports(&reports);
    Ok(())
}

fn configured_adapter_version_reports(
    config: &AgentactrConfig,
) -> Result<Vec<AdapterVersionReport>, String> {
    let vcs = LocalGitAdapter;
    let tracker = GithubRestAdapter::new(
        PathBuf::from(&config.observability.artifact_root).join("doctor"),
        &config.tracker,
    );
    let runtime = CodexRuntimeAdapter::new(&config.codex)?;
    Ok(vec![
        vcs.version_report(),
        tracker.version_report(),
        runtime.version_report(),
    ])
}

fn print_adapter_version_reports(reports: &[AdapterVersionReport]) {
    for report in reports {
        println!(
            "adapter: kind={} name={} product={} api={}",
            report.adapter_kind, report.adapter_name, report.product_name, report.api_version
        );
        println!(
            "adapter report: kind={} degraded={} required_actions={} warnings={}",
            report.adapter_kind,
            report.degraded_features.join("|"),
            report.required_actions.join("|"),
            report.warnings.join("|")
        );
    }
}

fn record_adapter_version_reports(
    config: &AgentactrConfig,
    run_id: &str,
    repo: &str,
    issue: &str,
    artifact_dir: &Path,
    reports: &[AdapterVersionReport],
) -> Result<PathBuf, String> {
    let path = artifact_dir.join("adapter_version_reports.json");
    let payload = adapter_version_reports_payload(reports);
    fs::write(
        &path,
        serde_json::to_string_pretty(&payload)
            .map_err(|e| format!("render adapter version reports: {e}"))?,
    )
    .map_err(|e| format!("write adapter version reports {}: {e}", path.display()))?;
    append_run_event(
        &RunEventContext::root(config, run_id, repo, issue),
        "adapter.version_reported",
        serde_json::json!({
            "artifact": path.display().to_string(),
            "reports": payload["reports"].clone(),
        }),
    )?;
    Ok(path)
}

fn adapter_version_reports_payload(reports: &[AdapterVersionReport]) -> serde_json::Value {
    serde_json::json!({
        "schema_version": "0.1",
        "reports": reports.iter().map(adapter_version_report_payload).collect::<Vec<_>>(),
    })
}

fn adapter_version_report_payload(report: &AdapterVersionReport) -> serde_json::Value {
    serde_json::json!({
        "adapter_kind": &report.adapter_kind,
        "adapter_name": &report.adapter_name,
        "adapter_version": &report.adapter_version,
        "product_name": &report.product_name,
        "product_version": &report.product_version,
        "api_version": &report.api_version,
        "capability_digest": &report.capability_digest,
        "degraded_features": &report.degraded_features,
        "required_actions": &report.required_actions,
        "warnings": &report.warnings,
    })
}

fn print_repo_inspection(inspection: &RepoInspection) {
    println!("Repository:");
    println!("  root = {}", inspection.root.display());
    println!("  git = {}", inspection.is_git);
    println!("  empty = {}", inspection.is_empty);
    println!("  detected_stack = {}", inspection.detected_stack.as_str());
    println!("  selected_stack = {}", inspection.primary_stack.as_str());
    println!(
        "  selected_quality_profile = {}",
        inspection.selected_quality_profile
    );
    println!("  confidence = {}", inspection.confidence);
    if inspection.evidence_files.is_empty() {
        println!("  evidence = []");
    } else {
        println!("  evidence:");
        for file in &inspection.evidence_files {
            println!("    - {file}");
        }
    }
    if !inspection.missing_prerequisites.is_empty() {
        println!("  missing_prerequisites:");
        for item in &inspection.missing_prerequisites {
            println!("    - {item}");
        }
    }
    if !inspection.setup_guidance.is_empty() {
        println!("  setup_guidance:");
        for item in &inspection.setup_guidance {
            println!("    - {item}");
        }
    }
    if !inspection.domain_profiles.is_empty() {
        println!("  domain_profiles:");
        for profile in &inspection.domain_profiles {
            println!(
                "    - {} ({}, confidence={}, evidence={})",
                profile.id,
                profile.kind,
                profile.confidence,
                profile.evidence.len()
            );
        }
    }
    if !inspection.domain_quality_plan.is_empty() {
        println!(
            "  domain_quality_gates = {}",
            inspection.domain_quality_plan.len()
        );
    }
}

#[derive(Clone, Debug)]
struct EffectiveRepositoryContext {
    inspection: RepoInspection,
    stack_source: RepositoryStackSource,
    fail_on_low_confidence_stack_detection: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RepositoryStackSource {
    Config,
    TrackerField(String),
    TrackerLabel(String),
    Detection,
    Unresolved,
}

impl RepositoryStackSource {
    fn as_display(&self) -> String {
        match self {
            Self::Config => "agentactr.toml:repository.declared_primary_stack".to_string(),
            Self::TrackerField(field) => format!("tracker_field:{field}"),
            Self::TrackerLabel(label) => format!("tracker_label:{label}"),
            Self::Detection => "repository_detection".to_string(),
            Self::Unresolved => "unresolved".to_string(),
        }
    }

    fn is_declared(&self) -> bool {
        matches!(
            self,
            Self::Config | Self::TrackerField(_) | Self::TrackerLabel(_)
        )
    }
}

fn effective_repository_context(
    inspection: RepoInspection,
    config: &AgentactrConfig,
    repo: &str,
    issue: &str,
) -> Result<EffectiveRepositoryContext, String> {
    if let Some(declared_stack) = declared_primary_stack(config) {
        return Ok(EffectiveRepositoryContext {
            inspection: apply_declared_stack_to_inspection_with_config(
                inspection,
                &declared_stack,
                config,
            ),
            stack_source: RepositoryStackSource::Config,
            fail_on_low_confidence_stack_detection: config
                .repository
                .fail_on_low_confidence_stack_detection,
        });
    }
    if !needs_tracker_stack_enrichment(&inspection, config) {
        return Ok(EffectiveRepositoryContext {
            stack_source: if inspection.primary_stack == StackKind::Unknown {
                RepositoryStackSource::Unresolved
            } else {
                RepositoryStackSource::Detection
            },
            inspection,
            fail_on_low_confidence_stack_detection: config
                .repository
                .fail_on_low_confidence_stack_detection,
        });
    }
    let metadata = fetch_tracker_stack_metadata(config, repo, issue)?;
    match stack_from_tracker_metadata(&metadata)? {
        Some((stack, source)) => Ok(EffectiveRepositoryContext {
            inspection: apply_declared_stack_to_inspection_with_config(inspection, &stack, config),
            stack_source: source,
            fail_on_low_confidence_stack_detection: config
                .repository
                .fail_on_low_confidence_stack_detection,
        }),
        None => Ok(EffectiveRepositoryContext {
            inspection,
            stack_source: RepositoryStackSource::Unresolved,
            fail_on_low_confidence_stack_detection: config
                .repository
                .fail_on_low_confidence_stack_detection,
        }),
    }
}

fn needs_tracker_stack_enrichment(inspection: &RepoInspection, config: &AgentactrConfig) -> bool {
    declared_primary_stack(config).is_none()
        && (inspection.is_empty
            || inspection.primary_stack == StackKind::Mixed
            || inspection.confidence < 70)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TrackerStackMetadata {
    labels: Vec<String>,
    body: String,
}

fn fetch_tracker_stack_metadata(
    config: &AgentactrConfig,
    repo: &str,
    issue: &str,
) -> Result<TrackerStackMetadata, String> {
    let artifact_dir = PathBuf::from(&config.observability.artifact_root)
        .join("preflight")
        .join("stack-context");
    create_dir(&artifact_dir)?;
    let tracker = GithubRestAdapter::new(artifact_dir, &config.tracker);
    let issue_id = IssueId(format!("{repo}#{issue}"));
    let issues = tracker.fetch_by_ids(&[issue_id]).map_err(|err| {
        format!(
            "tracker stack-label enrichment failed before repository fail-closed checks: {err}; set repository.declared_primary_stack to proceed without tracker metadata"
        )
    })?;
    Ok(issues
        .into_iter()
        .next()
        .map(|issue| TrackerStackMetadata {
            labels: issue.labels,
            body: issue.body,
        })
        .unwrap_or_default())
}

#[cfg(test)]
fn stack_from_tracker_labels(labels: &[String]) -> Result<Option<(StackKind, String)>, String> {
    stack_from_tracker_metadata(&TrackerStackMetadata {
        labels: labels.to_vec(),
        body: String::new(),
    })
    .map(|found| {
        found.map(|(stack, source)| match source {
            RepositoryStackSource::TrackerLabel(label) => (stack, label),
            RepositoryStackSource::TrackerField(field) => (stack, field),
            RepositoryStackSource::Config
            | RepositoryStackSource::Detection
            | RepositoryStackSource::Unresolved => (stack, source.as_display()),
        })
    })
}

fn stack_from_tracker_metadata(
    metadata: &TrackerStackMetadata,
) -> Result<Option<(StackKind, RepositoryStackSource)>, String> {
    let mut matches = Vec::new();
    for label in &metadata.labels {
        let normalized = label.trim().to_ascii_lowercase();
        let stack = match normalized.as_str() {
            "stack:rust" => Some(StackKind::Rust),
            "stack:typescript" => Some(StackKind::TypeScript),
            "stack:golang" => Some(StackKind::Golang),
            "stack:python" => Some(StackKind::Python),
            _ => None,
        };
        if let Some(stack) = stack {
            matches.push((stack, RepositoryStackSource::TrackerLabel(label.clone())));
        }
    }
    for (field, value) in tracker_stack_fields(&metadata.body) {
        let stack = match value.as_str() {
            "rust" => Some(StackKind::Rust),
            "typescript" => Some(StackKind::TypeScript),
            "golang" => Some(StackKind::Golang),
            "python" => Some(StackKind::Python),
            _ => None,
        };
        if let Some(stack) = stack {
            matches.push((stack, RepositoryStackSource::TrackerField(field)));
        }
    }
    matches.sort_by_key(|left| left.1.as_display());
    matches.dedup_by(|left, right| left.0 == right.0);
    match matches.as_slice() {
        [] => Ok(None),
        [(stack, source)] => Ok(Some((stack.clone(), source.clone()))),
        _ => Err(format!(
            "conflicting tracker stack declarations found: {}; keep exactly one of stack:rust, stack:typescript, stack:golang, stack:python or one structured stack field",
            matches
                .iter()
                .map(|(_, source)| source.as_display())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn tracker_stack_fields(body: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    for line in body.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches("<!--")
            .trim_end_matches("-->")
            .trim();
        for separator in [":", "="] {
            let Some((key, value)) = trimmed.split_once(separator) else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase().replace('-', "_");
            if matches!(
                key.as_str(),
                "agentactr_stack" | "repository_stack" | "stack"
            ) {
                fields.push((key, value.trim().to_ascii_lowercase()));
            }
        }
    }
    fields
}

fn effective_repo_inspection(
    inspection: RepoInspection,
    config: &AgentactrConfig,
) -> RepoInspection {
    let Some(declared_stack) = declared_primary_stack(config) else {
        return inspection;
    };
    apply_declared_stack_to_inspection_with_config(inspection, &declared_stack, config)
}

fn configured_repo_inspection(root: &Path, config: &AgentactrConfig) -> RepoInspection {
    effective_repo_inspection(discover_repository_with_config(root, config), config)
}

fn declared_primary_stack(config: &AgentactrConfig) -> Option<StackKind> {
    match config.repository.declared_primary_stack.as_str() {
        "typescript" | "ts" | "node" => Some(StackKind::TypeScript),
        "rust" => Some(StackKind::Rust),
        "golang" | "go" => Some(StackKind::Golang),
        "python" | "py" => Some(StackKind::Python),
        "mixed" => Some(StackKind::Mixed),
        "auto" | "unknown" | "" => None,
        _ => None,
    }
}

fn fail_on_blocking_repo_findings(context: &EffectiveRepositoryContext) -> Result<(), String> {
    let inspection = &context.inspection;
    if inspection.is_empty && !context.stack_source.is_declared() {
        return Err(
            "empty repository bootstrap is fail-closed; declare repository.declared_primary_stack or add a supported tracker stack label first"
                .to_string(),
        );
    }
    if !inspection.is_git {
        return Err("default CLI requires a Git repository for worktree isolation".to_string());
    }
    if context.fail_on_low_confidence_stack_detection
        && !context.stack_source.is_declared()
        && (inspection.primary_stack == StackKind::Mixed || inspection.confidence < 70)
    {
        return Err(format!(
            "repository stack detection is ambiguous or low-confidence (detected_stack={} confidence={}); declare repository.declared_primary_stack or add one supported tracker stack label before unattended runs",
            inspection.primary_stack.as_str(),
            inspection.confidence
        ));
    }
    if !inspection.missing_prerequisites.is_empty() {
        return Err(format!(
            "strict repository prerequisites are missing; run `agentactr repo inspect` for exact guidance ({})",
            inspection.missing_prerequisites.join("; ")
        ));
    }
    Ok(())
}

fn check_path(path: &str) {
    if Path::new(path).exists() {
        println!("ok: {path}");
    } else {
        println!("missing: {path}");
    }
}

fn check_env(name: &str, label: &str) {
    if env::var(name).is_ok() {
        println!("ok: {label} ({name})");
    } else {
        println!("missing: {label} ({name})");
    }
}

fn require_env_any(names: &[&str], label: &str) -> Result<String, String> {
    for name in names {
        if env::var(name).is_ok() {
            println!("ok: {label} ({name})");
            return Ok((*name).to_string());
        }
    }
    Err(format!("missing: {label} ({})", names.join(" or ")))
}

fn preferred_github_token_env_names(configured: &str) -> Vec<String> {
    let mut names = Vec::new();
    for name in [configured, "GITHUB_TOKEN", "GH_TOKEN"] {
        if !name.is_empty() && !names.iter().any(|existing| existing == name) {
            names.push(name.to_string());
        }
    }
    names
}

fn check_optional_env(name: &str, label: &str) {
    if env::var(name).is_ok() {
        println!("ok: {label} ({name})");
    } else {
        println!("not set: {label} ({name})");
    }
}

fn check_github_token_governance(config: &AgentactrConfig) {
    println!("GitHub token governance:");
    let configured = &config.tracker.token_env;
    let configured_set = env::var(configured).is_ok();
    let generic_set = env::var("GITHUB_TOKEN").is_ok() || env::var("GH_TOKEN").is_ok();
    if configured_set && configured != "GITHUB_TOKEN" && configured != "GH_TOKEN" {
        println!("  ok: configured token env `{configured}` is available and preferred");
    } else if generic_set {
        println!(
            "  warning: using generic PAT-style token env; GitHub App installation auth is preferred for production automation"
        );
    } else {
        println!("  missing: no configured GitHub token detected");
    }
}

struct GithubApiVersionSupport {
    version: &'static str,
    end_of_support: Option<&'static str>,
}

const SUPPORTED_GITHUB_API_VERSIONS: &[GithubApiVersionSupport] = &[
    GithubApiVersionSupport {
        version: "2026-03-10",
        end_of_support: None,
    },
    GithubApiVersionSupport {
        version: "2022-11-28",
        end_of_support: Some("March 10, 2028"),
    },
];

fn github_api_version_support(version: &str) -> Option<&'static GithubApiVersionSupport> {
    SUPPORTED_GITHUB_API_VERSIONS
        .iter()
        .find(|support| support.version == version)
}

fn check_github_api_version(config: &AgentactrConfig) {
    let configured = config.tracker.github_api_version.as_str();
    if let Some(support) = github_api_version_support(configured) {
        if let Some(end_of_support) = support.end_of_support {
            println!("ok: GitHub REST API version {configured} (supported until {end_of_support})");
        } else {
            println!("ok: GitHub REST API version {configured} (support end not yet scheduled)");
        }
    } else {
        let supported = SUPPORTED_GITHUB_API_VERSIONS
            .iter()
            .map(|support| support.version)
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "warning: GitHub REST API version {configured} is not in this SDK's supported set ({})",
            supported
        );
    }
}

fn check_github_lifecycle_labels(config: &AgentactrConfig) {
    println!("GitHub lifecycle labels:");
    if env::var(&config.tracker.token_env).is_err()
        && env::var("GITHUB_TOKEN").is_err()
        && env::var("GH_TOKEN").is_err()
    {
        println!("  skipped: GitHub token unavailable");
        return;
    }
    let artifact_dir = PathBuf::from(&config.observability.artifact_root).join("doctor");
    if let Err(err) = fs::create_dir_all(&artifact_dir) {
        println!(
            "  skipped: doctor artifact directory unavailable ({}): {err}",
            artifact_dir.display()
        );
        return;
    }
    let tracker = GithubRestAdapter::new(artifact_dir, &config.tracker);
    for label in [
        &config.tracker.claim_label,
        &config.tracker.running_label,
        &config.tracker.failed_label,
        &config.tracker.done_label,
    ] {
        match tracker.check_label_exists(&config.tracker.repo, label) {
            Ok(()) => println!("  ok: label `{label}` exists"),
            Err(err) => println!("  missing: label `{label}` unavailable: {err}"),
        }
    }
}

fn check_sqlite_store(config: &AgentactrConfig) {
    match validate_sqlite_store(config) {
        Ok(path) => println!("ok: SQLite run store openable ({})", path.display()),
        Err(err) => println!("missing: SQLite run store unavailable: {err}"),
    }
}

fn validate_sqlite_store(config: &AgentactrConfig) -> Result<PathBuf, String> {
    use sqlx_sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let sqlite_path = resolve_config_path(&config.observability.sqlite)?;
    let parent = sqlite_path
        .parent()
        .ok_or_else(|| format!("SQLite path has no parent: {}", sqlite_path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("create SQLite parent {}: {e}", parent.display()))?;
    let probe_path = parent.join(format!(
        ".agentactr-doctor-{}-{}.sqlite",
        std::process::id(),
        current_epoch_millis()
    ));
    let url = format!("sqlite://{}", probe_path.display());
    let options = url
        .parse::<SqliteConnectOptions>()
        .map_err(|e| format!("configure SQLite probe {url}: {e}"))?
        .create_if_missing(true);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("start SQLite probe runtime: {e}"))?;
    runtime.block_on(async {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|e| format!("open SQLite probe {}: {e}", probe_path.display()))?;
        sqlx_core::query::query("SELECT 1")
            .execute(&pool)
            .await
            .map_err(|e| format!("query SQLite probe {}: {e}", probe_path.display()))?;
        pool.close().await;
        Ok::<_, String>(())
    })?;
    let _ = fs::remove_file(&probe_path);
    let _ = fs::remove_file(probe_path.with_extension("sqlite-shm"));
    let _ = fs::remove_file(probe_path.with_extension("sqlite-wal"));
    Ok(sqlite_path)
}

fn check_otlp(config: &AgentactrConfig) {
    if !config.observability.otel_enabled {
        println!("ok: OTLP disabled by config");
        return;
    }
    match validate_otlp_endpoint(&config.observability.otel_endpoint) {
        Ok(()) => println!(
            "ok: OTLP endpoint reachable ({})",
            config.observability.otel_endpoint
        ),
        Err(err) => println!("warning: OTLP endpoint unavailable: {err}"),
    }
}

fn validate_otlp_endpoint(endpoint: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(endpoint).map_err(|e| format!("parse `{endpoint}`: {e}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| format!("OTLP endpoint `{endpoint}` has no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| format!("OTLP endpoint `{endpoint}` has no port"))?;
    let mut addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?;
    let addr = addrs
        .next()
        .ok_or_else(|| format!("resolve {host}:{port}: no addresses"))?;
    TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|e| format!("connect {host}:{port}: {e}"))?;
    Ok(())
}

fn check_workspace_permissions(config: &AgentactrConfig) {
    println!("Workspace permissions:");
    for (label, value) in [
        ("cwd", "."),
        ("workspace.root", config.workspace.root.as_str()),
        ("vcs.worktree_root", config.vcs.worktree_root.as_str()),
        (
            "observability.artifact_root",
            config.observability.artifact_root.as_str(),
        ),
        (
            "observability.debug_bundle_root",
            config.observability.debug_bundle_root.as_str(),
        ),
    ] {
        match validate_writable_path(value) {
            Ok(path) => println!("  ok: {label} writable ({})", path.display()),
            Err(err) => println!("  missing: {label} not writable: {err}"),
        }
    }
}

fn validate_writable_path(value: &str) -> Result<PathBuf, String> {
    let path = resolve_config_path(value)?;
    let probe_dir = if path.exists() {
        if path.is_dir() {
            path.clone()
        } else {
            path.parent()
                .ok_or_else(|| format!("{} has no parent", path.display()))?
                .to_path_buf()
        }
    } else {
        path.parent()
            .ok_or_else(|| format!("{} has no parent", path.display()))?
            .to_path_buf()
    };
    fs::create_dir_all(&probe_dir).map_err(|e| format!("create {}: {e}", probe_dir.display()))?;
    let probe = probe_dir.join(format!(
        ".agentactr-doctor-write-{}-{}",
        std::process::id(),
        current_epoch_millis()
    ));
    fs::write(&probe, b"ok").map_err(|e| format!("write {}: {e}", probe.display()))?;
    fs::remove_file(&probe).map_err(|e| format!("remove {}: {e}", probe.display()))?;
    Ok(path)
}

fn check_command(program: &str, args: &[&str]) {
    let status = Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => println!("ok: command `{program}`"),
        Ok(s) => println!("warning: command `{program}` exited with {s}"),
        Err(_) => println!("missing: command `{program}`"),
    }
}

fn require_command(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("ok: command `{program}`");
            Ok(())
        }
        Ok(s) => Err(format!("command `{program}` exited with {s}")),
        Err(e) => Err(format!("missing command `{program}`: {e}")),
    }
}

fn check_codex_login_status(command: &str, api_key_env: &str) {
    if env::var(api_key_env).is_ok() {
        println!("ok: Codex exec API-key auth ({api_key_env})");
        return;
    }
    let status = Command::new(command)
        .arg("login")
        .arg("status")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => println!("ok: {command} login status"),
        Ok(_) => println!("missing: Codex auth; run `codex login` for subscription auth or set `{api_key_env}` for codex exec automation"),
        Err(_) => println!("missing: {command} login status unavailable"),
    }
}

fn require_codex_exec_auth(command: &str, api_key_env: &str) -> Result<(), String> {
    if codex_api_key_env_value(api_key_env).is_some() {
        println!("ok: Codex exec API-key auth ({api_key_env})");
        return Ok(());
    }
    let status = Command::new(command)
        .arg("login")
        .arg("status")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("ok: {command} login status");
            Ok(())
        }
        Ok(_) => Err(format!("missing Codex auth; run `codex login` for subscription auth or set `{api_key_env}` for codex exec automation")),
        Err(e) => Err(format!("{command} login status unavailable: {e}")),
    }
}

fn codex_api_key_env_value(api_key_env: &str) -> Option<String> {
    env::var(api_key_env).ok().or_else(|| {
        (api_key_env != "CODEX_API_KEY")
            .then(|| env::var("CODEX_API_KEY").ok())
            .flatten()
    })
}

fn forward_codex_api_key_env(command: &mut Command, api_key_env: &str) {
    if let Some(value) = codex_api_key_env_value(api_key_env) {
        command.env("CODEX_API_KEY", value);
    }
}

fn should_probe_host_codex(decision: &ExecutionBackendDecision) -> bool {
    decision.effective != ExecutionBackend::DockerLinuxVm
}

fn require_codex_availability(
    config: &AgentactrConfig,
    decision: &ExecutionBackendDecision,
) -> Result<CodexMode, String> {
    let mode = require_supported_codex_transport(config)?;
    if should_probe_host_codex(decision) {
        require_command(&config.codex.command, &["--version"])?;
        require_codex_exec_auth(&config.codex.command, &config.codex.openai_api_key_env)?;
        if mode == CodexMode::CliJsonExec {
            require_codex_exec_capacity(config)?;
        }
    } else {
        println!(
            "ok: host Codex probes skipped; {} runs Codex inside Docker image {}",
            decision.effective.as_str(),
            config.execution.docker.image
        );
    }
    Ok(mode)
}

fn require_mcp_policy_ready(decision: &ExecutionBackendDecision) -> Result<(), String> {
    if should_probe_host_codex(decision) {
        require_agentactr_mcp_ready(Duration::from_secs(10))
    } else {
        println!(
            "ok: host agentactr MCP self-check skipped; Docker runtime image preflight verifies agentactr"
        );
        Ok(())
    }
}

fn require_supported_codex_transport(config: &AgentactrConfig) -> Result<CodexMode, String> {
    let mode = CodexMode::parse(&config.codex.mode)?;
    println!("preflight: Codex transport");
    match mode {
        CodexMode::CliJsonExec => {
            println!("ok: codex.mode=cli_json transport=codex exec --json");
            Ok(mode)
        }
        CodexMode::AppServer => {
            println!("missing: codex.mode=app_server transport=Codex app-server");
            println!("  status = feature-gated, adapter stub fails closed");
            println!(
                "  configured_transport = {}",
                config.codex.app_server_transport
            );
            println!(
                "  experimental_api = {}",
                config.codex.app_server_experimental_api
            );
            println!("  fallback_mode = {}", config.codex.fallback_mode);
            println!("  fallback = agentactr config set codex.mode cli_json");
            Err(CodexAppServerAdapter::unsupported_message())
        }
        CodexMode::CodexSdk => {
            println!("missing: codex.mode=codex_sdk transport=Codex SDK");
            println!("  status = feature-gated, TypeScript @openai/codex-sdk bridge pending");
            println!("  configured_bridge = {}", config.codex.sdk_bridge);
            println!("  fallback_mode = {}", config.codex.fallback_mode);
            println!("  requirement = Node.js 18+ and SDK sidecar contract tests");
            println!("  fallback = agentactr config set codex.mode cli_json");
            Err(CodexSdkAdapter::unsupported_message())
        }
    }
}

fn check_codex_transport(config: &AgentactrConfig) -> Option<CodexMode> {
    println!("preflight: Codex transport");
    match CodexMode::parse(&config.codex.mode) {
        Ok(CodexMode::CliJsonExec) => {
            println!("ok: codex.mode=cli_json transport=codex exec --json");
            Some(CodexMode::CliJsonExec)
        }
        Ok(CodexMode::AppServer) => {
            println!("missing: codex.mode=app_server transport=Codex app-server");
            println!("  status = feature-gated, adapter stub fails closed");
            println!(
                "  configured_transport = {}",
                config.codex.app_server_transport
            );
            println!(
                "  experimental_api = {}",
                config.codex.app_server_experimental_api
            );
            println!("  fallback_mode = {}", config.codex.fallback_mode);
            println!("  fallback = agentactr config set codex.mode cli_json");
            None
        }
        Ok(CodexMode::CodexSdk) => {
            println!("missing: codex.mode=codex_sdk transport=Codex SDK");
            println!("  status = feature-gated, TypeScript @openai/codex-sdk bridge pending");
            println!("  configured_bridge = {}", config.codex.sdk_bridge);
            println!("  fallback_mode = {}", config.codex.fallback_mode);
            println!("  requirement = Node.js 18+ and SDK sidecar contract tests");
            println!("  fallback = agentactr config set codex.mode cli_json");
            None
        }
        Err(err) => {
            println!("missing: Codex transport config invalid: {err}");
            None
        }
    }
}

fn check_codex_exec_capacity(config: &AgentactrConfig) {
    match codex_exec_capacity_probe(config, Duration::from_secs(60)) {
        Ok(()) => println!("ok: Codex exec capacity probe"),
        Err(err) => println!("missing: Codex exec capacity probe failed: {err}"),
    }
}

fn check_codex_project_trust(worktree: &Path) {
    match codex_project_trusted(worktree) {
        Ok(true) => println!("ok: Codex project trust"),
        Ok(false) => println!(
            "missing: Codex project trust; run `agentactr doctor --trust-codex-project` from this repo root if you explicitly allow updating Codex user config"
        ),
        Err(err) => println!("missing: Codex project trust check failed: {err}"),
    }
}

fn require_codex_exec_capacity(config: &AgentactrConfig) -> Result<(), String> {
    codex_exec_capacity_probe(config, Duration::from_secs(60))
        .map(|_| println!("ok: Codex exec capacity probe"))
}

fn codex_exec_capacity_probe(config: &AgentactrConfig, timeout: Duration) -> Result<(), String> {
    let mut command = Command::new(&config.codex.command);
    command.arg("exec").arg("--json");
    append_codex_project_profile_overrides(&mut command, Path::new("."), &config.codex.profile)?;
    command
        .arg("--sandbox")
        .arg("read-only")
        .arg("-c")
        .arg("approval_policy=\"never\"")
        .arg("--cd")
        .arg(".")
        .arg("Reply exactly: agentactr-preflight-ok")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    forward_codex_api_key_env(&mut command, &config.codex.openai_api_key_env);

    let output = run_command_capture_timeout(command, timeout)
        .map_err(|e| format!("unable to run codex exec probe: {e}"))?;
    let combined = format!("{}\n{}", output.stdout, output.stderr);
    if output.status.success() {
        if codex_probe_output_has_error_event(&output.stdout) {
            return Err(classify_codex_exec_failure(&combined));
        }
        return Ok(());
    }

    Err(classify_codex_exec_failure(&combined))
}

#[derive(Debug)]
struct CapturedCommandOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_command_capture_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<CapturedCommandOutput, String> {
    let mut child = command.spawn().map_err(|e| format!("start command: {e}"))?;
    let stdout = child.stdout.take().ok_or("stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("stderr unavailable")?;
    let stdout_thread = thread::spawn(move || read_to_string(stdout));
    let stderr_thread = thread::spawn(move || read_to_string(stderr));
    let status = wait_child_timeout(&mut child, timeout)?;
    let stdout = stdout_thread
        .join()
        .map_err(|_| "stdout reader thread panicked".to_string())??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| "stderr reader thread panicked".to_string())??;
    Ok(CapturedCommandOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_to_string(mut reader: impl Read) -> Result<String, String> {
    let mut output = String::new();
    reader
        .read_to_string(&mut output)
        .map_err(|e| format!("read command output: {e}"))?;
    Ok(output)
}

fn codex_probe_output_has_error_event(stdout: &str) -> bool {
    stdout.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| {
                value
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        value
                            .get("event")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
            })
            .is_some_and(|event| event == "error" || event == "turn.failed")
    })
}

fn classify_codex_exec_failure(output: &str) -> String {
    let normalized = output.to_ascii_lowercase();
    if normalized.contains("config profile") && normalized.contains("not found") {
        return "Codex config profile not found; agentactr launches Codex with repo-local project defaults and explicit -c overrides, so check for a custom wrapper or stale agentactr binary still passing --profile".to_string();
    }
    if contains_any(
        &normalized,
        &[
            "usage limit",
            "rate limit",
            "rate_limit",
            "too many requests",
            "quota",
            "insufficient_quota",
            "billing",
            "credit",
            "credits",
            "balance",
            "429",
        ],
    ) {
        let reported = codex_usage_reported_message(output)
            .map(|message| format!("; Codex/OpenAI reported: {message}"))
            .or_else(|| {
                codex_usage_retry_hint(output)
                    .map(|hint| format!("; Codex/OpenAI reported retry hint: {hint}"))
            })
            .unwrap_or_default();
        return format!(
            "Codex/OpenAI quota or usage capacity unavailable; subscription may be at usage limit or API-key billing credits/quota may be exhausted{reported}"
        );
    }
    if contains_any(
        &normalized,
        &[
            "not logged in",
            "login",
            "unauthorized",
            "unauthenticated",
            "authentication",
            "invalid api key",
            "incorrect api key",
            "401",
            "403",
        ],
    ) {
        return "Codex/OpenAI authentication failed; run `codex login` for subscription auth or set a valid CODEX_API_KEY".to_string();
    }
    if output.trim().is_empty() {
        return "codex exec exited without diagnostic output".to_string();
    }
    format!("codex exec probe failed: {}", compact_diagnostic(output))
}

fn codex_usage_reported_message(output: &str) -> Option<String> {
    output
        .lines()
        .filter_map(codex_usage_message_from_line)
        .next()
}

fn codex_usage_message_from_line(line: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        return codex_usage_message_from_json(&value);
    }
    let trimmed = line.trim();
    contains_any(
        &trimmed.to_ascii_lowercase(),
        &[
            "usage limit",
            "rate limit",
            "too many requests",
            "quota",
            "insufficient_quota",
            "try again",
            "retry after",
        ],
    )
    .then(|| compact_diagnostic_line(trimmed))
}

fn codex_usage_message_from_json(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for key in ["message", "detail", "error", "reason"] {
                if let Some(message) = map.get(key).and_then(serde_json::Value::as_str) {
                    if codex_usage_message_from_line(message).is_some() {
                        return Some(compact_diagnostic_line(message));
                    }
                }
            }
            map.values().find_map(codex_usage_message_from_json)
        }
        serde_json::Value::Array(values) => values.iter().find_map(codex_usage_message_from_json),
        serde_json::Value::String(message) => codex_usage_message_from_line(message),
        _ => None,
    }
}

fn codex_usage_retry_hint(output: &str) -> Option<String> {
    let normalized = output.to_ascii_lowercase();
    for (marker, label) in [
        ("try again at ", "try again at"),
        ("try again after ", "try again after"),
        ("retry after ", "retry after"),
        ("retry-after: ", "retry after"),
    ] {
        let Some(start) = normalized.find(marker).map(|start| start + marker.len()) else {
            continue;
        };
        let remaining = &output[start..];
        let end = remaining
            .char_indices()
            .find_map(|(idx, ch)| {
                matches!(ch, '\n' | '\r' | '"' | '\'' | '`' | '.' | ';').then_some(idx)
            })
            .unwrap_or(remaining.len());
        let value = remaining[..end]
            .trim()
            .trim_matches(|ch: char| matches!(ch, ',' | ':' | ')' | ']' | '}'))
            .trim();
        if !value.is_empty() {
            return Some(format!("{label} {value}"));
        }
    }
    None
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn compact_diagnostic(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(compact_diagnostic_line)
        .take(6)
        .collect::<Vec<_>>()
        .join(" | ")
}

fn compact_diagnostic_line(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_DIAGNOSTIC_CHARS: usize = 280;
    if compact.chars().count() <= MAX_DIAGNOSTIC_CHARS {
        return compact;
    }
    let mut truncated = compact
        .chars()
        .take(MAX_DIAGNOSTIC_CHARS)
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn require_codex_project_config_ready(
    worktree: &Path,
    profile: &str,
    decision: &ExecutionBackendDecision,
) -> Result<(), String> {
    let codex_config = worktree.join(".codex").join("config.toml");
    let content = fs::read_to_string(&codex_config).map_err(|e| {
        format!(
            "Codex project config is required in the run worktree but {} is not readable: {e}",
            codex_config.display()
        )
    })?;
    let parsed = parse_toml_document(&content)
        .map_err(|e| format!("parse {}: {e}", codex_config.display()))?;
    let has_project_defaults = parsed
        .as_table()
        .map(|table| {
            ["approval_policy", "sandbox_mode", "model_reasoning_effort"]
                .iter()
                .any(|key| table.contains_key(*key))
        })
        .unwrap_or(false);
    let has_legacy_profile = parsed
        .get("profiles")
        .and_then(|profiles| profiles.get(profile))
        .is_some();
    if !has_project_defaults && !has_legacy_profile {
        return Err(format!(
            "Codex project config {} does not define top-level Codex defaults or legacy profiles.{profile}",
            codex_config.display()
        ));
    }
    if toml_path(&parsed, "mcp_servers.agentactr").is_none() {
        return Err(format!(
            "Codex project config {} does not define mcp_servers.agentactr",
            codex_config.display()
        ));
    }
    if should_probe_host_codex(decision) {
        if !codex_project_trusted(worktree)? {
            return Err(format!(
                "Codex will skip project-scoped .codex/config.toml for untrusted projects; mark {} as trusted in ~/.codex/config.toml before running agentactr",
                worktree.display()
            ));
        }
        info!(
            worktree = %worktree.display(),
            profile,
            "validated Codex project config and host trust"
        );
    } else {
        info!(
            worktree = %worktree.display(),
            profile,
            "validated Codex project config; Docker runtime will use run-scoped CODEX_HOME trust"
        );
    }
    Ok(())
}

fn codex_project_trusted(worktree: &Path) -> Result<bool, String> {
    let config_path = codex_user_config_path()?;
    let content = match fs::read_to_string(&config_path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(format!(
                "read Codex user config {}: {err}",
                config_path.display()
            ))
        }
    };
    let parsed = parse_toml_document(&content)
        .map_err(|e| format!("parse Codex user config {}: {e}", config_path.display()))?;
    let Some(projects) = parsed.get("projects").and_then(toml::Value::as_table) else {
        return Ok(false);
    };
    let worktree = canonical_path(worktree);
    for (path, value) in projects {
        let trust_level = value
            .get("trust_level")
            .and_then(toml::Value::as_str)
            .unwrap_or_default();
        if trust_level != "trusted" {
            continue;
        }
        let trusted_path = PathBuf::from(path);
        let trusted_path = canonical_path(&trusted_path);
        if worktree == trusted_path || worktree.starts_with(&trusted_path) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn trust_current_codex_project() -> Result<PathBuf, String> {
    let config_path = codex_user_config_path()?;
    let project_path = canonical_path(Path::new("."));
    let content = match fs::read_to_string(&config_path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(format!(
                "read Codex user config {}: {err}",
                config_path.display()
            ))
        }
    };
    let updated = render_codex_project_trust(&content, &project_path)?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create Codex config dir {}: {e}", parent.display()))?;
    }
    fs::write(&config_path, updated)
        .map_err(|e| format!("write Codex user config {}: {e}", config_path.display()))?;
    Ok(config_path)
}

fn render_codex_project_trust(content: &str, project_path: &Path) -> Result<String, String> {
    let mut document = if content.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("parse Codex user config: {e}"))?
    };
    let project_key = project_path.display().to_string();
    set_toml_edit_path(
        &mut document,
        &["projects", project_key.as_str(), "trust_level"],
        toml_edit::value("trusted"),
    )?;
    Ok(document.to_string())
}

fn canonical_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn require_agentactr_mcp_ready(timeout: Duration) -> Result<(), String> {
    require_command("agentactr", &["--help"])?;
    let mut child = Command::new("agentactr")
        .arg("mcp")
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn configured MCP command `agentactr mcp serve`: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or("agentactr MCP stdin unavailable")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("agentactr MCP stdout unavailable")?;
    stdin
        .write_all(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"agentactr-doctor","version":"0.1.0"}}}"#,
        )
        .map_err(|e| format!("write MCP initialize request: {e}"))?;
    stdin
        .write_all(b"\n")
        .map_err(|e| format!("write MCP initialize newline: {e}"))?;
    stdin.flush().map_err(|e| format!("flush MCP stdin: {e}"))?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let mut reader = io::BufReader::new(stdout);
        let result = reader
            .read_line(&mut line)
            .map(|_| line)
            .map_err(|e| format!("read MCP initialize response: {e}"));
        let _ = tx.send(result);
    });
    let line = match rx.recv_timeout(timeout) {
        Ok(result) => result?,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            terminate_child(&mut child, Duration::from_secs(2));
            return Err(format!(
                "configured agentactr MCP command timed out after {}s",
                timeout.as_secs()
            ));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err(
                "configured agentactr MCP command closed before initialize response".to_string(),
            )
        }
    };
    validate_mcp_initialize_response(&line)?;
    stdin
        .write_all(br#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#)
        .map_err(|e| format!("write MCP initialized notification: {e}"))?;
    stdin
        .write_all(b"\n")
        .map_err(|e| format!("write MCP initialized newline: {e}"))?;
    drop(stdin);
    let status =
        wait_child_timeout(&mut child, timeout).map_err(|e| format!("wait MCP self-check: {e}"))?;
    if !status.success() {
        return Err(format!("agentactr MCP self-check exited with {status}"));
    }
    println!("ok: configured agentactr MCP command");
    Ok(())
}

fn validate_mcp_initialize_response(line: &str) -> Result<(), String> {
    let parsed = serde_json::from_str::<serde_json::Value>(line)
        .map_err(|e| format!("parse MCP initialize response: {e}"))?;
    let protocol = parsed
        .pointer("/result/protocolVersion")
        .and_then(serde_json::Value::as_str)
        .ok_or("MCP initialize response missing result.protocolVersion")?;
    if !MCP_PROTOCOL_SUPPORTED.contains(&protocol) {
        return Err(format!("unsupported MCP protocol negotiated: {protocol}"));
    }
    let name = parsed
        .pointer("/result/serverInfo/name")
        .and_then(serde_json::Value::as_str)
        .ok_or("MCP initialize response missing result.serverInfo.name")?;
    if name != "agentactr" {
        return Err(format!("unexpected MCP server name: {name}"));
    }
    if parsed.pointer("/result/capabilities/tools").is_none() {
        return Err("MCP initialize response missing tools capabilities".to_string());
    }
    Ok(())
}

fn wait_child_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("poll child: {e}"))? {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            terminate_child(child, Duration::from_secs(2));
            return Err(format!("child did not exit within {}s", timeout.as_secs()));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn terminate_child(child: &mut std::process::Child, grace: Duration) {
    terminate_process_group(child, "TERM");
    if wait_for_process_group_exit(child, grace) {
        return;
    }
    terminate_process_group(child, "KILL");
    let _ = child.kill();
    let _ = wait_for_process_group_exit(child, grace);
    let _ = child.wait();
}

fn wait_for_process_group_exit(child: &mut std::process::Child, grace: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < grace {
        let root_exited = matches!(child.try_wait(), Ok(Some(_)) | Err(_));
        if cfg!(not(unix)) && root_exited {
            return true;
        }
        if cfg!(unix) && !quality_process_group_alive(child.id()) {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

#[cfg(unix)]
fn configure_quality_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
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
fn configure_quality_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_group(child: &std::process::Child, signal: &str) {
    let _ = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(format!("-{}", child.id()))
        .status();
}

#[cfg(not(unix))]
fn terminate_process_group(_child: &std::process::Child, _signal: &str) {}

#[cfg(unix)]
fn quality_process_group_alive(process_group_id: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(format!("-{process_group_id}"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn quality_process_group_alive(_process_group_id: u32) -> bool {
    false
}

fn run_status(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("run command: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command exited with {status}"))
    }
}

fn print_run_banner(
    repo: &str,
    issue: &str,
    policy: &RunPolicy,
    tracker_capabilities: &AdapterCapabilities,
) {
    println!(
        "Human intervention: {} (source: {}; CLI: --human-intervention {})",
        policy.human_intervention.as_config(),
        policy.human_intervention_source.as_text(),
        policy.human_intervention.as_cli()
    );
    println!(
        "Codex approval policy: {} (source: {}; CLI: --codex-approval {})",
        policy.codex_approval.as_config(),
        policy.codex_approval_source.as_text(),
        policy.codex_approval.as_cli()
    );
    println!(
        "GitHub finalization: {} (effective: {}; source: {})",
        policy.github_finalization.as_config(),
        policy.github_finalization_effective_text(tracker_capabilities),
        policy.github_finalization_source.as_text()
    );
    if policy.github_finalization != GithubFinalizationSetting::Disabled
        && !tracker_supports_github_finalization(tracker_capabilities)
    {
        println!(
            "GitHub finalization capability: disabled in this milestone; tracker mutation ports are unavailable"
        );
    }
    println!("Runtime prompting: {}", policy.runtime_prompting());
    println!();
    println!("Run target: {repo}#{issue}");
    match policy.human_intervention {
        HumanInterventionSetting::FailClosed => println!("This run will not wait for human input. Approval requests, ambiguous diffs, or undecidable gates fail closed."),
        HumanInterventionSetting::Interactive => println!("This run may ask for explicit operator decisions when Codex approval or SDK intervention is required."),
        HumanInterventionSetting::ReviewRequired => println!("This run remains non-interactive during execution and stops before final GitHub finalization for review."),
    }
    println!();
    println!("To change behavior for this run only:");
    println!("  agentactr run issue --repo {repo} --issue {issue} --human-intervention interactive --codex-approval on-request");
    println!("To persist the default:");
    println!("  agentactr config set human_intervention.mode interactive");
    println!("  agentactr config set codex.approval_policy on-request");
    println!("  agentactr config set github.finalization require_human_review");
}

fn new_run_id(issue: &str) -> String {
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("issue-{issue}-{epoch_ms}")
}

fn read_repo_from_config() -> Option<String> {
    let content = fs::read_to_string("agentactr.toml").ok()?;
    find_config_value(&content, "tracker.repo")
}

fn load_agentactr_config(repo_override: Option<&str>) -> Result<AgentactrConfig, String> {
    let repo = repo_override
        .map(ToString::to_string)
        .or_else(read_repo_from_config)
        .unwrap_or_else(|| "OWNER/REPO".to_string());
    let mut config = AgentactrConfig::strict_defaults(repo);
    if let Ok(content) = fs::read_to_string("agentactr.toml") {
        let parsed =
            parse_toml_document(&content).map_err(|e| format!("parse agentactr.toml: {e}"))?;
        merge_config_from_toml(&mut config, &parsed)?;
        config.codex.validate_milestone_policy()?;
    }
    if let Some(repo) = repo_override {
        config.tracker.repo = repo.to_string();
    }
    merge_config_from_env(&mut config)?;
    config.codex.validate_milestone_policy()?;
    Ok(config)
}

fn merge_config_from_toml(
    config: &mut AgentactrConfig,
    parsed: &toml::Value,
) -> Result<(), String> {
    set_string(parsed, "tracker.kind", &mut config.tracker.kind);
    set_string(parsed, "tracker.repo", &mut config.tracker.repo);
    set_string(parsed, "tracker.token_env", &mut config.tracker.token_env);
    set_string(
        parsed,
        "tracker.github_api_version",
        &mut config.tracker.github_api_version,
    );
    set_string_vec(
        parsed,
        "tracker.active_labels",
        &mut config.tracker.active_labels,
    );
    set_string_vec(
        parsed,
        "tracker.ignore_labels",
        &mut config.tracker.ignore_labels,
    );
    set_string(
        parsed,
        "tracker.claim_label",
        &mut config.tracker.claim_label,
    );
    set_string(
        parsed,
        "tracker.running_label",
        &mut config.tracker.running_label,
    );
    set_string(
        parsed,
        "tracker.failed_label",
        &mut config.tracker.failed_label,
    );
    set_string(parsed, "tracker.done_label", &mut config.tracker.done_label);
    set_string(parsed, "codex.command", &mut config.codex.command);
    set_string(parsed, "codex.mode", &mut config.codex.mode);
    set_string(parsed, "codex.profile", &mut config.codex.profile);
    set_string(
        parsed,
        "codex.approval_policy",
        &mut config.codex.approval_policy,
    );
    set_string(parsed, "codex.sandbox_mode", &mut config.codex.sandbox_mode);
    set_string(parsed, "codex.network", &mut config.codex.network);
    set_string(
        parsed,
        "codex.default_model",
        &mut config.codex.default_model,
    );
    set_string(
        parsed,
        "codex.model_reasoning_effort",
        &mut config.codex.model_reasoning_effort,
    );
    set_string(
        parsed,
        "codex.openai_api_key_env",
        &mut config.codex.openai_api_key_env,
    );
    set_string(
        parsed,
        "codex.app_server_transport",
        &mut config.codex.app_server_transport,
    );
    set_bool(
        parsed,
        "codex.app_server_experimental_api",
        &mut config.codex.app_server_experimental_api,
    );
    set_string(parsed, "codex.sdk_bridge", &mut config.codex.sdk_bridge);
    set_string(
        parsed,
        "codex.fallback_mode",
        &mut config.codex.fallback_mode,
    );
    if let Some(value) = toml_path(parsed, "codex.auth_mode").and_then(toml::Value::as_str) {
        config.codex.auth_mode = CodexAuthMode::parse(value)?;
    }
    set_string(
        parsed,
        "human_intervention.mode",
        &mut config.human_intervention.mode,
    );
    set_string(
        parsed,
        "human_intervention.on_codex_approval_request",
        &mut config.human_intervention.on_codex_approval_request,
    );
    set_string(
        parsed,
        "human_intervention.on_ambiguous_diff",
        &mut config.human_intervention.on_ambiguous_diff,
    );
    set_string(
        parsed,
        "human_intervention.on_review_disagreement",
        &mut config.human_intervention.on_review_disagreement,
    );
    set_string(
        parsed,
        "human_intervention.on_missing_codex_auth",
        &mut config.human_intervention.on_missing_codex_auth,
    );
    set_string(
        parsed,
        "human_intervention.on_missing_github_token",
        &mut config.human_intervention.on_missing_github_token,
    );
    set_bool(
        parsed,
        "human_intervention.run_start_banner",
        &mut config.human_intervention.run_start_banner,
    );
    set_bool(
        parsed,
        "human_intervention.print_override_steps",
        &mut config.human_intervention.print_override_steps,
    );
    set_string(
        parsed,
        "github.finalization",
        &mut config.github.finalization,
    );
    set_string(
        parsed,
        "github.standard_label_policy",
        &mut config.github.standard_label_policy,
    );
    set_string(
        parsed,
        "github.project_automation",
        &mut config.github.project_automation,
    );
    set_string(
        parsed,
        "github.project_owner",
        &mut config.github.project_owner,
    );
    set_u32(
        parsed,
        "github.project_number",
        &mut config.github.project_number,
    );
    set_string(
        parsed,
        "github.project_title",
        &mut config.github.project_title,
    );
    set_string(
        parsed,
        "github.project_priority_field",
        &mut config.github.project_priority_field,
    );
    set_string(
        parsed,
        "github.project_size_field",
        &mut config.github.project_size_field,
    );
    set_string(parsed, "mcp.default_policy", &mut config.mcp.default_policy);
    set_string(
        parsed,
        "mcp.remote_research_servers",
        &mut config.mcp.remote_research_servers,
    );
    set_string(
        parsed,
        "mcp.remote_github_read_tools",
        &mut config.mcp.remote_github_read_tools,
    );
    set_string(
        parsed,
        "mcp.remote_github_write_tools",
        &mut config.mcp.remote_github_write_tools,
    );
    set_string(
        parsed,
        "mcp.openai_developer_docs",
        &mut config.mcp.openai_developer_docs,
    );
    set_string(
        parsed,
        "mcp.google_developer_api",
        &mut config.mcp.google_developer_api,
    );
    set_string(parsed, "mcp.huggingface", &mut config.mcp.huggingface);
    set_string(parsed, "mcp.github_remote", &mut config.mcp.github_remote);
    set_bool(
        parsed,
        "mcp.fail_on_required_mcp_missing",
        &mut config.mcp.fail_on_required_mcp_missing,
    );
    set_string(
        parsed,
        "repository.empty_repo_policy",
        &mut config.repository.empty_repo_policy,
    );
    set_string(
        parsed,
        "repository.declared_primary_stack",
        &mut config.repository.declared_primary_stack,
    );
    set_string(
        parsed,
        "repository.allowed_bootstrap",
        &mut config.repository.allowed_bootstrap,
    );
    set_string(
        parsed,
        "repository.bootstrap_prereqs",
        &mut config.repository.bootstrap_prereqs,
    );
    set_bool(
        parsed,
        "repository.fail_on_low_confidence_stack_detection",
        &mut config.repository.fail_on_low_confidence_stack_detection,
    );
    set_string(parsed, "vcs.kind", &mut config.vcs.kind);
    set_string(
        parsed,
        "vcs.workspace_strategy",
        &mut config.vcs.workspace_strategy,
    );
    set_string(parsed, "vcs.base_ref", &mut config.vcs.base_ref);
    set_string(parsed, "vcs.worktree_root", &mut config.vcs.worktree_root);
    set_string(
        parsed,
        "vcs.branch_template",
        &mut config.vcs.branch_template,
    );
    set_bool(
        parsed,
        "vcs.record_base_commit",
        &mut config.vcs.record_base_commit,
    );
    set_bool(
        parsed,
        "vcs.fail_on_dirty_source_checkout",
        &mut config.vcs.fail_on_dirty_source_checkout,
    );
    set_bool(
        parsed,
        "vcs.copy_runtime_config_to_worktree",
        &mut config.vcs.copy_runtime_config_to_worktree,
    );
    set_bool(
        parsed,
        "vcs.detect_cross_issue_file_overlap",
        &mut config.vcs.detect_cross_issue_file_overlap,
    );
    set_string(parsed, "vcs.overlap_policy", &mut config.vcs.overlap_policy);
    set_string(parsed, "quality.profile", &mut config.quality.profile);
    set_string(
        parsed,
        "quality.pre_commit_mode",
        &mut config.quality.pre_commit_mode,
    );
    set_string(
        parsed,
        "quality.technology_detection",
        &mut config.quality.technology_detection,
    );
    set_string_vec(parsed, "quality.domains", &mut config.quality.domains);
    set_string_vec(
        parsed,
        "quality.domain_gate_opt_ins",
        &mut config.quality.domain_gate_opt_ins,
    );
    set_bool(
        parsed,
        "quality.run_existing_pre_commit_config",
        &mut config.quality.run_existing_pre_commit_config,
    );
    set_bool(
        parsed,
        "quality.fail_on_missing_toolchain",
        &mut config.quality.fail_on_missing_toolchain,
    );
    set_bool(
        parsed,
        "quality.fail_on_untracked_generated_files",
        &mut config.quality.fail_on_untracked_generated_files,
    );
    set_bool(
        parsed,
        "quality.allow_test_omission_reason",
        &mut config.quality.allow_test_omission_reason,
    );
    set_string(
        parsed,
        "quality.artifact_dir",
        &mut config.quality.artifact_dir,
    );
    set_bool(
        parsed,
        "quality.dependency_checks",
        &mut config.quality.dependency_checks,
    );
    set_bool(
        parsed,
        "quality.architecture_checks",
        &mut config.quality.architecture_checks,
    );
    set_string(
        parsed,
        "quality.tool_pinning",
        &mut config.quality.tool_pinning,
    );
    set_string_vec(
        parsed,
        "architecture.domains",
        &mut config.architecture.domains,
    );
    set_string(
        parsed,
        "architecture.domain_graph_artifact",
        &mut config.architecture.domain_graph_artifact,
    );
    set_bool(
        parsed,
        "architecture.fail_on_domain_drift",
        &mut config.architecture.fail_on_domain_drift,
    );
    set_string_vec(
        parsed,
        "templates.enabled_domains",
        &mut config.templates.enabled_domains,
    );
    set_string(
        parsed,
        "templates.framework_profile",
        &mut config.templates.framework_profile,
    );
    set_string(
        parsed,
        "templates.agents_policy",
        &mut config.templates.agents_policy,
    );
    set_string(parsed, "commit.mode", &mut config.commit.mode);
    set_bool(parsed, "commit.signoff", &mut config.commit.signoff);
    set_string(parsed, "commit.gpg_sign", &mut config.commit.gpg_sign);
    set_string(
        parsed,
        "commit.message_template",
        &mut config.commit.message_template,
    );
    set_string_vec(
        parsed,
        "commit.required_trailers",
        &mut config.commit.required_trailers,
    );
    set_string(parsed, "merge.mode", &mut config.merge.mode);
    set_string(parsed, "merge.push", &mut config.merge.push);
    set_string(parsed, "merge.strategy", &mut config.merge.strategy);
    set_bool(
        parsed,
        "merge.require_clean_rebase",
        &mut config.merge.require_clean_rebase,
    );
    set_bool(
        parsed,
        "merge.require_no_cross_issue_overlap",
        &mut config.merge.require_no_cross_issue_overlap,
    );
    set_bool(
        parsed,
        "merge.require_human_review_for_merge",
        &mut config.merge.require_human_review_for_merge,
    );
    set_string(parsed, "workspace.root", &mut config.workspace.root);
    set_bool(
        parsed,
        "workspace.keep_successful",
        &mut config.workspace.keep_successful,
    );
    set_bool(
        parsed,
        "workspace.keep_failed",
        &mut config.workspace.keep_failed,
    );
    set_u64(
        parsed,
        "scheduling.poll_interval_ms",
        &mut config.scheduling.poll_interval_ms,
    );
    set_u64(
        parsed,
        "scheduling.max_concurrent_issue_runs",
        &mut config.scheduling.max_concurrent_issue_runs,
    );
    set_u64(
        parsed,
        "scheduling.lease_ttl_ms",
        &mut config.scheduling.lease_ttl_ms,
    );
    set_u64(
        parsed,
        "scheduling.max_retries",
        &mut config.scheduling.max_retries,
    );
    set_bool(parsed, "spawn.enabled", &mut config.spawn.enabled);
    set_u64(
        parsed,
        "spawn.max_child_agents_per_issue",
        &mut config.spawn.max_child_agents_per_issue,
    );
    set_u64(
        parsed,
        "spawn.max_spawn_depth",
        &mut config.spawn.max_spawn_depth,
    );
    set_bool(
        parsed,
        "spawn.allow_parallel_read_only",
        &mut config.spawn.allow_parallel_read_only,
    );
    set_bool(
        parsed,
        "spawn.allow_parallel_writers",
        &mut config.spawn.allow_parallel_writers,
    );
    set_string(parsed, "spawn.strategy", &mut config.spawn.strategy);
    set_u64(
        parsed,
        "spawn.max_total_uncached_input_tokens",
        &mut config.spawn.max_total_uncached_input_tokens,
    );
    set_u64(
        parsed,
        "spawn.max_child_uncached_input_tokens",
        &mut config.spawn.max_child_uncached_input_tokens,
    );
    set_u64(
        parsed,
        "spawn.max_child_output_tokens",
        &mut config.spawn.max_child_output_tokens,
    );
    set_string(
        parsed,
        "spawn.artifact_handoff",
        &mut config.spawn.artifact_handoff,
    );
    set_bool(
        parsed,
        "spawn.pause_on_memory_pressure",
        &mut config.spawn.pause_on_memory_pressure,
    );
    set_string(parsed, "execution.backend", &mut config.execution.backend);
    set_bool(
        parsed,
        "execution.strict_memory_required",
        &mut config.execution.strict_memory_required,
    );
    set_string(
        parsed,
        "execution.docker.command",
        &mut config.execution.docker.command,
    );
    set_string(
        parsed,
        "execution.docker.image",
        &mut config.execution.docker.image,
    );
    set_string(
        parsed,
        "execution.docker.pull_policy",
        &mut config.execution.docker.pull_policy,
    );
    set_string(
        parsed,
        "execution.docker.network",
        &mut config.execution.docker.network,
    );
    set_string(
        parsed,
        "execution.docker.workspace_mount",
        &mut config.execution.docker.workspace_mount,
    );
    set_string(
        parsed,
        "execution.docker.artifact_mount",
        &mut config.execution.docker.artifact_mount,
    );
    set_bool(
        parsed,
        "execution.docker.remove_containers",
        &mut config.execution.docker.remove_containers,
    );
    set_string(
        parsed,
        "execution.docker.container_prefix",
        &mut config.execution.docker.container_prefix,
    );
    set_bool(
        parsed,
        "linux_memory.enabled",
        &mut config.linux_memory.enabled,
    );
    set_string(
        parsed,
        "linux_memory.cgroup_root",
        &mut config.linux_memory.cgroup_root,
    );
    set_string(
        parsed,
        "linux_memory.root_group",
        &mut config.linux_memory.root_group,
    );
    set_string(parsed, "linux_memory.mode", &mut config.linux_memory.mode);
    set_bool(
        parsed,
        "linux_memory.cgroup_v2_required",
        &mut config.linux_memory.cgroup_v2_required,
    );
    set_bool(
        parsed,
        "linux_memory.psi_required",
        &mut config.linux_memory.psi_required,
    );
    set_string(
        parsed,
        "linux_memory.per_issue_memory_high",
        &mut config.linux_memory.per_issue_memory_high,
    );
    set_string(
        parsed,
        "linux_memory.per_issue_memory_max",
        &mut config.linux_memory.per_issue_memory_max,
    );
    set_string(
        parsed,
        "linux_memory.per_agent_memory_high",
        &mut config.linux_memory.per_agent_memory_high,
    );
    set_string(
        parsed,
        "linux_memory.per_agent_memory_max",
        &mut config.linux_memory.per_agent_memory_max,
    );
    set_u64(
        parsed,
        "linux_memory.psi_memory_some_threshold_us",
        &mut config.linux_memory.psi_memory_some_threshold_us,
    );
    set_u64(
        parsed,
        "linux_memory.psi_memory_window_us",
        &mut config.linux_memory.psi_memory_window_us,
    );
    set_i64(
        parsed,
        "linux_memory.oom_score_adj",
        &mut config.linux_memory.oom_score_adj,
    );
    set_string(
        parsed,
        "linux_memory.setrlimit_address_space",
        &mut config.linux_memory.setrlimit_address_space,
    );
    set_string(
        parsed,
        "linux_memory.setrlimit_file_size",
        &mut config.linux_memory.setrlimit_file_size,
    );
    set_string(
        parsed,
        "linux_memory.kill_policy",
        &mut config.linux_memory.kill_policy,
    );
    set_string(
        parsed,
        "linux_memory.oom_policy",
        &mut config.linux_memory.oom_policy,
    );
    if toml_path(parsed, "linux_memory.kill_policy").is_some()
        && toml_path(parsed, "linux_memory.oom_policy").is_none()
    {
        config.linux_memory.oom_policy = config.linux_memory.kill_policy.clone();
    }
    set_string(
        parsed,
        "observability.jsonl",
        &mut config.observability.jsonl,
    );
    set_string(
        parsed,
        "observability.sqlite",
        &mut config.observability.sqlite,
    );
    set_string(
        parsed,
        "observability.artifact_root",
        &mut config.observability.artifact_root,
    );
    set_bool(
        parsed,
        "observability.otel_enabled",
        &mut config.observability.otel_enabled,
    );
    set_string(
        parsed,
        "observability.otel_endpoint",
        &mut config.observability.otel_endpoint,
    );
    set_string(
        parsed,
        "observability.debug_bundle_root",
        &mut config.observability.debug_bundle_root,
    );
    set_bool(
        parsed,
        "observability.redact_secrets",
        &mut config.observability.redact_secrets,
    );
    Ok(())
}

fn merge_config_from_env(config: &mut AgentactrConfig) -> Result<(), String> {
    set_env_string("AGENTACTR_REPO", &mut config.tracker.repo);
    set_env_string("AGENTACTR_GITHUB_TOKEN_ENV", &mut config.tracker.token_env);
    set_env_string(
        "AGENTACTR_GITHUB_API_VERSION",
        &mut config.tracker.github_api_version,
    );
    set_env_string("AGENTACTR_CODEX_COMMAND", &mut config.codex.command);
    set_env_string("AGENTACTR_CODEX_PROFILE", &mut config.codex.profile);
    set_env_string(
        "AGENTACTR_CODEX_APPROVAL",
        &mut config.codex.approval_policy,
    );
    set_env_string("AGENTACTR_CODEX_SANDBOX", &mut config.codex.sandbox_mode);
    set_env_string(
        "AGENTACTR_CODEX_API_KEY_ENV",
        &mut config.codex.openai_api_key_env,
    );
    set_env_string(
        "AGENTACTR_CODEX_APP_SERVER_TRANSPORT",
        &mut config.codex.app_server_transport,
    );
    if let Ok(value) = env::var("AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API") {
        config.codex.app_server_experimental_api =
            parse_bool_env("AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API", &value)?;
    }
    set_env_string("AGENTACTR_CODEX_SDK_BRIDGE", &mut config.codex.sdk_bridge);
    set_env_string(
        "AGENTACTR_CODEX_FALLBACK_MODE",
        &mut config.codex.fallback_mode,
    );
    set_env_string(
        "AGENTACTR_HUMAN_INTERVENTION",
        &mut config.human_intervention.mode,
    );
    set_env_string(
        "AGENTACTR_GITHUB_FINALIZATION",
        &mut config.github.finalization,
    );
    set_env_string(
        "AGENTACTR_GITHUB_STANDARD_LABEL_POLICY",
        &mut config.github.standard_label_policy,
    );
    set_env_string(
        "AGENTACTR_GITHUB_PROJECT_AUTOMATION",
        &mut config.github.project_automation,
    );
    set_env_string("AGENTACTR_VCS_BASE_REF", &mut config.vcs.base_ref);
    set_env_string("AGENTACTR_VCS_WORKTREE_ROOT", &mut config.vcs.worktree_root);
    set_env_string(
        "AGENTACTR_OBSERVABILITY_JSONL",
        &mut config.observability.jsonl,
    );
    set_env_string(
        "AGENTACTR_ARTIFACT_ROOT",
        &mut config.observability.artifact_root,
    );
    Ok(())
}

fn set_env_string(name: &str, target: &mut String) {
    if let Ok(value) = env::var(name) {
        *target = value;
    }
}

fn parse_bool_env(name: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => Err(format!("{name} must be a boolean-like value, got {other}")),
    }
}

fn set_string(parsed: &toml::Value, dotted_key: &str, target: &mut String) {
    if let Some(value) = toml_path(parsed, dotted_key).and_then(toml::Value::as_str) {
        *target = value.to_string();
    }
}

fn set_string_vec(parsed: &toml::Value, dotted_key: &str, target: &mut Vec<String>) {
    let Some(values) = toml_path(parsed, dotted_key).and_then(toml::Value::as_array) else {
        return;
    };
    let parsed_values = values
        .iter()
        .filter_map(toml::Value::as_str)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !parsed_values.is_empty() || values.is_empty() {
        *target = parsed_values;
    }
}

fn set_bool(parsed: &toml::Value, dotted_key: &str, target: &mut bool) {
    if let Some(value) = toml_path(parsed, dotted_key).and_then(toml::Value::as_bool) {
        *target = value;
    }
}

fn set_u64(parsed: &toml::Value, dotted_key: &str, target: &mut u64) {
    if let Some(value) = toml_path(parsed, dotted_key)
        .and_then(toml::Value::as_integer)
        .and_then(|value| u64::try_from(value).ok())
    {
        *target = value;
    }
}

fn set_u32(parsed: &toml::Value, dotted_key: &str, target: &mut u32) {
    if let Some(value) = toml_path(parsed, dotted_key)
        .and_then(toml::Value::as_integer)
        .and_then(|value| u32::try_from(value).ok())
    {
        *target = value;
    }
}

fn set_i64(parsed: &toml::Value, dotted_key: &str, target: &mut i64) {
    if let Some(value) = toml_path(parsed, dotted_key).and_then(toml::Value::as_integer) {
        *target = value;
    }
}

fn toml_path<'a>(parsed: &'a toml::Value, dotted_key: &str) -> Option<&'a toml::Value> {
    let mut current = parsed;
    for segment in dotted_key.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn parse_toml_document(content: &str) -> Result<toml::Value, String> {
    toml::from_str::<toml::Table>(content)
        .map(toml::Value::Table)
        .map_err(|e| e.to_string())
}

fn find_config_value(content: &str, dotted_key: &str) -> Option<String> {
    let parsed = parse_toml_document(content).ok()?;
    let current = toml_path(&parsed, dotted_key)?;
    current
        .as_str()
        .map(ToString::to_string)
        .or_else(|| Some(current.to_string()))
}

fn set_config_value(path: &str, dotted_key: &str, value: &str) -> Result<(), String> {
    if !CONFIG_KEY_VALUES.contains(&dotted_key) {
        return Err(format!("unsupported config key `{dotted_key}`"));
    }
    let content = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let mut document = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("parse {path}: {e}"))?;
    let segments = dotted_key.split('.').collect::<Vec<_>>();
    if segments.len() < 2 {
        return Err("key must use section.key form".to_string());
    }
    let value = canonical_config_set_value(dotted_key, value)?;
    let value = parse_toml_edit_scalar(&value)?;
    set_toml_edit_path(&mut document, &segments, value)?;
    let updated = document.to_string();
    let parsed = parse_toml_document(&updated).map_err(|e| format!("parse updated {path}: {e}"))?;
    let repo =
        find_config_value(&updated, "tracker.repo").unwrap_or_else(|| "OWNER/REPO".to_string());
    let mut config = AgentactrConfig::strict_defaults(repo);
    merge_config_from_toml(&mut config, &parsed)?;
    config.codex.validate_milestone_policy()?;
    let rendered = render_agentactr_toml(&config);
    fs::write(path, rendered).map_err(|e| format!("write {path}: {e}"))
}

fn canonical_config_set_value(dotted_key: &str, value: &str) -> Result<String, String> {
    match dotted_key {
        "codex.mode" => Ok(CodexMode::parse(value)?.as_str().to_string()),
        "codex.app_server_transport" => {
            Ok(CodexAppServerTransport::parse(value)?.as_str().to_string())
        }
        "codex.sdk_bridge" => Ok(CodexSdkBridge::parse(value)?.as_str().to_string()),
        "codex.fallback_mode" => Ok(CodexFallbackMode::parse(value)?.as_str().to_string()),
        "github.standard_label_policy" => canonical_static_value(
            value,
            GITHUB_STANDARD_LABEL_POLICY_VALUES,
            "github.standard_label_policy",
        ),
        "github.project_automation" => canonical_static_value(
            value,
            GITHUB_PROJECT_AUTOMATION_VALUES,
            "github.project_automation",
        ),
        _ => Ok(value.to_string()),
    }
}

fn canonical_static_value(value: &str, allowed: &[&str], key: &str) -> Result<String, String> {
    if allowed.contains(&value) {
        Ok(value.to_string())
    } else {
        Err(format!(
            "unsupported {key} value `{value}`; expected one of {}",
            allowed.join("|")
        ))
    }
}

fn parse_toml_edit_scalar(value: &str) -> Result<toml_edit::Item, String> {
    match value {
        "true" => Ok(toml_edit::value(true)),
        "false" => Ok(toml_edit::value(false)),
        v if v.parse::<i64>().is_ok() => Ok(toml_edit::value(v.parse::<i64>().unwrap_or_default())),
        v if v.starts_with('[') || v.starts_with('{') => v
            .parse::<toml_edit::Value>()
            .map(toml_edit::Item::Value)
            .map_err(|e| format!("parse TOML value `{v}`: {e}")),
        v => Ok(toml_edit::value(v)),
    }
}

fn set_toml_edit_path(
    root: &mut toml_edit::DocumentMut,
    segments: &[&str],
    value: toml_edit::Item,
) -> Result<(), String> {
    let mut current = root.as_item_mut();
    for segment in &segments[..segments.len() - 1] {
        if current.get(segment).is_none() {
            current[segment] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        current = current
            .get_mut(segment)
            .ok_or_else(|| format!("missing TOML section `{segment}`"))?;
    }
    let key = segments.last().ok_or("empty TOML path")?;
    if !current.is_table() && !current.is_inline_table() {
        return Err(format!(
            "TOML section `{}` is not a table",
            segments[..segments.len() - 1].join(".")
        ));
    }
    current[key] = value;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn restore_env(name: &str, value: Option<String>) {
        match value {
            Some(value) => env::set_var(name, value),
            None => env::remove_var(name),
        }
    }

    #[test]
    fn finalize_approve_requires_successful_quality_status_not_only_report_file() {
        let root = env::temp_dir().join(format!(
            "agentactr-quality-status-test-{}-{}",
            std::process::id(),
            new_run_id("quality-status")
        ));
        fs::create_dir_all(&root).unwrap();
        let report = root.join("quality_report.txt");
        fs::write(&report, "quality gate failed\n").unwrap();
        write_quality_status(&report, false, Some("failed gate")).unwrap();

        let err = agentactr_sdk::load_recorded_quality_summary(&report, FinalizeDecision::Approve)
            .unwrap_err();

        let _ = fs::remove_dir_all(root);
        assert!(err.contains("requires successful quality status"));
        assert!(err.contains("failed gate"));
    }

    #[test]
    fn finalize_reject_can_load_failed_quality_status() {
        let root = env::temp_dir().join(format!(
            "agentactr-quality-reject-test-{}-{}",
            std::process::id(),
            new_run_id("quality-status")
        ));
        fs::create_dir_all(&root).unwrap();
        let report = root.join("quality_report.txt");
        fs::write(&report, "quality gate failed\n").unwrap();
        write_quality_status(&report, false, Some("failed gate")).unwrap();

        let summary =
            agentactr_sdk::load_recorded_quality_summary(&report, FinalizeDecision::Reject)
                .unwrap();

        let _ = fs::remove_dir_all(root);
        assert!(!summary.success);
        assert_eq!(summary.failed_reason.as_deref(), Some("failed gate"));
    }

    fn run_git_test_command(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("run git test command `{}`: {e}", args.join(" ")));
        assert!(
            output.status.success(),
            "git test command `{}` failed with {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn empty_test_domain_graph() -> agentactr_sdk::DomainGraph {
        agentactr_sdk::DomainGraph {
            schema_version: agentactr_sdk::DOMAIN_GRAPH_SCHEMA_VERSION.to_string(),
            artifact_format_version: agentactr_sdk::DOMAIN_GRAPH_ARTIFACT_FORMAT_VERSION
                .to_string(),
            producer: "agentactr-cli-test".to_string(),
            created_at: "0".to_string(),
            repo: "test".to_string(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    #[test]
    fn run_policy_uses_declared_stack_for_empty_repo_gate() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.repository.declared_primary_stack = "rust".to_string();
        let inspection = RepoInspection {
            root: PathBuf::from("."),
            is_git: true,
            is_empty: true,
            detected_stack: StackKind::Unknown,
            primary_stack: StackKind::Unknown,
            confidence: 0,
            evidence_files: Vec::new(),
            missing_prerequisites: vec![
                "empty repository requires repository.declared_primary_stack before bootstrap"
                    .to_string(),
            ],
            setup_guidance: vec![
                "agentactr config set repository.declared_primary_stack rust|typescript|golang|python"
                    .to_string(),
            ],
            selected_quality_profile: "strict".to_string(),
            quality_plan: Vec::new(),
            domain_profiles: Vec::new(),
            domain_quality_plan: Vec::new(),
            domain_graph: empty_test_domain_graph(),
        };

        let inspection = effective_repo_inspection(inspection, &config);

        assert_eq!(inspection.primary_stack, StackKind::Rust);
        assert!(!inspection.quality_plan.is_empty());
        assert!(inspection.missing_prerequisites.is_empty());
        assert!(fail_on_blocking_repo_findings(&EffectiveRepositoryContext {
            inspection,
            stack_source: RepositoryStackSource::Config,
            fail_on_low_confidence_stack_detection: config
                .repository
                .fail_on_low_confidence_stack_detection,
        })
        .is_ok());
    }

    #[test]
    fn run_policy_rejects_empty_repo_without_declared_stack() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let inspection = RepoInspection {
            root: PathBuf::from("."),
            is_git: true,
            is_empty: true,
            detected_stack: StackKind::Unknown,
            primary_stack: StackKind::Unknown,
            confidence: 0,
            evidence_files: Vec::new(),
            missing_prerequisites: Vec::new(),
            setup_guidance: Vec::new(),
            selected_quality_profile: "strict".to_string(),
            quality_plan: Vec::new(),
            domain_profiles: Vec::new(),
            domain_quality_plan: Vec::new(),
            domain_graph: empty_test_domain_graph(),
        };

        let err = fail_on_blocking_repo_findings(&EffectiveRepositoryContext {
            inspection,
            stack_source: RepositoryStackSource::Unresolved,
            fail_on_low_confidence_stack_detection: config
                .repository
                .fail_on_low_confidence_stack_detection,
        })
        .unwrap_err();

        assert!(err.contains("declared_primary_stack"));
    }

    #[test]
    fn run_policy_rejects_ambiguous_stack_without_declared_stack() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let inspection = RepoInspection {
            root: PathBuf::from("."),
            is_git: true,
            is_empty: false,
            detected_stack: StackKind::Mixed,
            primary_stack: StackKind::Mixed,
            confidence: 70,
            evidence_files: vec!["package.json".to_string(), "pyproject.toml".to_string()],
            missing_prerequisites: Vec::new(),
            setup_guidance: Vec::new(),
            selected_quality_profile: "strict".to_string(),
            quality_plan: Vec::new(),
            domain_profiles: Vec::new(),
            domain_quality_plan: Vec::new(),
            domain_graph: empty_test_domain_graph(),
        };

        let err = fail_on_blocking_repo_findings(&EffectiveRepositoryContext {
            inspection,
            stack_source: RepositoryStackSource::Unresolved,
            fail_on_low_confidence_stack_detection: config
                .repository
                .fail_on_low_confidence_stack_detection,
        })
        .unwrap_err();

        assert!(err.contains("ambiguous or low-confidence"));
        assert!(err.contains("repository.declared_primary_stack"));
    }

    #[test]
    fn tracker_stack_label_allowlist_rejects_go_alias() {
        assert_eq!(
            stack_from_tracker_labels(&["stack:golang".to_string()])
                .unwrap()
                .map(|(stack, _)| stack),
            Some(StackKind::Golang)
        );
        assert!(stack_from_tracker_labels(&["stack:go".to_string()])
            .unwrap()
            .is_none());
    }

    #[test]
    fn tracker_structured_stack_field_is_supported() {
        let metadata = TrackerStackMetadata {
            labels: Vec::new(),
            body: "agentactr_stack: python\n".to_string(),
        };

        let resolved = stack_from_tracker_metadata(&metadata).unwrap();

        assert_eq!(
            resolved.map(|(stack, source)| (stack, source.as_display())),
            Some((
                StackKind::Python,
                "tracker_field:agentactr_stack".to_string()
            ))
        );
    }

    #[test]
    fn conflicting_tracker_stack_field_and_label_fail_closed() {
        let metadata = TrackerStackMetadata {
            labels: vec!["stack:rust".to_string()],
            body: "agentactr_stack: python\n".to_string(),
        };

        let err = stack_from_tracker_metadata(&metadata).unwrap_err();

        assert!(err.contains("conflicting tracker stack declarations"));
    }

    #[test]
    fn gitignore_merge_appends_each_missing_generated_entry() {
        let existing = ".agentactr/runs/\n";
        let generated = render_gitignore_additions();

        let updated = merge_gitignore_additions(existing, &generated).unwrap();

        assert!(updated.contains(".agentactr/runs/"));
        assert!(updated.contains(".agentactr/artifacts/"));
        assert!(updated.contains(".agentactr/debug/"));
        assert!(updated.contains(".agentactr/workspaces/"));
        assert!(updated.contains(".agentactr/worktrees/"));
        assert_eq!(updated.matches(".agentactr/runs/").count(), 1);
    }

    #[test]
    fn gitignore_merge_is_noop_when_generated_entries_exist() {
        let generated = render_gitignore_additions();

        assert!(merge_gitignore_additions(&generated, &generated).is_none());
    }

    #[test]
    fn codex_config_mcp_server_enabled_detects_enabled_server() {
        let root = env::temp_dir().join(format!(
            "agentactr-codex-mcp-config-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            r#"[mcp_servers.github_remote]
url = "https://api.githubcopilot.com/mcp/"
enabled = true

[mcp_servers.disabled_server]
enabled = false
"#,
        )
        .unwrap();

        assert!(codex_config_mcp_server_enabled(
            &config_path,
            "github_remote"
        ));
        assert!(!codex_config_mcp_server_enabled(
            &config_path,
            "disabled_server"
        ));
        assert!(!codex_config_mcp_server_enabled(&config_path, "missing"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_failure_classifier_detects_usage_and_credit_limits() {
        let usage = classify_codex_exec_failure("You've hit the usage limit. Try again later.");
        assert!(usage.contains("quota or usage capacity"));

        let openai_message =
            "You've hit your usage limit. To get more access now, send a request to your admin or try again at <time returned by Codex>.";
        let subscription_limit = classify_codex_exec_failure(&format!(
            r#"{{"type":"error","message":"{openai_message}"}}"#
        ));
        assert!(subscription_limit.contains("quota or usage capacity"));
        assert!(subscription_limit.contains("Codex/OpenAI reported:"));
        assert!(subscription_limit.contains(openai_message));

        let credits = classify_codex_exec_failure(
            r#"{"error":{"code":"insufficient_quota","message":"billing quota exceeded"}}"#,
        );
        assert!(credits.contains("quota or usage capacity"));
        assert!(credits.contains("Codex/OpenAI reported: billing quota exceeded"));
    }

    #[test]
    fn codex_usage_retry_hint_extracts_retry_after_variants() {
        assert_eq!(
            codex_usage_retry_hint("rate_limit: retry after 42 seconds"),
            Some("retry after 42 seconds".to_string())
        );
        assert_eq!(
            codex_usage_retry_hint("Retry-After: 120"),
            Some("retry after 120".to_string())
        );
        assert_eq!(codex_usage_retry_hint("usage limit"), None);
    }

    #[test]
    fn codex_usage_reported_message_preserves_openai_diagnostic() {
        let diagnostic = "You've hit your usage limit. To get more access now, send a request to your admin or try again at <time returned by Codex>.";

        assert_eq!(
            codex_usage_reported_message(diagnostic),
            Some(diagnostic.to_string())
        );
        assert_eq!(
            codex_usage_reported_message(&format!(
                r#"{{"type":"turn.failed","error":{{"message":"{diagnostic}"}}}}"#
            )),
            Some(diagnostic.to_string())
        );
    }

    #[test]
    fn codex_failure_classifier_detects_auth_failures() {
        let auth = classify_codex_exec_failure("401 unauthorized: invalid API key");

        assert!(auth.contains("authentication failed"));
    }

    #[test]
    fn codex_failure_classifier_detects_unloaded_project_profile() {
        let profile = classify_codex_exec_failure("Error: config profile `agentactr` not found");

        assert!(profile.contains("repo-local project defaults"));
        assert!(profile.contains("stale agentactr binary"));
    }

    #[test]
    fn codex_project_trust_renders_quoted_absolute_project_path() {
        let project = Path::new("/tmp/agentactr.example/repo");

        let rendered = render_codex_project_trust("", project).unwrap();
        let parsed = parse_toml_document(&rendered).unwrap();

        assert_eq!(
            parsed["projects"]["/tmp/agentactr.example/repo"]["trust_level"].as_str(),
            Some("trusted")
        );
        assert!(rendered.contains("[projects.\"/tmp/agentactr.example/repo\"]"));
    }

    #[test]
    fn codex_probe_output_detects_error_events() {
        assert!(codex_probe_output_has_error_event(
            r#"{"type":"turn.failed","error":{"message":"rate limit"}}"#
        ));
        assert!(codex_probe_output_has_error_event(
            r#"{"event":"error","message":"failed"}"#
        ));
        assert!(!codex_probe_output_has_error_event(
            r#"{"type":"turn.completed"}"#
        ));
    }

    #[test]
    fn toml_config_merges_repository_policy() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let parsed = parse_toml_document(
            r#"
[repository]
empty_repo_policy = "custom_empty_policy"
declared_primary_stack = "rust"
allowed_bootstrap = "custom_bootstrap"
bootstrap_prereqs = "custom_prereqs"
fail_on_low_confidence_stack_detection = false
"#,
        )
        .unwrap();

        merge_config_from_toml(&mut config, &parsed).unwrap();

        assert_eq!(config.repository.empty_repo_policy, "custom_empty_policy");
        assert_eq!(config.repository.declared_primary_stack, "rust");
        assert_eq!(config.repository.allowed_bootstrap, "custom_bootstrap");
        assert_eq!(config.repository.bootstrap_prereqs, "custom_prereqs");
        assert!(!config.repository.fail_on_low_confidence_stack_detection);
    }

    #[test]
    fn toml_config_merges_linux_memory_policy() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let parsed = parse_toml_document(
            r#"
[linux_memory]
enabled = false
cgroup_root = "/tmp/agentactr-cgroup-test"
root_group = "agentactr-test"
mode = "observe_only"
cgroup_v2_required = false
psi_required = false
per_issue_memory_high = "1G"
per_issue_memory_max = "2G"
per_agent_memory_high = "512M"
per_agent_memory_max = "1G"
psi_memory_some_threshold_us = 10
psi_memory_window_us = 20
oom_score_adj = 123
setrlimit_address_space = "6G"
setrlimit_file_size = "1G"
kill_policy = "cancel_lowest_priority_subagent"
oom_policy = "observe"
"#,
        )
        .unwrap();

        merge_config_from_toml(&mut config, &parsed).unwrap();

        assert!(!config.linux_memory.enabled);
        assert_eq!(
            config.linux_memory.cgroup_root,
            "/tmp/agentactr-cgroup-test"
        );
        assert_eq!(config.linux_memory.root_group, "agentactr-test");
        assert_eq!(config.linux_memory.mode, "observe_only");
        assert!(!config.linux_memory.cgroup_v2_required);
        assert!(!config.linux_memory.psi_required);
        assert_eq!(config.linux_memory.per_issue_memory_high, "1G");
        assert_eq!(config.linux_memory.per_issue_memory_max, "2G");
        assert_eq!(config.linux_memory.per_agent_memory_high, "512M");
        assert_eq!(config.linux_memory.per_agent_memory_max, "1G");
        assert_eq!(config.linux_memory.psi_memory_some_threshold_us, 10);
        assert_eq!(config.linux_memory.psi_memory_window_us, 20);
        assert_eq!(config.linux_memory.oom_score_adj, 123);
        assert_eq!(config.linux_memory.setrlimit_address_space, "6G");
        assert_eq!(config.linux_memory.setrlimit_file_size, "1G");
        assert_eq!(
            config.linux_memory.kill_policy,
            "cancel_lowest_priority_subagent"
        );
        assert_eq!(config.linux_memory.oom_policy, "observe");
    }

    #[test]
    fn toml_config_accepts_spec_linux_memory_schema() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let parsed = parse_toml_document(
            r#"
[linux_memory]
enabled = true
cgroup_root = "auto"
root_group = "agentactr"
per_issue_memory_high = "5G"
per_issue_memory_max = "7G"
oom_score_adj = 300
kill_policy = "cancel_lowest_priority_subagent"
"#,
        )
        .unwrap();

        merge_config_from_toml(&mut config, &parsed).unwrap();

        assert!(config.linux_memory.enabled);
        assert_eq!(config.linux_memory.cgroup_root, "auto");
        assert_eq!(config.linux_memory.root_group, "agentactr");
        assert_eq!(config.linux_memory.per_issue_memory_high, "5G");
        assert_eq!(config.linux_memory.per_issue_memory_max, "7G");
        assert_eq!(config.linux_memory.oom_score_adj, 300);
        assert_eq!(
            config.linux_memory.kill_policy,
            "cancel_lowest_priority_subagent"
        );
        assert_eq!(
            config.linux_memory.oom_policy,
            "cancel_lowest_priority_subagent"
        );
    }

    #[test]
    fn toml_config_accepts_execution_docker_backend_schema() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let parsed = parse_toml_document(
            r#"
[execution]
backend = "docker_linux_vm"
strict_memory_required = true

[execution.docker]
command = "docker"
image = "ghcr.io/example/agentactr-runtime:sha"
pull_policy = "never"
network = "none"
workspace_mount = "rw"
artifact_mount = "rw"
remove_containers = false
container_prefix = "agentactr-test"
"#,
        )
        .unwrap();

        merge_config_from_toml(&mut config, &parsed).unwrap();

        assert_eq!(config.execution.backend, "docker_linux_vm");
        assert!(config.execution.strict_memory_required);
        assert_eq!(config.execution.docker.command, "docker");
        assert_eq!(
            config.execution.docker.image,
            "ghcr.io/example/agentactr-runtime:sha"
        );
        assert_eq!(config.execution.docker.pull_policy, "never");
        assert_eq!(config.execution.docker.network, "none");
        assert_eq!(config.execution.docker.workspace_mount, "rw");
        assert_eq!(config.execution.docker.artifact_mount, "rw");
        assert!(!config.execution.docker.remove_containers);
        assert_eq!(config.execution.docker.container_prefix, "agentactr-test");
    }

    #[test]
    fn local_sqlite_lease_blocks_second_active_run_for_issue() {
        let root = env::temp_dir().join(format!(
            "agentactr-local-lease-test-{}-{}",
            std::process::id(),
            new_run_id("lease")
        ));
        fs::create_dir_all(&root).unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.sqlite = root.join("runs.sqlite").display().to_string();
        config.observability.jsonl = root.join("events.jsonl").display().to_string();

        record_run_state(&config, "run-1", "OWNER/REPO", "42", "started", &root).unwrap();
        let err =
            record_run_state(&config, "run-2", "OWNER/REPO", "42", "started", &root).unwrap_err();

        assert!(err.contains("local SQLite lease already active"));
        assert!(err.contains("only coordinates local dispatch"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refresh_local_lease_extends_active_expiry() {
        let root = env::temp_dir().join(format!(
            "agentactr-local-lease-refresh-test-{}-{}",
            std::process::id(),
            new_run_id("lease-refresh")
        ));
        fs::create_dir_all(&root).unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.scheduling.lease_ttl_ms = 60_000;
        config.observability.sqlite = root.join("runs.sqlite").display().to_string();
        config.observability.jsonl = root.join("events.jsonl").display().to_string();

        record_run_state(&config, "run-1", "OWNER/REPO", "42", "started", &root).unwrap();
        let first_expiry = local_lease_expiry_ms(&config, "OWNER/REPO", "42");
        thread::sleep(Duration::from_millis(20));
        refresh_local_lease(&config, "run-1", "OWNER/REPO", "42").unwrap();
        let refreshed_expiry = local_lease_expiry_ms(&config, "OWNER/REPO", "42");

        assert!(
            refreshed_expiry > first_expiry,
            "expected refreshed lease expiry {refreshed_expiry} to exceed original {first_expiry}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn issue_submission_begin_transitions_pending_with_sdk_cas_contract() {
        let root = env::temp_dir().join(format!(
            "agentactr-issue-ledger-cas-test-{}-{}",
            std::process::id(),
            new_run_id("issue-ledger")
        ));
        fs::create_dir_all(&root).unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.sqlite = root.join("runs.sqlite").display().to_string();
        let proposal = IssueProposal {
            proposal_id: IssueProposalId::new("proposal-1"),
            repo: "OWNER/REPO".to_string(),
            parent_issue: Some(42),
            title: "child issue".to_string(),
            body: "body".to_string(),
            labels: Vec::new(),
            assignees: Vec::new(),
            milestone: None,
            issue_type: None,
            issue_field_values: Vec::new(),
            project_fields: Vec::new(),
            digest: "digest-1".to_string(),
            dedupe: agentactr_sdk::IssueDedupeStatus::Unique,
            framework: None,
            related_issues: Vec::new(),
            provenance: Vec::new(),
        };
        let key = agentactr_sdk::issue_submission_key("run-1", &proposal);
        with_issue_ledger_pool(&config, |runtime, pool| {
            runtime.block_on(async {
                ensure_issue_submission_ledger_table(&pool).await?;
                sqlx_core::query::query(
                    r#"INSERT INTO issue_submission_ledger
                       (run_id, proposal_id, repo, parent_issue, proposal_digest, state, detail)
                       VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 'pending review')"#,
                )
                .bind(&key.run_id)
                .bind(key.proposal_id.as_str())
                .bind(&key.repo)
                .bind(ledger_parent_issue_value(key.parent_issue))
                .bind(&key.proposal_digest)
                .execute(&pool)
                .await
                .map_err(|e| format!("insert pending issue submission ledger: {e}"))?;
                Ok(())
            })
        })
        .unwrap();

        begin_issue_submission(&config, "run-1", &proposal, None).unwrap();
        let entry = load_issue_submission_ledger(&config, "run-1", &proposal)
            .unwrap()
            .unwrap();

        assert_eq!(entry.state, IssueSubmissionLedgerState::Submitted);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn issue_set_manifest_records_framework_and_planner_paths() {
        let root = env::temp_dir().join(format!(
            "agentactr-issue-set-manifest-test-{}-{}",
            std::process::id(),
            new_run_id("issue-set")
        ));
        fs::create_dir_all(&root).unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();
        let framework = agentactr_sdk::FrameworkDeclaration {
            ecosystem: "typescript".to_string(),
            id: "nextjs".to_string(),
            version_or_profile: Some("app-router".to_string()),
        };
        let mut context = create_issue_set_context(
            &config,
            "draft-test",
            "OWNER/REPO",
            Some(42),
            Some(framework),
            IssueSetSource::Draft,
        )
        .unwrap();
        context.planner_prompt_path =
            Some(context.artifact_dir.join("planner_prompt.redacted.txt"));
        context.planner_metadata_path = Some(context.artifact_dir.join("planner_metadata.json"));

        write_issue_set_manifest(&context, &config, Some("typescript")).unwrap();
        let manifest = serde_json::from_str::<serde_json::Value>(
            &fs::read_to_string(&context.manifest_path).unwrap(),
        )
        .unwrap();

        assert_eq!(manifest["framework"]["ecosystem"], "typescript");
        assert_eq!(manifest["framework"]["id"], "nextjs");
        assert_eq!(manifest["framework"]["version_or_profile"], "app-router");
        assert_eq!(
            manifest["planner_prompt_path"],
            context
                .artifact_dir
                .join("planner_prompt.redacted.txt")
                .display()
                .to_string()
        );
        assert_eq!(
            manifest["planner_metadata_path"],
            context
                .artifact_dir
                .join("planner_metadata.json")
                .display()
                .to_string()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_review_status_gates_required_issue_submission() {
        let root = env::temp_dir().join(format!(
            "agentactr-codex-review-gate-test-{}-{}",
            std::process::id(),
            new_run_id("review")
        ));
        fs::create_dir_all(&root).unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();
        let context = create_issue_set_context(
            &config,
            "draft-review-test",
            "OWNER/REPO",
            None,
            None,
            IssueSetSource::Draft,
        )
        .unwrap();

        let missing = require_codex_review_for_proposal(&context, "proposal-1").unwrap_err();
        assert!(missing.contains("--codex-review"));

        write_file(
            codex_issue_review_status_path(&context),
            r#"{"status":"approved","reviewed_proposal_ids":["proposal-1"]}"#,
        )
        .unwrap();
        require_codex_review_for_proposal(&context, "proposal-1").unwrap();
        let uncovered = require_codex_review_for_proposal(&context, "proposal-2").unwrap_err();
        assert!(uncovered.contains("was not covered"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn clap_help_includes_codex_review_issue_flags() {
        let issue_draft =
            render_generated_help(&["issue".to_string(), "draft".to_string()]).unwrap();
        assert!(issue_draft.contains("--codex-draft"));
        assert!(issue_draft.contains("--codex-review"));
        let issue_submit =
            render_generated_help(&["issue".to_string(), "submit".to_string()]).unwrap();
        assert!(issue_submit.contains("--require-codex-review"));
    }

    fn local_lease_expiry_ms(config: &AgentactrConfig, repo: &str, issue: &str) -> i64 {
        use sqlx_core::row::Row;
        use sqlx_sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        use std::str::FromStr;

        let sqlite_path = Path::new(&config.observability.sqlite);
        let url = format!("sqlite://{}", sqlite_path.display());
        let options = SqliteConnectOptions::from_str(&url).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .unwrap();
            let row = sqlx_core::query::query(
                "SELECT expires_at_ms FROM local_leases WHERE repo = ?1 AND issue = ?2",
            )
            .bind(repo)
            .bind(issue)
            .fetch_one(&pool)
            .await
            .unwrap();
            row.try_get("expires_at_ms").unwrap()
        })
    }

    #[test]
    fn toml_config_merges_rendered_policy_sections() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let parsed = parse_toml_document(
            r#"
[tracker]
active_labels = ["ready"]
ignore_labels = ["blocked"]
claim_label = "claimed"
running_label = "running"
failed_label = "failed"
done_label = "done"

[codex]
app_server_transport = "stdio"
app_server_experimental_api = true
sdk_bridge = "typescript"
fallback_mode = "cli_json"

[human_intervention]
on_codex_approval_request = "queue_review"
run_start_banner = false

[mcp]
default_policy = "manual"
remote_github_write_tools = "enabled_for_test"

[quality]
pre_commit_mode = "optional"
technology_detection = "manual"
fail_on_missing_toolchain = false
artifact_dir = ".custom/quality"
dependency_checks = false
architecture_checks = false
tool_pinning = "advisory"

[commit]
mode = "disabled"
signoff = true
required_trailers = ["Run"]

[merge]
mode = "manual"
push = "disabled"
require_clean_rebase = false

[workspace]
root = ".custom/workspaces"
keep_successful = false

[scheduling]
poll_interval_ms = 1
max_concurrent_issue_runs = 2
lease_ttl_ms = 3
max_retries = 4

[spawn]
enabled = false
max_child_agents_per_issue = 1
allow_parallel_read_only = false
strategy = "custom_one_writer"
max_total_uncached_input_tokens = 123
max_child_uncached_input_tokens = 45
max_child_output_tokens = 67
artifact_handoff = "custom_refs"
pause_on_memory_pressure = false

[observability]
otel_enabled = true
otel_endpoint = "http://127.0.0.1:4317"
debug_bundle_root = ".custom/debug"
redact_secrets = false
"#,
        )
        .unwrap();

        merge_config_from_toml(&mut config, &parsed).unwrap();

        assert_eq!(config.tracker.active_labels, vec!["ready"]);
        assert_eq!(config.tracker.claim_label, "claimed");
        assert_eq!(config.codex.app_server_transport, "stdio");
        assert!(config.codex.app_server_experimental_api);
        assert_eq!(config.codex.sdk_bridge, "typescript");
        assert_eq!(config.codex.fallback_mode, "cli_json");
        assert_eq!(
            config.human_intervention.on_codex_approval_request,
            "queue_review"
        );
        assert!(!config.human_intervention.run_start_banner);
        assert_eq!(config.mcp.default_policy, "manual");
        assert_eq!(config.mcp.remote_github_write_tools, "enabled_for_test");
        assert_eq!(config.quality.pre_commit_mode, "optional");
        assert!(!config.quality.fail_on_missing_toolchain);
        assert_eq!(config.commit.mode, "disabled");
        assert!(config.commit.signoff);
        assert_eq!(config.commit.required_trailers, vec!["Run"]);
        assert_eq!(config.merge.mode, "manual");
        assert!(!config.merge.require_clean_rebase);
        assert_eq!(config.workspace.root, ".custom/workspaces");
        assert!(!config.workspace.keep_successful);
        assert_eq!(config.scheduling.lease_ttl_ms, 3);
        assert!(!config.spawn.enabled);
        assert_eq!(config.spawn.strategy, "custom_one_writer");
        assert_eq!(config.spawn.max_total_uncached_input_tokens, 123);
        assert_eq!(config.spawn.max_child_uncached_input_tokens, 45);
        assert_eq!(config.spawn.max_child_output_tokens, 67);
        assert_eq!(config.spawn.artifact_handoff, "custom_refs");
        assert!(!config.spawn.pause_on_memory_pressure);
        assert!(config.observability.otel_enabled);
        assert!(!config.observability.redact_secrets);
    }

    #[test]
    fn checked_in_root_config_exposes_current_operator_surface() {
        let parsed = parse_toml_document(include_str!("../../../agentactr.toml")).unwrap();

        assert_eq!(
            toml_path(&parsed, "spawn.strategy")
                .and_then(toml::Value::as_str)
                .unwrap(),
            "budget_aware_one_writer"
        );
        assert_eq!(
            toml_path(&parsed, "spawn.artifact_handoff")
                .and_then(toml::Value::as_str)
                .unwrap(),
            "refs_summaries_and_digests"
        );
        assert_eq!(
            toml_path(&parsed, "execution.backend")
                .and_then(toml::Value::as_str)
                .unwrap(),
            "auto"
        );
        assert_eq!(
            toml_path(&parsed, "execution.docker.container_prefix")
                .and_then(toml::Value::as_str)
                .unwrap(),
            "agentactr"
        );
        assert_eq!(
            toml_path(&parsed, "linux_memory.mode")
                .and_then(toml::Value::as_str)
                .unwrap(),
            "enforce_on_linux_observe_elsewhere"
        );
        assert_eq!(
            toml_path(&parsed, "linux_memory.oom_policy")
                .and_then(toml::Value::as_str)
                .unwrap(),
            "fail_run_preserve_debug_bundle"
        );
    }

    #[test]
    fn docker_release_metadata_is_digest_pinned_and_apache_licensed() {
        let runtime = include_str!("../../../docker/agentactr-runtime/Dockerfile");
        let static_cli = include_str!("../../../docker/agentactr-cli-static/Dockerfile");

        for dockerfile in [runtime, static_cli] {
            assert!(!dockerfile.contains("UNLICENSED"));
            assert!(dockerfile.contains(r#"org.opencontainers.image.licenses="Apache-2.0""#));
            for line in dockerfile.lines().filter(|line| line.starts_with("FROM ")) {
                let image = line
                    .split_whitespace()
                    .skip(1)
                    .find(|part| !part.starts_with("--"))
                    .unwrap_or_default();
                if !image.contains('/') && !image.contains(':') {
                    continue;
                }
                assert!(
                    line.contains("@sha256:"),
                    "Docker base image is not digest pinned: {line}"
                );
            }
        }
    }

    #[test]
    fn trusted_docker_image_workflows_use_depot_builds() {
        let static_cli = include_str!("../../../docker/agentactr-cli-static/Dockerfile");
        let release = include_str!("../../../.github/workflows/release.yml");
        let nightly = include_str!("../../../.github/workflows/nightly.yml");

        assert!(!static_cli.contains("FROM --platform=$BUILDPLATFORM rust:"));
        assert!(!static_cli.contains("FROM --platform=$TARGETPLATFORM rust:"));
        for workflow in [release, nightly] {
            assert!(workflow.contains("depot/setup-action@v1"));
            assert!(workflow.contains("depot/build-push-action@v1"));
            assert!(workflow.contains("project: ${{ vars.DEPOT_PROJECT_ID }}"));
            assert!(workflow.contains("token: ${{ secrets.DEPOT_TOKEN }}"));
            assert!(!workflow.contains("docker/setup-buildx-action@"));
            assert!(!workflow.contains("scripts/build-agentactr-runtime.sh"));
            assert!(!workflow.contains("scripts/build-agentactr-cli-static.sh"));
        }
        assert!(!release.contains("docker/setup-qemu-action@"));
        assert!(nightly.contains("docker/setup-qemu-action@"));
    }

    #[test]
    fn architecture_boundary_artifacts_are_current_with_specs_and_agents() {
        let script = include_str!("../../../scripts/check-architecture-boundaries.sh");
        let workflow = include_str!("../../../.github/workflows/architecture.yml");
        let svg = include_str!("../../../internal_readme/architecture.svg");
        let readme = include_str!("../../../README.md");
        let agents = include_str!("../../../AGENTS.md");
        let spec = include_str!("../../../specs_agentactrSDK.md");
        let pre_commit = include_str!("../../../.pre-commit-config.yaml");

        for needle in [
            "agentactr-core must not import concrete adapters",
            "agentactr-sdk must expose ports, use cases, config, renderers, and planners",
            "pub struct CodexRuntimeAdapter",
            "pub fn docker_command",
            "struct GithubRestAdapter",
            "struct LinuxMemoryController",
            "internal_specs_agentactrSDK/svgs/sdk_cli_boundary\\.svg",
            "internal_readme/architecture\\.svg",
            "scripts/check-architecture-boundaries\\.sh",
        ] {
            assert!(
                script.contains(needle),
                "boundary script missing `{needle}`"
            );
        }

        assert!(workflow.contains("run: scripts/check-architecture-boundaries.sh"));
        assert!(
            !workflow.contains("GithubRestAdapter") && !workflow.contains("CodexRuntimeAdapter"),
            "architecture workflow must delegate policy checks to the shared script"
        );
        assert!(pre_commit.contains("id: architecture-boundaries"));
        assert!(pre_commit.contains("entry: scripts/check-architecture-boundaries.sh"));

        for needle in [
            "agentactr-core",
            "agentactr-sdk",
            "agentactr-codex",
            "agentactr-execution",
            "agentactr CLI",
            "CLI-local adapters",
            "Linux memory",
            "domain graph",
            "issue planners",
            "run-scoped CODEX_HOME",
            "Docker command wrapper",
            "provider-neutral process spec",
        ] {
            assert!(svg.contains(needle), "architecture.svg missing `{needle}`");
        }
        assert!(
            !svg.contains("Docker backend wrapper"),
            "Docker backend wrapping belongs to agentactr-execution"
        );

        assert!(
            readme.contains("![Present repository architecture](internal_readme/architecture.svg)")
        );
        for crate_name in [
            "`agentactr-core`",
            "`agentactr-sdk`",
            "`agentactr-codex`",
            "`agentactr-execution`",
            "`agentactr-cli`",
        ] {
            assert!(readme.contains(crate_name), "README missing `{crate_name}`");
        }

        for needle in [
            "specs_agentactrSDK.md",
            "README.md",
            "internal_readme/",
            "internal_specs_agentactrSDK/svgs/",
            "Hexagonal/Clean Architecture",
            "Dependency Inversion",
            "Configuration-driven composition",
        ] {
            assert!(agents.contains(needle), "AGENTS.md missing `{needle}`");
        }

        for needle in [
            "internal_specs_agentactrSDK/svgs/sdk_cli_boundary.svg",
            "agentactr-execution",
            "Domain Graph and Platform Profiles",
            "quality.domains",
            "templates.agents_policy",
            "github.standard_label_policy",
            "linux_memory.setrlimit_address_space",
            "execution.backend",
        ] {
            assert!(spec.contains(needle), "spec missing `{needle}`");
        }
    }

    #[test]
    fn github_token_preflight_prefers_configured_env_and_deduplicates_generic_names() {
        assert_eq!(
            preferred_github_token_env_names("AGENTACTR_GITHUB_APP_TOKEN"),
            vec![
                "AGENTACTR_GITHUB_APP_TOKEN".to_string(),
                "GITHUB_TOKEN".to_string(),
                "GH_TOKEN".to_string()
            ]
        );
        assert_eq!(
            preferred_github_token_env_names("GITHUB_TOKEN"),
            vec!["GITHUB_TOKEN".to_string(), "GH_TOKEN".to_string()]
        );
    }

    #[test]
    fn codex_milestone_env_overrides_fail_closed_on_invalid_bool() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_transport = env::var("AGENTACTR_CODEX_APP_SERVER_TRANSPORT").ok();
        let previous_experimental = env::var("AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API").ok();
        let previous_bridge = env::var("AGENTACTR_CODEX_SDK_BRIDGE").ok();
        let previous_fallback = env::var("AGENTACTR_CODEX_FALLBACK_MODE").ok();

        env::set_var("AGENTACTR_CODEX_APP_SERVER_TRANSPORT", "stdio");
        env::set_var("AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API", "maybe");
        env::set_var("AGENTACTR_CODEX_SDK_BRIDGE", "typescript");
        env::set_var("AGENTACTR_CODEX_FALLBACK_MODE", "cli_json");

        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let err = merge_config_from_env(&mut config).unwrap_err();

        restore_env("AGENTACTR_CODEX_APP_SERVER_TRANSPORT", previous_transport);
        restore_env(
            "AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API",
            previous_experimental,
        );
        restore_env("AGENTACTR_CODEX_SDK_BRIDGE", previous_bridge);
        restore_env("AGENTACTR_CODEX_FALLBACK_MODE", previous_fallback);

        assert!(err.contains("AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API"));
        assert!(err.contains("boolean-like"));
    }

    #[test]
    fn codex_milestone_env_overrides_fail_closed_on_invalid_policy_value() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_transport = env::var("AGENTACTR_CODEX_APP_SERVER_TRANSPORT").ok();
        let previous_experimental = env::var("AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API").ok();
        let previous_bridge = env::var("AGENTACTR_CODEX_SDK_BRIDGE").ok();
        let previous_fallback = env::var("AGENTACTR_CODEX_FALLBACK_MODE").ok();

        env::set_var("AGENTACTR_CODEX_APP_SERVER_TRANSPORT", "websocket");
        env::set_var("AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API", "false");
        env::set_var("AGENTACTR_CODEX_SDK_BRIDGE", "typescript");
        env::set_var("AGENTACTR_CODEX_FALLBACK_MODE", "cli_json");

        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        merge_config_from_env(&mut config).unwrap();
        let err = config.codex.validate_milestone_policy().unwrap_err();

        restore_env("AGENTACTR_CODEX_APP_SERVER_TRANSPORT", previous_transport);
        restore_env(
            "AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API",
            previous_experimental,
        );
        restore_env("AGENTACTR_CODEX_SDK_BRIDGE", previous_bridge);
        restore_env("AGENTACTR_CODEX_FALLBACK_MODE", previous_fallback);

        assert!(err.contains("app_server_transport=websocket"));
        assert!(err.contains("app_server_experimental_api=true"));
    }

    #[test]
    fn codex_milestone_env_overrides_fail_closed_on_alias_value() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_transport = env::var("AGENTACTR_CODEX_APP_SERVER_TRANSPORT").ok();
        let previous_experimental = env::var("AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API").ok();
        let previous_bridge = env::var("AGENTACTR_CODEX_SDK_BRIDGE").ok();
        let previous_fallback = env::var("AGENTACTR_CODEX_FALLBACK_MODE").ok();

        env::set_var("AGENTACTR_CODEX_APP_SERVER_TRANSPORT", "ws");
        env::set_var("AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API", "true");
        env::set_var("AGENTACTR_CODEX_SDK_BRIDGE", "ts");
        env::set_var("AGENTACTR_CODEX_FALLBACK_MODE", "exec-json");

        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        merge_config_from_env(&mut config).unwrap();
        let err = config.codex.validate_milestone_policy().unwrap_err();

        restore_env("AGENTACTR_CODEX_APP_SERVER_TRANSPORT", previous_transport);
        restore_env(
            "AGENTACTR_CODEX_APP_SERVER_EXPERIMENTAL_API",
            previous_experimental,
        );
        restore_env("AGENTACTR_CODEX_SDK_BRIDGE", previous_bridge);
        restore_env("AGENTACTR_CODEX_FALLBACK_MODE", previous_fallback);

        assert!(err.contains("codex.app_server_transport"));
        assert!(err.contains("canonical stored value `websocket`"));
    }

    #[test]
    fn doctor_adapter_version_reports_expose_structured_degradation() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");

        let reports = configured_adapter_version_reports(&config).unwrap();

        assert_eq!(reports.len(), 3);
        for report in &reports {
            assert!(
                !report.degraded_features.is_empty(),
                "{} report should expose degraded features",
                report.adapter_kind
            );
            assert!(
                !report.required_actions.is_empty(),
                "{} report should expose required actions",
                report.adapter_kind
            );
        }

        let runtime = reports
            .iter()
            .find(|report| report.adapter_kind == "agent_runtime")
            .unwrap();
        assert_eq!(runtime.api_version, "codex-exec-json");
        assert!(runtime
            .degraded_features
            .contains(&"cancellation".to_string()));
        assert!(runtime
            .required_actions
            .iter()
            .any(|action| action.contains("contract tests")));
    }

    #[test]
    fn adapter_version_reports_are_artifacted_and_traced() {
        let root = env::temp_dir().join(format!(
            "agentactr-adapter-report-test-{}-{}",
            std::process::id(),
            new_run_id("adapter-report")
        ));
        fs::create_dir_all(&root).unwrap();
        let artifact_dir = root.join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.jsonl = root.join("events.jsonl").display().to_string();
        config.observability.artifact_root = artifact_dir.display().to_string();
        let reports = configured_adapter_version_reports(&config).unwrap();

        let path = record_adapter_version_reports(
            &config,
            "run-1",
            "OWNER/REPO",
            "42",
            &artifact_dir,
            &reports,
        )
        .unwrap();

        let artifact = fs::read_to_string(path).unwrap();
        let parsed = serde_json::from_str::<serde_json::Value>(&artifact).unwrap();
        assert_eq!(parsed["schema_version"], "0.1");
        assert_eq!(parsed["reports"].as_array().unwrap().len(), 3);
        let runtime = parsed["reports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|report| report["adapter_kind"] == "agent_runtime")
            .unwrap();
        assert_eq!(runtime["api_version"], "codex-exec-json");
        assert!(runtime["degraded_features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|feature| feature == "cancellation"));
        assert!(!runtime["required_actions"].as_array().unwrap().is_empty());

        let events = fs::read_to_string(root.join("events.jsonl")).unwrap();
        let event = serde_json::from_str::<serde_json::Value>(events.trim()).unwrap();
        assert_eq!(event["event_type"], "adapter.version_reported");
        assert_eq!(event["payload"]["reports"].as_array().unwrap().len(), 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn docker_backend_skips_host_codex_preflight() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.execution.backend = "docker_linux_vm".to_string();

        let decision = resolve_execution_backend(&config.execution).unwrap();

        assert_eq!(decision.effective, ExecutionBackend::DockerLinuxVm);
        assert!(!should_probe_host_codex(&decision));
    }

    #[test]
    fn docker_codex_project_config_preflight_does_not_require_host_trust() {
        let root = env::temp_dir().join(format!(
            "agentactr-docker-codex-config-test-{}-{}",
            std::process::id(),
            new_run_id("docker-codex")
        ));
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(
            root.join(".codex/config.toml"),
            r#"approval_policy = "never"
sandbox_mode = "workspace-write"

[mcp_servers.agentactr]
command = "agentactr"
args = ["mcp"]
"#,
        )
        .unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.execution.backend = "docker_linux_vm".to_string();
        let decision = resolve_execution_backend(&config.execution).unwrap();

        require_codex_project_config_ready(&root, "agentactr", &decision).unwrap();

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_backend_keeps_host_codex_preflight() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.execution.backend = "native_linux_cgroup_v2".to_string();

        let decision = resolve_execution_backend(&config.execution).unwrap();

        assert_eq!(decision.effective, ExecutionBackend::NativeLinuxCgroupV2);
        assert!(should_probe_host_codex(&decision));
    }

    #[test]
    fn toml_codex_milestone_policy_rejects_alias_values() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let parsed = parse_toml_document(
            r#"
[codex]
mode = "exec-json"
app_server_transport = "stdio"
sdk_bridge = "typescript"
fallback_mode = "cli_json"
"#,
        )
        .unwrap();

        merge_config_from_toml(&mut config, &parsed).unwrap();
        let err = config.codex.validate_milestone_policy().unwrap_err();

        assert!(err.contains("codex.mode"));
        assert!(err.contains("canonical stored value `cli_json`"));
    }

    #[test]
    fn run_policy_uses_configured_github_finalization() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.github.finalization = "require_human_review".to_string();

        let policy = RunPolicy::from_config_and_args(&config, &[]).unwrap();

        assert_eq!(
            policy.github_finalization,
            GithubFinalizationSetting::RequireHumanReview
        );
        assert_eq!(
            policy.github_finalization_text(),
            "require human review before terminal finalization/close"
        );
    }

    #[test]
    fn run_policy_effective_finalization_honors_tracker_capabilities() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.github.finalization = "require_human_review".to_string();
        let policy = RunPolicy::from_config_and_args(&config, &[]).unwrap();

        assert_eq!(
            policy.github_finalization_effective_text(&GithubRestAdapter::bootstrap_capabilities()),
            "require human review before terminal finalization/close"
        );

        let supported = AdapterCapabilities {
            adapter_kind: "issue_tracker".to_string(),
            supported_features: vec![
                "issue_read".to_string(),
                "claim_mutation".to_string(),
                "comment_create".to_string(),
                "label_set".to_string(),
            ],
            degraded_features: Vec::new(),
            required_actions: Vec::new(),
        };
        assert_eq!(
            policy.github_finalization_effective_text(&supported),
            "require human review before terminal finalization/close"
        );
    }

    #[test]
    fn run_issue_args_reject_unknown_flags_before_side_effects() {
        let args = vec![
            "run".to_string(),
            "issue".to_string(),
            "--repo".to_string(),
            "OWNER/REPO".to_string(),
            "--issue".to_string(),
            "123".to_string(),
            "--dryrun".to_string(),
        ];

        let err = validate_run_issue_args(&args).unwrap_err();

        assert!(err.contains("unknown agentactr run issue flag `--dryrun`"));
    }

    #[test]
    fn run_issue_args_reject_typoed_value_flags_and_stray_values() {
        let args = vec![
            "run".to_string(),
            "issue".to_string(),
            "--repo".to_string(),
            "OWNER/REPO".to_string(),
            "--issue".to_string(),
            "123".to_string(),
            "--github-finalizaton".to_string(),
            "disabled".to_string(),
        ];

        let err = validate_run_issue_args(&args).unwrap_err();

        assert!(err.contains("unknown agentactr run issue flag `--github-finalizaton`"));
    }

    #[test]
    fn run_issue_args_accept_known_flags() {
        let args = vec![
            "run".to_string(),
            "issue".to_string(),
            "--repo".to_string(),
            "OWNER/REPO".to_string(),
            "--issue".to_string(),
            "123".to_string(),
            "--human-intervention".to_string(),
            "interactive".to_string(),
            "--codex-approval".to_string(),
            "on-request".to_string(),
            "--github-finalization".to_string(),
            "disabled".to_string(),
            "--dry-run".to_string(),
        ];

        validate_run_issue_args(&args).unwrap();
    }

    #[test]
    fn run_issue_args_reject_missing_values() {
        let args = vec![
            "run".to_string(),
            "issue".to_string(),
            "--repo".to_string(),
            "OWNER/REPO".to_string(),
            "--issue".to_string(),
        ];

        let err = validate_run_issue_args(&args).unwrap_err();

        assert!(err.contains("--issue requires a value"));
    }

    #[test]
    fn docker_pull_policy_always_pulls_even_when_image_exists() {
        assert_eq!(
            docker_image_readiness_action("always", true).unwrap(),
            DockerImageReadinessAction::Pull
        );
        assert_eq!(
            docker_image_readiness_action("always", false).unwrap(),
            DockerImageReadinessAction::Pull
        );
    }

    #[test]
    fn docker_pull_policy_if_missing_reuses_existing_image() {
        assert_eq!(
            docker_image_readiness_action("if_missing", true).unwrap(),
            DockerImageReadinessAction::UseExisting
        );
        assert_eq!(
            docker_image_readiness_action("if_missing", false).unwrap(),
            DockerImageReadinessAction::Pull
        );
    }

    #[test]
    fn docker_pull_policy_never_fails_when_missing() {
        assert_eq!(
            docker_image_readiness_action("never", true).unwrap(),
            DockerImageReadinessAction::UseExisting
        );
        assert_eq!(
            docker_image_readiness_action("never", false).unwrap(),
            DockerImageReadinessAction::MissingForbidden
        );
    }

    #[test]
    fn github_api_version_support_accepts_current_public_versions() {
        assert!(github_api_version_support("2026-03-10").is_some());
        let legacy = github_api_version_support("2022-11-28").unwrap();
        assert_eq!(legacy.end_of_support, Some("March 10, 2028"));
        assert!(github_api_version_support("2021-01-01").is_none());
    }

    #[test]
    fn review_required_rejects_interactive_codex_approval() {
        let args = vec![
            "run".to_string(),
            "issue".to_string(),
            "--human-intervention".to_string(),
            "review-required".to_string(),
            "--codex-approval".to_string(),
            "on-request".to_string(),
        ];
        let err = RunPolicy::from_args(&args).unwrap_err();
        assert!(err.contains("requires --human-intervention interactive"));
    }

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

    #[test]
    fn append_run_event_writes_replayable_jsonl() {
        let root = env::temp_dir().join(format!(
            "agentactr-trace-event-test-{}-{}",
            std::process::id(),
            new_run_id("trace")
        ));
        fs::create_dir_all(&root).unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.jsonl = root.join("events.jsonl").display().to_string();

        append_run_event(
            &RunEventContext::agent(&config, "run-1", "OWNER/REPO", "42", "agent-run-1", None),
            "agent.started",
            serde_json::json!({"role": "Implementer"}),
        )
        .unwrap();

        let events = fs::read_to_string(root.join("events.jsonl")).unwrap();
        let parsed = serde_json::from_str::<serde_json::Value>(events.trim()).unwrap();
        assert_eq!(parsed["schema_version"], "0.1");
        assert!(parsed["ts"]
            .as_str()
            .is_some_and(|ts| { ts.contains('T') && ts.ends_with('Z') && !ts.contains("unix") }));
        assert!(parsed["ts_unix_ms"].is_number());
        assert_eq!(parsed["run_id"], "run-1");
        assert_eq!(parsed["issue_id"], "github:OWNER/REPO#42");
        assert_eq!(parsed["agent_run_id"], "agent-run-1");
        assert_eq!(parsed["event_type"], "agent.started");
        assert_eq!(parsed["payload"]["role"], "Implementer");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn iso_timestamp_from_epoch_millis_formats_utc_timestamp() {
        assert_eq!(
            iso_timestamp_from_epoch_millis(0),
            "1970-01-01T00:00:00.000Z"
        );
        assert_eq!(
            iso_timestamp_from_epoch_millis(1_704_067_200_123),
            "2024-01-01T00:00:00.123Z"
        );
    }

    #[test]
    fn help_marks_milestone_commands_separately() {
        let text = help_text();

        assert!(text.contains("Implemented bootstrap commands:"));
        assert!(text.contains("commands [--json]"));
        assert!(text.contains("completions bash|zsh|fish|powershell|elvish"));
        assert!(text.contains("quality run RUN_ID"));
        assert!(text.contains("vcs list [--json]"));
        assert!(text.contains("vcs show RUN_ID [--json]"));
        assert!(text.contains("vcs status RUN_ID"));
        assert!(text.contains("vcs diff RUN_ID [--output PATH]"));
        assert!(text.contains("merge plan RUN_ID [--json]"));
        assert!(text.contains("trace list"));
        assert!(text.contains("trace show RUN_ID"));
        assert!(text.contains("debug bundle RUN_ID"));
        assert!(text.contains("menu [--json]"));
        assert!(text.contains("docs cli-markdown [--output PATH]"));
        assert!(text.contains("issue mark ISSUE_SET_ID --proposal PROPOSAL_ID"));
        assert!(text.contains("Specified milestone commands:"));
        assert!(text.contains("daemon --config agentactr.toml"));
        assert!(text.contains(
            "run query --repo OWNER/REPO --label agentactr:ready --human-intervention fail-closed"
        ));
        assert!(text.contains("vcs commit RUN_ID"));
        assert!(text.contains("vcs cleanup RUN_ID"));
        assert!(text.contains("finalize RUN_ID --approve"));
        assert!(text.contains("finalize RUN_ID --reject --reason REASON"));
        assert!(
            !text.contains("quality run RUN_ID             # not implemented in this milestone")
        );
        assert!(
            !text.contains("vcs status RUN_ID              # not implemented in this milestone")
        );
        assert!(
            !text.contains("vcs list [--json]              # not implemented in this milestone")
        );
        assert!(
            !text.contains("vcs show RUN_ID [--json]       # not implemented in this milestone")
        );
        assert!(
            !text.contains("vcs diff RUN_ID                # not implemented in this milestone")
        );
        assert!(
            !text.contains("merge plan RUN_ID              # not implemented in this milestone")
        );
        assert!(
            !text.contains("trace list|show RUN_ID         # not implemented in this milestone")
        );
        assert!(
            !text.contains("debug bundle RUN_ID            # not implemented in this milestone")
        );
        assert!(
            !text.contains("commands [--json]              # not implemented in this milestone")
        );
        assert!(!text.contains(
            "completions bash|zsh|fish|powershell|elvish\n                                 # not implemented in this milestone"
        ));
        assert!(
            !text.contains("menu [--json]                  # not implemented in this milestone")
        );
        assert!(!text.contains(
            "docs cli-markdown [--output PATH]\n                                 # not implemented in this milestone"
        ));
        assert!(text.contains("auth codex --method chatgpt|subscription|api-key"));
    }

    #[test]
    fn issue_candidate_query_accepts_frozen_flags() {
        let args = vec![
            "issue".to_string(),
            "find".to_string(),
            "--repo".to_string(),
            "OWNER/REPO".to_string(),
            "--query".to_string(),
            "cache bug".to_string(),
            "--state".to_string(),
            "closed".to_string(),
            "--label".to_string(),
            "bug".to_string(),
            "--assignee".to_string(),
            "octocat".to_string(),
            "--author".to_string(),
            "hubot".to_string(),
            "--since".to_string(),
            "2026-01-02T03:04:05Z".to_string(),
            "--sort".to_string(),
            "comments".to_string(),
            "--direction".to_string(),
            "asc".to_string(),
            "--page".to_string(),
            "3".to_string(),
            "--per-page".to_string(),
            "25".to_string(),
            "--limit".to_string(),
            "75".to_string(),
            "--include-pull-requests".to_string(),
        ];

        let query = parse_candidate_query(&args, "OWNER/REPO").unwrap();

        assert_eq!(query.repo, "OWNER/REPO");
        assert_eq!(query.state, agentactr_sdk::CandidateState::Closed);
        assert_eq!(query.labels, vec!["bug".to_string()]);
        assert_eq!(query.assignee.as_deref(), Some("octocat"));
        assert_eq!(query.author.as_deref(), Some("hubot"));
        assert_eq!(query.since.as_deref(), Some("2026-01-02T03:04:05Z"));
        assert_eq!(query.text_query.as_deref(), Some("cache bug"));
        assert_eq!(query.sort, agentactr_sdk::CandidateSort::Comments);
        assert_eq!(query.direction, agentactr_sdk::SortDirection::Asc);
        assert_eq!(query.page, Some(3));
        assert_eq!(query.per_page, 25);
        assert_eq!(query.limit, 75);
        assert!(query.include_pull_requests);
    }

    #[test]
    fn clap_help_tree_renders_top_level_and_nested_commands() {
        let top = render_generated_help(&[]).unwrap();
        assert!(top.contains("Usage: agentactr [COMMAND]"));
        assert!(top.contains("Commands:"));
        assert!(top.contains("run"));
        assert!(top.contains("vcs"));
        assert!(top.contains("completions"));
        assert!(top.contains("docs"));

        let vcs_status = render_generated_help(&["vcs".to_string(), "status".to_string()]).unwrap();
        assert!(vcs_status.contains("Command: agentactr vcs status"));
        assert!(vcs_status.contains("Usage: status <RUN_ID>"));
        assert!(vcs_status.contains("Read recorded run worktree status and touched files"));

        let run_issue = render_generated_help(&["run".to_string(), "issue".to_string()]).unwrap();
        assert!(run_issue.contains("--human-intervention"));
        assert!(run_issue.contains("--github-finalization"));
        assert!(run_issue.contains("--dry-run"));
        let run_query = render_generated_help(&["run".to_string(), "query".to_string()]).unwrap();
        assert!(run_query.contains("--repo <OWNER/REPO>"));
        assert!(run_query.contains("--label <LABEL>"));
        assert!(
            run_query.contains("--human-intervention <fail-closed|interactive|review-required>")
        );
        let finalize = render_generated_help(&["finalize".to_string()]).unwrap();
        assert!(finalize.contains("Usage: finalize [OPTIONS] <RUN_ID>"));
        assert!(finalize.contains("--approve"));
        assert!(finalize.contains("--reject"));
        assert!(finalize.contains("--reason <REASON>"));

        let docs_cli_markdown =
            render_generated_help(&["docs".to_string(), "cli-markdown".to_string()]).unwrap();
        assert!(docs_cli_markdown.contains("Command: agentactr docs cli-markdown"));
        assert!(docs_cli_markdown.contains("Usage: cli-markdown [OPTIONS]"));
        assert!(docs_cli_markdown.contains("--output <PATH>"));

        let config_set = render_generated_help(&["config".to_string(), "set".to_string()]).unwrap();
        assert!(config_set.contains("linux_memory.oom_policy"));
        assert!(config_set.contains("linux_memory.setrlimit_address_space"));
        assert!(config_set.contains("observability.otel_endpoint"));
        assert!(config_set.contains("observability.debug_bundle_root"));

        assert_eq!(
            generated_help_path(&[
                "vcs".to_string(),
                "status".to_string(),
                "--help".to_string()
            ]),
            Some(vec!["vcs".to_string(), "status".to_string()])
        );
        assert_eq!(
            generated_help_path(&["help".to_string(), "merge".to_string(), "plan".to_string()]),
            Some(vec!["merge".to_string(), "plan".to_string()])
        );
        assert_eq!(
            generated_help_path(&["help".to_string(), "--help".to_string()]),
            Some(vec!["help".to_string()])
        );
        assert_eq!(
            generated_help_path(&[
                "help".to_string(),
                "run".to_string(),
                "issue".to_string(),
                "--help".to_string()
            ]),
            Some(vec!["run".to_string(), "issue".to_string()])
        );
    }

    #[test]
    fn completions_are_generated_from_clap_tree_without_runtime_state() {
        let bash = completion_script(parse_completion_shell("bash").unwrap()).unwrap();
        assert!(bash.contains("agentactr"));
        assert!(bash.contains("run"));
        assert!(bash.contains("vcs"));
        assert!(bash.contains("completions"));
        assert!(bash.contains("bash zsh fish powershell elvish"));
        assert!(bash.contains("compgen -W \"fail-closed interactive review-required\""));
        assert!(bash.contains(
            "compgen -W \"automatic_after_quality_gates require_human_review disabled\""
        ));
        assert!(bash.contains("codex.mode"));
        assert!(bash.contains("human_intervention.mode"));
        assert!(bash.contains("linux_memory.oom_policy"));
        assert!(bash.contains("linux_memory.setrlimit_address_space"));
        assert!(bash.contains("observability.otel_endpoint"));
        assert!(bash.contains("observability.debug_bundle_root"));

        let zsh = completion_script(parse_completion_shell("zsh").unwrap()).unwrap();
        assert!(zsh.contains("#compdef agentactr"));

        let err = parse_completion_shell("nu").unwrap_err();
        assert!(err.contains("unsupported completion shell `nu`"));
    }

    #[test]
    fn cli_markdown_reference_is_generated_from_clap_and_catalog() {
        let docs = render_cli_markdown().unwrap();

        assert!(docs.contains("# agentactr CLI Reference"));
        assert!(docs.contains("Generated by `agentactr docs cli-markdown`"));
        assert!(docs.contains("Command: agentactr run issue"));
        assert!(docs.contains("Command: agentactr docs cli-markdown"));
        assert!(docs.contains("[possible values: fail-closed, interactive, review-required]"));
        assert!(docs.contains("linux_memory.oom_policy"));
        assert!(docs.contains("observability.debug_bundle_root"));

        for entry in command_catalog() {
            let catalog_command = markdown_table_cell(&format!("`agentactr {}`", entry.command));
            assert!(
                docs.contains(&catalog_command),
                "CLI Markdown docs missing catalog command: {catalog_command}"
            );
            assert!(
                docs.contains(entry.sdk_use_case_owner),
                "CLI Markdown docs missing SDK owner for {}",
                entry.command
            );
        }
        assert_eq!(docs.matches("### `agentactr finalize`").count(), 1);
        assert!(!docs.contains("### `agentactr finalize RUN_ID --approve`"));
        assert!(!docs.contains("### `agentactr finalize RUN_ID --reject --reason REASON`"));

        let committed = include_str!("../../../docs/cli/reference.md");
        assert_eq!(
            committed, docs,
            "docs/cli/reference.md is stale; regenerate with `agentactr docs cli-markdown --output docs/cli/reference.md`"
        );
    }

    #[test]
    fn cli_markdown_output_rejects_directory_targets() {
        let root = env::temp_dir().join(format!(
            "agentactr-cli-docs-output-dir-test-{}-{}",
            std::process::id(),
            new_run_id("docs-output")
        ));
        fs::create_dir_all(&root).unwrap();

        let err = write_cli_markdown_output(&root, "# docs\n").unwrap_err();

        assert!(err.contains("must not be a directory"));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn cli_markdown_output_rejects_symlink_targets() {
        let root = env::temp_dir().join(format!(
            "agentactr-cli-docs-output-symlink-test-{}-{}",
            std::process::id(),
            new_run_id("docs-output")
        ));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("target.md");
        let link = root.join("reference.md");
        fs::write(&target, "do not overwrite\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = write_cli_markdown_output(&link, "# docs\n").unwrap_err();

        assert!(err.contains("must not be a symlink"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "do not overwrite\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn commands_inventory_reports_implemented_and_milestone_surfaces() {
        let catalog = command_catalog();

        let commands_entry = catalog
            .iter()
            .find(|entry| entry.command == "commands [--json]")
            .unwrap();
        assert_eq!(commands_entry.status, "implemented");
        assert_eq!(commands_entry.side_effects, "read_only");
        let menu_entry = catalog
            .iter()
            .find(|entry| entry.command == "menu [--json]")
            .unwrap();
        assert_eq!(menu_entry.status, "implemented");
        assert_eq!(menu_entry.side_effects, "read_only");
        let completion_entry = catalog
            .iter()
            .find(|entry| entry.command == "completions bash|zsh|fish|powershell|elvish")
            .unwrap();
        assert_eq!(completion_entry.status, "implemented");
        assert_eq!(completion_entry.side_effects, "read_only_stdout");
        let docs_entry = catalog
            .iter()
            .find(|entry| entry.command == "docs cli-markdown [--output PATH]")
            .unwrap();
        assert_eq!(docs_entry.status, "implemented");
        assert_eq!(
            docs_entry.side_effects,
            "read_only_stdout_or_explicit_doc_write"
        );
        let issue_find = catalog
            .iter()
            .find(|entry| entry.command.starts_with("issue find --repo OWNER/REPO"))
            .unwrap();
        assert!(issue_find.command.contains("--label LABEL..."));
        assert!(issue_find.command.contains("--assignee USER|none|*"));
        assert!(issue_find.command.contains("--include-pull-requests"));
        let issue_submit = catalog
            .iter()
            .find(|entry| entry.command.starts_with("issue submit ISSUE_SET_ID"))
            .unwrap();
        assert!(issue_submit
            .command
            .contains("--allow-possible-duplicate --reason REASON"));
        let vcs_status = catalog
            .iter()
            .find(|entry| entry.command == "vcs status RUN_ID")
            .unwrap();
        assert_eq!(vcs_status.status, "implemented");
        assert!(vcs_status
            .platform_constraints
            .contains(&"recorded_worktree"));
        let vcs_list = catalog
            .iter()
            .find(|entry| entry.command == "vcs list [--json]")
            .unwrap();
        assert_eq!(vcs_list.status, "implemented");
        assert_eq!(vcs_list.side_effects, "read_only_local_inventory");
        let vcs_show = catalog
            .iter()
            .find(|entry| entry.command == "vcs show RUN_ID [--json]")
            .unwrap();
        assert_eq!(vcs_show.status, "implemented");
        assert_eq!(vcs_show.side_effects, "read_only_local_detail");
        let vcs_diff = catalog
            .iter()
            .find(|entry| entry.command == "vcs diff RUN_ID [--output PATH]")
            .unwrap();
        assert_eq!(vcs_diff.status, "implemented");
        assert_eq!(vcs_diff.side_effects, "writes_diff_artifact_and_trace");
        let merge_plan = catalog
            .iter()
            .find(|entry| entry.command == "merge plan RUN_ID [--json]")
            .unwrap();
        assert_eq!(merge_plan.status, "implemented");
        assert_eq!(
            merge_plan.side_effects,
            "writes_merge_plan_artifact_and_trace"
        );
        assert!(catalog
            .iter()
            .any(|entry| entry.command == "finalize RUN_ID --approve [--resume]"));
        assert!(catalog
            .iter()
            .any(|entry| entry.command == "finalize RUN_ID --reject --reason REASON [--resume]"));
    }

    #[test]
    fn menu_inventory_is_read_only_and_references_exact_commands() {
        let payload = menu_json_payload();
        assert_eq!(payload["schema_version"], "0.1");
        assert_eq!(payload["mode"], "bootstrap_read_only");
        assert_eq!(payload["automation_surface"], "agentactr commands --json");
        let actions = payload["actions"].as_array().unwrap();
        assert_eq!(actions.len(), command_catalog().len());
        assert_eq!(actions[0]["index"], 1);
        assert_eq!(actions[0]["command"], "--version");
        assert_eq!(actions[0]["equivalent_command"], "agentactr --version");
        assert_eq!(actions[0]["executes"], false);
        assert!(actions
            .iter()
            .any(|action| action["equivalent_command"] == "agentactr run issue --repo OWNER/REPO --issue 123 [--human-intervention fail-closed|interactive|review-required] [--codex-approval never|on-request] [--github-finalization automatic_after_quality_gates|require_human_review|disabled] [--dry-run]"));

        let mut text_args = vec!["menu".to_string()];
        cmd_menu(&mut text_args).unwrap();
        let mut json_args = vec!["menu".to_string(), "--json".to_string()];
        cmd_menu(&mut json_args).unwrap();
        let mut bad_args = vec!["menu".to_string(), "--run".to_string()];
        assert_eq!(
            cmd_menu(&mut bad_args).unwrap_err(),
            "usage: agentactr menu [--json]"
        );
    }

    #[test]
    fn visible_vcs_milestone_commands_fail_with_explicit_milestone_error() {
        for command in ["commit", "cleanup"] {
            let mut args = vec!["vcs".to_string(), command.to_string(), "run-1".to_string()];
            let err = cmd_vcs(&mut args).unwrap_err();
            assert!(
                err.contains(&format!(
                    "`agentactr vcs {command}` is specified but not implemented in this milestone"
                )),
                "unexpected error for vcs {command}: {err}"
            );
        }
    }

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
    fn artifact_integrity_treats_absent_workspace_diff_as_not_recorded() {
        let root = env::temp_dir().join(format!(
            "agentactr-artifact-integrity-no-diff-{}-{}",
            std::process::id(),
            new_run_id("integrity")
        ));
        let artifacts = root.join("artifacts").join("run-1");
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
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();
        let context = load_run_artifact_context(&config, "run-1").unwrap();

        let integrity = collect_artifact_integrity(&context).unwrap();

        assert_eq!(integrity["status"], "verified");
        assert_eq!(integrity["verified"], true);
        assert_eq!(integrity["workspace_diff"]["status"], "not_recorded");
        assert_eq!(integrity["workspace_diff"]["required"], false);
        assert_eq!(integrity["workspace_diff"]["verified"], true);
        assert_eq!(integrity["merge_plan"]["status"], "not_recorded");
        assert_eq!(integrity["merge_plan"]["required"], false);
        assert_eq!(integrity["merge_plan"]["verified"], true);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_integrity_rejects_absolute_parent_escape() {
        let root = env::temp_dir().join(format!(
            "agentactr-artifact-path-test-{}-{}",
            std::process::id(),
            new_run_id("debug")
        ));
        let artifacts = root.join("artifacts").join("run-1");
        fs::create_dir_all(&artifacts).unwrap();
        fs::write(root.join("artifacts").join("outside.txt"), "outside\n").unwrap();

        let escaped = artifacts.join("..").join("outside.txt");
        let integrity = verify_artifact_digest(
            &artifacts,
            &escaped,
            Some("sha256:not-used"),
            serde_json::json!({}),
        );

        assert_eq!(integrity["status"], "path_outside_artifact_root");
        assert_eq!(integrity["verified"], false);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn artifact_integrity_rejects_symlink_escape() {
        let root = env::temp_dir().join(format!(
            "agentactr-artifact-symlink-test-{}-{}",
            std::process::id(),
            new_run_id("debug")
        ));
        let artifacts = root.join("artifacts").join("run-1");
        fs::create_dir_all(&artifacts).unwrap();
        let outside = root.join("outside-secret.txt");
        fs::write(&outside, "outside\n").unwrap();
        std::os::unix::fs::symlink(&outside, artifacts.join("linked-secret.txt")).unwrap();

        let integrity = verify_artifact_digest(
            &artifacts,
            &artifacts.join("linked-secret.txt"),
            Some("sha256:not-used"),
            serde_json::json!({}),
        );

        assert_eq!(integrity["status"], "path_outside_artifact_root");
        assert_eq!(integrity["verified"], false);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_context_rejects_mismatched_worktree_run_id() {
        let root = env::temp_dir().join(format!(
            "agentactr-worktree-run-id-test-{}-{}",
            std::process::id(),
            new_run_id("context")
        ));
        let artifacts = root.join("artifacts").join("run-1");
        let worktree = root.join("worktrees").join("run-1");
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
                    "run_id": "run-2"
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();

        let err = match load_run_artifact_context(&config, "run-1") {
            Ok(_) => panic!("mismatched worktree run id should fail"),
            Err(err) => err,
        };

        assert!(err.contains("worktree.run_id `run-2`, not `run-1`"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_worktree_scope_rejects_manifest_path_outside_worktree_root() {
        let root = env::temp_dir().join(format!(
            "agentactr-worktree-scope-test-{}-{}",
            std::process::id(),
            new_run_id("context")
        ));
        let artifacts = root.join("artifacts").join("run-1");
        let worktree_root = root.join("worktrees");
        let outside = root.join("outside-worktree");
        fs::create_dir_all(&artifacts).unwrap();
        fs::create_dir_all(&worktree_root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            artifacts.join("context_manifest.json"),
            serde_json::json!({
                "run_id": "run-1",
                "repo": "OWNER/REPO",
                "issue": "42",
                "worktree": {
                    "path": outside.display().to_string(),
                    "base_commit": "abc123",
                    "run_id": "run-1"
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();
        config.vcs.worktree_root = worktree_root.display().to_string();
        let context = load_run_artifact_context(&config, "run-1").unwrap();

        let err = validate_run_worktree_scope(&config, &context).unwrap_err();

        assert!(err.contains("outside configured vcs.worktree_root"));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn run_worktree_scope_rejects_symlink_escape() {
        let root = env::temp_dir().join(format!(
            "agentactr-worktree-symlink-test-{}-{}",
            std::process::id(),
            new_run_id("context")
        ));
        let artifacts = root.join("artifacts").join("run-1");
        let worktree_root = root.join("worktrees");
        let outside = root.join("outside-worktree");
        let linked = worktree_root.join("run-1");
        fs::create_dir_all(&artifacts).unwrap();
        fs::create_dir_all(&worktree_root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, &linked).unwrap();
        fs::write(
            artifacts.join("context_manifest.json"),
            serde_json::json!({
                "run_id": "run-1",
                "repo": "OWNER/REPO",
                "issue": "42",
                "worktree": {
                    "path": linked.display().to_string(),
                    "base_commit": "abc123",
                    "run_id": "run-1"
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();
        config.vcs.worktree_root = worktree_root.display().to_string();
        let context = load_run_artifact_context(&config, "run-1").unwrap();

        let err = validate_run_worktree_scope(&config, &context).unwrap_err();

        assert!(err.contains("resolves outside configured vcs.worktree_root"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_scoped_paths_reject_non_segment_run_ids() {
        let root = env::temp_dir().join(format!(
            "agentactr-run-id-segment-test-{}-{}",
            std::process::id(),
            new_run_id("debug")
        ));
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();
        config.observability.debug_bundle_root = root.join("debug").display().to_string();

        assert!(run_artifact_dir(&config, "issue-42-123").is_ok());
        for invalid in ["", "..", "../escape", "nested/run", "nested\\run", "run id"] {
            assert!(
                run_artifact_dir(&config, invalid).is_err(),
                "run_artifact_dir accepted invalid RUN_ID {invalid:?}"
            );
            assert!(
                debug_bundle_dir(&config, invalid).is_err(),
                "debug_bundle_dir accepted invalid RUN_ID {invalid:?}"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quality_rerun_can_write_non_finalizing_report_artifact() {
        let root = env::temp_dir().join(format!(
            "agentactr-quality-rerun-test-{}-{}",
            std::process::id(),
            new_run_id("quality")
        ));
        fs::create_dir_all(&root).unwrap();
        let report_path = root.join("quality_report.rerun.test.txt");
        let inspection = RepoInspection {
            root: root.clone(),
            is_git: false,
            is_empty: false,
            detected_stack: StackKind::Unknown,
            primary_stack: StackKind::Unknown,
            confidence: 100,
            evidence_files: Vec::new(),
            missing_prerequisites: Vec::new(),
            setup_guidance: Vec::new(),
            selected_quality_profile: "strict".to_string(),
            quality_plan: vec![agentactr_sdk::QualityCommand {
                name: "noop".to_string(),
                command: "true".to_string(),
                required: true,
                non_mutating_final_gate: true,
            }],
            domain_profiles: Vec::new(),
            domain_quality_plan: Vec::new(),
            domain_graph: empty_test_domain_graph(),
        };

        run_quality_gates_to_report(&inspection, &root, &report_path, &[]).unwrap();

        let report = fs::read_to_string(&report_path).unwrap();
        assert!(report.contains("## noop"));
        assert!(report.contains("command=true"));
        assert!(report.contains("status=exit status: 0"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quality_report_executes_safe_domain_command_gates_and_records_findings() {
        let root = env::temp_dir().join(format!(
            "agentactr-domain-quality-test-{}-{}",
            std::process::id(),
            new_run_id("quality")
        ));
        fs::create_dir_all(&root).unwrap();
        let report_path = root.join("quality_report.txt");
        let inspection = RepoInspection {
            root: root.clone(),
            is_git: false,
            is_empty: false,
            detected_stack: StackKind::Unknown,
            primary_stack: StackKind::Unknown,
            confidence: 100,
            evidence_files: Vec::new(),
            missing_prerequisites: Vec::new(),
            setup_guidance: Vec::new(),
            selected_quality_profile: "strict".to_string(),
            quality_plan: Vec::new(),
            domain_profiles: Vec::new(),
            domain_quality_plan: vec![
                agentactr_sdk::DomainQualityGate::finding_gate(
                    "grpc_deadlines",
                    "rpc.grpc",
                    "agentactr",
                    "client RPCs require deadlines",
                ),
                agentactr_sdk::DomainQualityGate::command_gate(
                    "domain_noop",
                    "api_contracts.protobuf",
                    "sh",
                    "true",
                ),
            ],
            domain_graph: empty_test_domain_graph(),
        };

        run_quality_gates_to_report(&inspection, &root, &report_path, &[]).unwrap();

        let report = fs::read_to_string(&report_path).unwrap();
        assert!(report.contains("## domain:grpc_deadlines"));
        assert!(report.contains("status=finding-only"));
        assert!(report.contains("## domain:domain_noop"));
        assert!(report.contains("command=true"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quality_report_runs_opted_in_domain_gate_and_skips_without_opt_in() {
        let root = env::temp_dir().join(format!(
            "agentactr-domain-optin-quality-test-{}-{}",
            std::process::id(),
            new_run_id("quality")
        ));
        fs::create_dir_all(&root).unwrap();
        let skipped_report_path = root.join("quality_report.skipped.txt");
        let opted_report_path = root.join("quality_report.opted.txt");
        let inspection = RepoInspection {
            root: root.clone(),
            is_git: false,
            is_empty: false,
            detected_stack: StackKind::Unknown,
            primary_stack: StackKind::Unknown,
            confidence: 100,
            evidence_files: Vec::new(),
            missing_prerequisites: Vec::new(),
            setup_guidance: Vec::new(),
            selected_quality_profile: "strict".to_string(),
            quality_plan: Vec::new(),
            domain_profiles: Vec::new(),
            domain_quality_plan: vec![agentactr_sdk::DomainQualityGate {
                opt_in_required: true,
                network_required: true,
                setup_guidance: vec!["explicitly opt in before running preview".to_string()],
                ..agentactr_sdk::DomainQualityGate::command_gate(
                    "pulumi_preview",
                    "iac.pulumi",
                    "sh",
                    "true",
                )
            }],
            domain_graph: empty_test_domain_graph(),
        };

        run_quality_gates_to_report(&inspection, &root, &skipped_report_path, &[]).unwrap();
        let skipped_report = fs::read_to_string(&skipped_report_path).unwrap();
        assert!(skipped_report.contains("status=skipped"));
        assert!(skipped_report.contains("enabled_by_config=false"));

        run_quality_gates_to_report(
            &inspection,
            &root,
            &opted_report_path,
            &["iac.pulumi:pulumi_preview".to_string()],
        )
        .unwrap();
        let opted_report = fs::read_to_string(&opted_report_path).unwrap();
        assert!(opted_report.contains("status=exit status: 0"));
        assert!(opted_report.contains("enabled_by_config=true"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quality_report_preserves_configured_domain_policy_for_worktree() {
        let root = env::temp_dir().join(format!(
            "agentactr-domain-policy-test-{}-{}",
            std::process::id(),
            new_run_id("quality")
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        fs::write(root.join("buf.yaml"), "version: v2\n").unwrap();
        fs::write(
            root.join("service.proto"),
            "syntax = \"proto3\";\npackage app.v1;\n",
        )
        .unwrap();
        let report_path = root.join("quality_report.txt");
        let inspection = RepoInspection {
            root: root.clone(),
            is_git: false,
            is_empty: false,
            detected_stack: StackKind::Unknown,
            primary_stack: StackKind::Unknown,
            confidence: 100,
            evidence_files: Vec::new(),
            missing_prerequisites: Vec::new(),
            setup_guidance: Vec::new(),
            selected_quality_profile: "strict".to_string(),
            quality_plan: vec![agentactr_sdk::QualityCommand {
                name: "noop".to_string(),
                command: "true".to_string(),
                required: true,
                non_mutating_final_gate: true,
            }],
            domain_profiles: Vec::new(),
            domain_quality_plan: Vec::new(),
            domain_graph: empty_test_domain_graph(),
        };

        run_quality_gates_to_report(&inspection, &root, &report_path, &[]).unwrap();

        let report = fs::read_to_string(&report_path).unwrap();
        assert!(report.contains("## noop"));
        assert!(!report.contains("## domain:"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quality_failure_uses_repo_relative_report_path_and_network_guidance() {
        let root = env::temp_dir().join(format!(
            "agentactr-quality-network-test-{}-{}",
            std::process::id(),
            new_run_id("quality")
        ));
        let worktree = root.join("worktree");
        let artifact_dir = root.join(".agentactr").join("artifacts").join("run-1");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&artifact_dir).unwrap();
        let report_path = artifact_dir.join("quality_report.txt");
        let inspection = RepoInspection {
            root: worktree.clone(),
            is_git: false,
            is_empty: false,
            detected_stack: StackKind::Unknown,
            primary_stack: StackKind::Unknown,
            confidence: 100,
            evidence_files: Vec::new(),
            missing_prerequisites: Vec::new(),
            setup_guidance: Vec::new(),
            selected_quality_profile: "strict".to_string(),
            quality_plan: vec![agentactr_sdk::QualityCommand {
                name: "install".to_string(),
                command:
                    "printf 'error: ConnectionRefused downloading package manifest foo' >&2; exit 1"
                        .to_string(),
                required: true,
                non_mutating_final_gate: true,
            }],
            domain_profiles: Vec::new(),
            domain_quality_plan: Vec::new(),
            domain_graph: empty_test_domain_graph(),
        };

        let err =
            run_quality_gates_to_report(&inspection, &worktree, &report_path, &[]).unwrap_err();
        let report = fs::read_to_string(&report_path).unwrap();

        assert!(err.contains("report=.agentactr/artifacts/run-1/quality_report.txt"));
        assert!(!err.contains(&root.display().to_string()));
        assert!(err.contains("--human-intervention interactive --codex-approval on-request"));
        assert!(report.contains("network_guidance:"));
        assert!(report.contains("quality gate `install` appears to require network access"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn vcs_status_reads_manifest_worktree_and_metadata() {
        let root = env::temp_dir().join(format!(
            "agentactr-vcs-status-test-{}-{}",
            std::process::id(),
            new_run_id("vcs")
        ));
        let worktree = root.join("worktree");
        let artifacts = root.join("artifacts").join("run-1");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&artifacts).unwrap();
        run_git_test_command(&worktree, &["init"]);
        run_git_test_command(
            &worktree,
            &["config", "user.email", "agentactr@example.invalid"],
        );
        run_git_test_command(&worktree, &["config", "user.name", "Agent Actr"]);
        fs::write(worktree.join("file.txt"), "before\n").unwrap();
        run_git_test_command(&worktree, &["add", "file.txt"]);
        run_git_test_command(&worktree, &["commit", "-m", "initial"]);
        let base_commit = git_output_in_dir(&worktree, &["rev-parse", "HEAD"]).unwrap();
        fs::write(worktree.join("file.txt"), "after\n").unwrap();
        fs::write(
            worktree.join(".agentactr-run.toml"),
            "branch_name = \"agentactr/test\"\nsource_checkout_clean_at_prepare = true\n",
        )
        .unwrap();
        fs::write(
            artifacts.join("context_manifest.json"),
            serde_json::json!({
                "run_id": "run-1",
                "repo": "OWNER/REPO",
                "issue": "42",
                "worktree": {
                    "path": worktree.display().to_string(),
                    "base_commit": base_commit,
                    "run_id": "run-1"
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();

        let context = load_run_artifact_context(&config, "run-1").unwrap();
        let status = collect_vcs_status(&context).unwrap();

        assert_eq!(status.branch_name, "agentactr/test");
        assert_eq!(status.source_checkout_clean_at_prepare, Some(true));
        assert_eq!(status.touched_files, vec!["file.txt"]);
        assert_eq!(status.repo, "OWNER/REPO");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn vcs_inventory_and_show_are_read_only_and_scope_checked() {
        let root = env::temp_dir().join(format!(
            "agentactr-vcs-inventory-test-{}-{}",
            std::process::id(),
            new_run_id("vcs")
        ));
        let worktree_root = root.join("worktrees");
        let worktree = worktree_root.join("run-1");
        let artifacts = root.join("artifacts");
        let run_artifacts = artifacts.join("run-1");
        let invalid_artifacts = artifacts.join("run-2");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&run_artifacts).unwrap();
        fs::create_dir_all(&invalid_artifacts).unwrap();
        run_git_test_command(&worktree, &["init"]);
        run_git_test_command(
            &worktree,
            &["config", "user.email", "agentactr@example.invalid"],
        );
        run_git_test_command(&worktree, &["config", "user.name", "Agent Actr"]);
        fs::write(worktree.join("file.txt"), "before\n").unwrap();
        run_git_test_command(&worktree, &["add", "file.txt"]);
        run_git_test_command(&worktree, &["commit", "-m", "initial"]);
        let base_commit = git_output_in_dir(&worktree, &["rev-parse", "HEAD"]).unwrap();
        fs::write(worktree.join("file.txt"), "after\n").unwrap();
        fs::write(
            worktree.join(".agentactr-run.toml"),
            "branch_name = \"agentactr/test\"\nsource_checkout_clean_at_prepare = false\n",
        )
        .unwrap();
        fs::write(
            run_artifacts.join("context_manifest.json"),
            serde_json::json!({
                "run_id": "run-1",
                "repo": "OWNER/REPO",
                "issue": "42",
                "worktree": {
                    "path": worktree.display().to_string(),
                    "base_commit": base_commit,
                    "run_id": "run-1"
                }
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            invalid_artifacts.join("context_manifest.json"),
            serde_json::json!({
                "run_id": "run-2",
                "repo": "OWNER/REPO",
                "issue": "43",
                "worktree": {
                    "path": root.join("outside").display().to_string(),
                    "base_commit": "abc123",
                    "run_id": "run-2"
                }
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
                    "payload": {"status": "completed"}
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
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = artifacts.display().to_string();
        config.observability.jsonl = trace_path.display().to_string();
        config.vcs.worktree_root = worktree_root.display().to_string();

        let entries = collect_vcs_inventory(&config).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries[0].valid);
        assert_eq!(entries[0].run_id, "run-1");
        assert_eq!(entries[0].last_run_status, "completed");
        assert_eq!(entries[0].branch_name.as_deref(), Some("agentactr/test"));
        assert_eq!(entries[0].touched_file_count, Some(1));
        assert!(!entries[1].valid);
        assert_eq!(entries[1].run_id, "run-2");
        assert_eq!(entries[1].last_run_status, "started");
        assert!(entries[1]
            .error
            .as_deref()
            .unwrap()
            .contains("outside configured vcs.worktree_root"));

        let mut context = load_run_artifact_context(&config, "run-1").unwrap();
        context.worktree = validate_run_worktree_scope(&config, &context).unwrap();
        let status = collect_vcs_status(&context).unwrap();
        let records = read_trace_records(&trace_path).unwrap();
        let payload =
            vcs_show_payload(&config, &status, &latest_run_status(&records, "run-1")).unwrap();
        assert_eq!(payload["run_status"], "completed");
        assert_eq!(
            payload["worktree_metadata"]["branch_name"],
            "agentactr/test"
        );
        assert_eq!(
            payload["vcs_policy"]["worktree_root"],
            config.vcs.worktree_root
        );
        assert_eq!(
            payload["milestone_status"]["merge_plan"],
            "implemented_read_only"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn vcs_diff_records_patch_metadata_and_trace() {
        let root = env::temp_dir().join(format!(
            "agentactr-vcs-diff-test-{}-{}",
            std::process::id(),
            new_run_id("vcs")
        ));
        let worktree_root = root.join("worktrees");
        let worktree = worktree_root.join("run-1");
        let artifacts = root.join("artifacts").join("run-1");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&artifacts).unwrap();
        run_git_test_command(&worktree, &["init"]);
        run_git_test_command(
            &worktree,
            &["config", "user.email", "agentactr@example.invalid"],
        );
        run_git_test_command(&worktree, &["config", "user.name", "Agent Actr"]);
        fs::write(worktree.join("file.txt"), "before\n").unwrap();
        run_git_test_command(&worktree, &["add", "file.txt"]);
        run_git_test_command(&worktree, &["commit", "-m", "initial"]);
        let base_commit = git_output_in_dir(&worktree, &["rev-parse", "HEAD"]).unwrap();
        fs::write(worktree.join("file.txt"), "after\n").unwrap();
        fs::write(worktree.join("new.txt"), "new\n").unwrap();
        fs::write(
            artifacts.join("context_manifest.json"),
            serde_json::json!({
                "run_id": "run-1",
                "repo": "OWNER/REPO",
                "issue": "42",
                "worktree": {
                    "path": worktree.display().to_string(),
                    "base_commit": base_commit,
                    "run_id": "run-1"
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();
        config.observability.jsonl = root.join("events.jsonl").display().to_string();
        config.vcs.worktree_root = worktree_root.display().to_string();
        let mut context = load_run_artifact_context(&config, "run-1").unwrap();
        context.worktree = validate_run_worktree_scope(&config, &context).unwrap();

        let diff = collect_workspace_diff(&context).unwrap();
        let (patch_path, metadata_path) =
            record_workspace_diff_artifacts(&config, &context, &diff, None).unwrap();

        assert_eq!(diff.run_id, "run-1");
        assert!(!diff.is_empty);
        assert!(diff.patch.contains("diff --git a/file.txt b/file.txt"));
        assert!(diff.patch.ends_with('\n'));
        assert!(diff.patch.contains("diff --git a/new.txt b/new.txt"));
        assert!(diff.patch.contains("new file mode 100644"));
        assert!(diff.touched_files.contains(&"file.txt".to_string()));
        assert!(diff.touched_files.contains(&"new.txt".to_string()));
        assert_eq!(diff.untracked_files, vec!["new.txt"]);
        let patch = fs::read_to_string(&patch_path).unwrap();
        assert_eq!(patch, diff.patch);
        let metadata =
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&metadata_path).unwrap())
                .unwrap();
        assert_eq!(metadata["run_id"], "run-1");
        assert_eq!(metadata["artifact"], patch_path.display().to_string());
        assert_eq!(metadata["untracked_file_count"], 1);
        assert_eq!(metadata["includes_untracked_file_bodies"], true);
        assert!(metadata["patch_sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        let trace = fs::read_to_string(root.join("events.jsonl")).unwrap();
        assert!(trace.contains("\"event_type\":\"vcs.diff.recorded\""));
        assert!(trace.contains(&patch_path.display().to_string()));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn github_rate_limit_artifact_is_emitted_as_trace_event() {
        let root = env::temp_dir().join(format!(
            "agentactr-github-rate-trace-test-{}-{}",
            std::process::id(),
            new_run_id("rate")
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("github_rate_limit_events.jsonl"),
            r#"{"attempt":1,"status":429,"reason":"secondary-rate-limit-fallback"}"#,
        )
        .unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.jsonl = root.join("events.jsonl").display().to_string();

        emit_github_rate_limit_trace_events(&config, "run-1", "OWNER/REPO", "42", &root).unwrap();

        let events = fs::read_to_string(root.join("events.jsonl")).unwrap();
        let parsed = serde_json::from_str::<serde_json::Value>(events.trim()).unwrap();
        assert_eq!(parsed["event_type"], "github.rate_limit.updated");
        assert_eq!(parsed["payload"]["status"], 429);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merge_plan_records_disabled_read_only_artifact_and_trace() {
        let root = env::temp_dir().join(format!(
            "agentactr-merge-plan-test-{}-{}",
            std::process::id(),
            new_run_id("merge")
        ));
        let worktree_root = root.join("worktrees");
        let worktree = worktree_root.join("run-1");
        let artifacts = root.join("artifacts").join("run-1");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&artifacts).unwrap();
        run_git_test_command(&worktree, &["init"]);
        run_git_test_command(
            &worktree,
            &["config", "user.email", "agentactr@example.invalid"],
        );
        run_git_test_command(&worktree, &["config", "user.name", "Agent Actr"]);
        fs::write(worktree.join("file.txt"), "before\n").unwrap();
        run_git_test_command(&worktree, &["add", "file.txt"]);
        run_git_test_command(&worktree, &["commit", "-m", "initial"]);
        let base_commit = git_output_in_dir(&worktree, &["rev-parse", "HEAD"]).unwrap();
        fs::write(worktree.join("file.txt"), "after\n").unwrap();
        fs::write(artifacts.join("workspace.diff.patch"), "diff body\n").unwrap();
        fs::write(
            artifacts.join("context_manifest.json"),
            serde_json::json!({
                "run_id": "run-1",
                "repo": "OWNER/REPO",
                "issue": "42",
                "worktree": {
                    "path": worktree.display().to_string(),
                    "base_commit": base_commit,
                    "run_id": "run-1"
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.artifact_root = root.join("artifacts").display().to_string();
        config.observability.jsonl = root.join("events.jsonl").display().to_string();
        config.vcs.worktree_root = worktree_root.display().to_string();
        config.vcs.base_ref = base_commit.clone();
        config.merge.mode = "disabled".to_string();
        let mut context = load_run_artifact_context(&config, "run-1").unwrap();
        context.worktree = validate_run_worktree_scope(&config, &context).unwrap();

        let plan = collect_merge_plan(&config, &context).unwrap();
        let artifact = record_merge_plan_artifact(&config, &context, &plan).unwrap();

        assert_eq!(plan.run_id, "run-1");
        assert_eq!(plan.merge_mode, "disabled");
        assert!(!plan.merge_enabled);
        assert!(!plan.base_ref_drifted);
        assert!(plan.workspace_diff_exists);
        assert_eq!(plan.recommendation, "do_not_merge");
        assert!(plan
            .blockers
            .contains(&"merge.mode is disabled".to_string()));
        assert!(plan.touched_files.contains(&"file.txt".to_string()));
        let payload =
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&artifact).unwrap())
                .unwrap();
        assert_eq!(payload["recommendation"], "do_not_merge");
        assert_eq!(payload["workspace_diff_exists"], true);
        assert_eq!(
            payload["workspace_diff_artifact"],
            artifacts.join("workspace.diff.patch").display().to_string()
        );
        let metadata_path = artifacts.join("merge_plan.metadata.json");
        let metadata =
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&metadata_path).unwrap())
                .unwrap();
        assert_eq!(metadata["artifact"], artifact.display().to_string());
        assert_eq!(
            metadata["metadata_artifact"],
            metadata_path.display().to_string()
        );
        assert!(metadata["artifact_sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(metadata["recommendation"], "do_not_merge");
        let integrity = collect_artifact_integrity(&context).unwrap();
        assert_eq!(integrity["merge_plan"]["status"], "verified");
        assert_eq!(integrity["merge_plan"]["verified"], true);
        assert_eq!(
            integrity["merge_plan"]["expected_sha256"],
            metadata["artifact_sha256"]
        );
        let trace = fs::read_to_string(root.join("events.jsonl")).unwrap();
        assert!(trace.contains("\"event_type\":\"vcs.merge_plan.recorded\""));
        assert!(trace.contains(&artifact.display().to_string()));
        assert!(trace.contains(&metadata_path.display().to_string()));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn go_logical_gates_do_not_execute_literal_wrapper_names_when_empty() {
        let root = env::temp_dir().join(format!(
            "agentactr-go-gate-test-{}-{}",
            std::process::id(),
            new_run_id("go")
        ));
        fs::create_dir_all(&root).unwrap();

        let output = run_quality_command("gofmt", "gofmt-check", &root).unwrap();

        assert_eq!(output.executed_command, "gofmt -l <go files>");
        assert!(output.success);
        assert_eq!(output.status, "skipped: no Go files");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scoped_go_gofmt_gate_expands_logical_wrapper_in_module_dir() {
        let root = env::temp_dir().join(format!(
            "agentactr-go-scoped-gofmt-test-{}-{}",
            std::process::id(),
            new_run_id("go")
        ));
        let module = root.join("services/api");
        fs::create_dir_all(&module).unwrap();

        let output = run_quality_command(
            "gofmt:services/api",
            "cd services/api && gofmt-check",
            &root,
        )
        .unwrap();

        assert_eq!(
            output.executed_command,
            "cd services/api && gofmt -l <go files>"
        );
        assert!(output.success);
        assert_eq!(output.status, "skipped: no Go files");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scoped_go_tidy_gate_expands_logical_wrapper_in_module_dir() {
        let root = env::temp_dir().join(format!(
            "agentactr-go-scoped-tidy-test-{}-{}",
            std::process::id(),
            new_run_id("go")
        ));
        let module = root.join("services/api");
        fs::create_dir_all(&module).unwrap();

        let output = run_quality_command(
            "tidy_check:services/api",
            "cd services/api && go mod tidy-check",
            &root,
        )
        .unwrap();

        assert_eq!(
            output.executed_command,
            "cd services/api && go mod tidy in temporary copy"
        );
        assert!(!output.success);
        assert_eq!(output.status, "failed: go.mod missing");
        assert_eq!(output.stderr, "go mod tidy-check requires go.mod");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scoped_go_logical_wrapper_accepts_quoted_module_dir() {
        let root = env::temp_dir().join(format!(
            "agentactr-go-quoted-scope-test-{}-{}",
            std::process::id(),
            new_run_id("go")
        ));
        let module = root.join("services/api space");
        fs::create_dir_all(&module).unwrap();

        let output = run_quality_command(
            "gofmt:services/api space",
            "cd 'services/api space' && gofmt-check",
            &root,
        )
        .unwrap();

        assert_eq!(
            output.executed_command,
            "cd services/api space && gofmt -l <go files>"
        );
        assert!(output.success);
        assert_eq!(output.status, "skipped: no Go files");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn context_artifacts_record_spawn_plan() {
        let root = env::temp_dir().join(format!(
            "agentactr-context-artifact-test-{}-{}",
            std::process::id(),
            new_run_id("context")
        ));
        let worktree = root.join("worktree");
        let artifacts = root.join("artifacts");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&artifacts).unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.observability.jsonl = root.join("events.jsonl").display().to_string();
        let event =
            RunEventContext::agent(&config, "run-1", "OWNER/REPO", "42", "agent-run-1", None);
        let worktree_ref = agentactr_sdk::WorktreeRef {
            path: worktree,
            base_commit: "abc123".to_string(),
            run_id: "run-1".to_string(),
        };
        let memory = MemoryRunContext {
            enforce: false,
            run_group: None,
            agent_group: None,
            agent_group_id: None,
            status_artifact: artifacts.join("memory_status.json"),
        };
        let issue = agentactr_sdk::Issue {
            id: "OWNER/REPO#42".to_string(),
            repo: "OWNER/REPO".to_string(),
            number: 42,
            title: "test".to_string(),
            body: String::new(),
            state: "open".to_string(),
            author: "octo".to_string(),
            labels: Vec::new(),
            created_at: None,
            updated_at: None,
            is_pull_request: false,
            html_url: None,
            source_artifact: None,
        };
        let inspection = RepoInspection {
            root: root.clone(),
            is_git: true,
            is_empty: false,
            detected_stack: StackKind::Rust,
            primary_stack: StackKind::Rust,
            confidence: 100,
            evidence_files: vec!["Cargo.toml".to_string()],
            missing_prerequisites: Vec::new(),
            setup_guidance: Vec::new(),
            selected_quality_profile: "strict".to_string(),
            quality_plan: Vec::new(),
            domain_profiles: Vec::new(),
            domain_quality_plan: Vec::new(),
            domain_graph: empty_test_domain_graph(),
        };
        let policy = RunPolicy::new(
            HumanInterventionSetting::FailClosed,
            CodexApprovalSetting::Never,
            GithubFinalizationSetting::AutomaticAfterQualityGates,
        )
        .unwrap();
        let spawn_plan = build_spawn_plan(&config, "run-1", "agent-run-1", &artifacts);
        let child_agent_run_id = spawn_plan.child_nodes[0].agent_run_id.as_str().to_string();
        let child_memory = vec![ChildMemoryAssignment {
            agent_run_id: child_agent_run_id.clone(),
            lease: MemoryLease {
                group_id: agentactr_sdk::MemoryGroupId::new("run:run-1:agent:child"),
                policy: agentactr_sdk::MemoryPolicyRef::new("linux_memory.agent"),
            },
            group: artifacts.join("memory").join("agent-child"),
        }];

        write_run_context_artifacts(&RunContextArtifactInput {
            event,
            agent_run_id: "agent-run-1",
            worktree_ref: &worktree_ref,
            artifact_dir: &artifacts,
            trace_path: &root.join("events.jsonl"),
            memory: &memory,
            child_memory: &child_memory,
            issue_context: &issue,
            inspection: &inspection,
            stack_source: &RepositoryStackSource::Detection,
            run_policy: &policy,
            spawn_plan: &spawn_plan,
        })
        .unwrap();

        let graph = fs::read_to_string(artifacts.join("agent_graph.json")).unwrap();
        let parsed = serde_json::from_str::<serde_json::Value>(&graph).unwrap();
        assert_eq!(
            parsed["spawn"]["mode"],
            "one_writer_parallel_read_only_helpers"
        );
        assert!(parsed["nodes"].as_array().unwrap().len() > 1);
        let child_node = parsed["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["agent_run_id"].as_str() == Some(child_agent_run_id.as_str()))
            .unwrap();
        assert_eq!(child_node["memory_group_id"], "run:run-1:agent:child");
        let manifest = fs::read_to_string(artifacts.join("context_manifest.json")).unwrap();
        let parsed = serde_json::from_str::<serde_json::Value>(&manifest).unwrap();
        assert_eq!(parsed["repository"]["detected_stack"], "rust");
        assert_eq!(parsed["repository"]["selected_stack"], "rust");
        assert_eq!(parsed["repository"]["stack_source"], "repository_detection");
        assert_eq!(parsed["repository"]["selected_quality_profile"], "strict");
        assert_eq!(
            parsed["artifacts"]["repository_context"],
            artifacts
                .join("repository_context.json")
                .display()
                .to_string()
        );
        let repo_context = fs::read_to_string(artifacts.join("repository_context.json")).unwrap();
        let repo_context = serde_json::from_str::<serde_json::Value>(&repo_context).unwrap();
        assert_eq!(repo_context["selected_stack"], "rust");
        assert_eq!(repo_context["stack_source"], "repository_detection");
        assert_eq!(
            parsed["spawn"]["mode"],
            "one_writer_parallel_read_only_helpers"
        );
        assert_eq!(
            parsed["artifacts"]["adapter_version_reports"],
            artifacts
                .join("adapter_version_reports.json")
                .display()
                .to_string()
        );
        assert_eq!(
            parsed["artifacts"]["agent_graph"],
            artifacts.join("agent_graph.json").display().to_string()
        );
        assert_eq!(
            parsed["artifacts"]["spawn_handoffs"],
            artifacts.join("spawn_handoffs.json").display().to_string()
        );
        assert_eq!(
            parsed["artifacts"]["runtime_process_events"],
            artifacts
                .join("runtime_process_events.jsonl")
                .display()
                .to_string()
        );
        assert_eq!(
            parsed["artifacts"]["codex_prompt"],
            artifacts.join("codex.prompt.txt").display().to_string()
        );
        assert_eq!(
            parsed["artifacts"]["codex_prompt_metadata"],
            artifacts
                .join("codex.prompt.metadata.json")
                .display()
                .to_string()
        );
        assert_eq!(
            parsed["artifacts"]["codex_stdout_jsonl"],
            artifacts.join("codex.stdout.jsonl").display().to_string()
        );
        assert_eq!(
            parsed["artifacts"]["codex_stderr_log"],
            artifacts.join("codex.stderr.log").display().to_string()
        );
        assert_eq!(
            parsed["artifacts"]["github_rate_limit_events"],
            artifacts
                .join("github_rate_limit_events.jsonl")
                .display()
                .to_string()
        );
        assert_eq!(
            parsed["artifacts"]["github_rate_limit_log"],
            artifacts
                .join("github_issue.rate_limit.log")
                .display()
                .to_string()
        );
        assert_eq!(
            parsed["artifacts"]["workspace_diff"],
            artifacts.join("workspace.diff.patch").display().to_string()
        );
        assert_eq!(
            parsed["artifacts"]["workspace_diff_metadata"],
            artifacts
                .join("workspace.diff.metadata.json")
                .display()
                .to_string()
        );
        assert_eq!(
            parsed["artifacts"]["merge_plan"],
            artifacts.join("merge_plan.json").display().to_string()
        );
        assert_eq!(
            parsed["artifacts"]["merge_plan_metadata"],
            artifacts
                .join("merge_plan.metadata.json")
                .display()
                .to_string()
        );
        assert_eq!(
            parsed["artifacts"]["finalization_status"],
            artifacts
                .join("finalization_status.json")
                .display()
                .to_string()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn config_set_preserves_sectioned_toml() {
        let path = env::temp_dir().join(format!(
            "agentactr-config-set-test-{}-{}.toml",
            std::process::id(),
            new_run_id("config")
        ));
        let original = render_agentactr_toml(&AgentactrConfig::strict_defaults("OWNER/REPO"));
        fs::write(&path, original).unwrap();

        set_config_value(
            path.to_str().unwrap(),
            "codex.approval_policy",
            "on-request",
        )
        .unwrap();

        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("[codex]"));
        assert!(
            !updated.lines().any(|line| line.starts_with('\t')),
            "config set must regenerate deterministic, unindented top-level TOML"
        );
        assert!(updated
            .contains("approval_policy = \"on-request\" # possible values: never, on-request"));
        assert!(updated.contains(
            "finalization = \"require_human_review\" # possible values: automatic_after_quality_gates, require_human_review, disabled"
        ));
        assert_eq!(
            find_config_value(&updated, "codex.approval_policy").as_deref(),
            Some("on-request")
        );
        assert!(updated.lines().count() > 20);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_key_help_values_include_loaded_linux_memory_and_observability_keys() {
        for key in [
            "linux_memory.enabled",
            "linux_memory.cgroup_root",
            "linux_memory.root_group",
            "linux_memory.mode",
            "linux_memory.cgroup_v2_required",
            "linux_memory.psi_required",
            "linux_memory.per_issue_memory_high",
            "linux_memory.per_issue_memory_max",
            "linux_memory.per_agent_memory_high",
            "linux_memory.per_agent_memory_max",
            "linux_memory.psi_memory_some_threshold_us",
            "linux_memory.psi_memory_window_us",
            "linux_memory.oom_score_adj",
            "linux_memory.setrlimit_address_space",
            "linux_memory.setrlimit_file_size",
            "linux_memory.kill_policy",
            "linux_memory.oom_policy",
            "quality.domains",
            "quality.domain_gate_opt_ins",
            "architecture.domains",
            "architecture.domain_graph_artifact",
            "architecture.fail_on_domain_drift",
            "templates.enabled_domains",
            "templates.framework_profile",
            "templates.agents_policy",
            "observability.jsonl",
            "observability.sqlite",
            "observability.artifact_root",
            "observability.otel_enabled",
            "observability.otel_endpoint",
            "observability.debug_bundle_root",
            "observability.redact_secrets",
        ] {
            assert!(
                CONFIG_KEY_VALUES.contains(&key),
                "CONFIG_KEY_VALUES is missing loaded config key `{key}`"
            );
        }
    }

    #[test]
    fn version_string_uses_crate_version() {
        let version = version_string();
        assert!(version.starts_with(&format!("agentactr {}", env!("CARGO_PKG_VERSION"))));
        assert!(version.contains("git_sha="));
        assert!(version.contains("rustc=\""));
    }

    #[test]
    fn config_set_normalizes_codex_milestone_aliases_before_persisting() {
        let path = env::temp_dir().join(format!(
            "agentactr-config-set-canonical-test-{}-{}.toml",
            std::process::id(),
            new_run_id("config-canonical")
        ));
        let original = render_agentactr_toml(&AgentactrConfig::strict_defaults("OWNER/REPO"));
        fs::write(&path, original).unwrap();

        set_config_value(path.to_str().unwrap(), "codex.mode", "exec-json").unwrap();
        set_config_value(
            path.to_str().unwrap(),
            "codex.app_server_experimental_api",
            "true",
        )
        .unwrap();
        set_config_value(path.to_str().unwrap(), "codex.app_server_transport", "ws").unwrap();
        set_config_value(path.to_str().unwrap(), "codex.sdk_bridge", "ts").unwrap();
        set_config_value(path.to_str().unwrap(), "codex.fallback_mode", "exec-json").unwrap();

        let updated = fs::read_to_string(&path).unwrap();
        assert_eq!(
            find_config_value(&updated, "codex.mode").as_deref(),
            Some("cli_json")
        );
        assert_eq!(
            find_config_value(&updated, "codex.app_server_transport").as_deref(),
            Some("websocket")
        );
        assert_eq!(
            find_config_value(&updated, "codex.sdk_bridge").as_deref(),
            Some("typescript")
        );
        assert_eq!(
            find_config_value(&updated, "codex.fallback_mode").as_deref(),
            Some("cli_json")
        );
        let _ = fs::remove_file(path);
    }
}
