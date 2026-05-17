use agentactr_core::{AgentactrConfig, CodexAuthMode, DomainProfile, DomainQualityGate};

use crate::discovery::RepoInspection;
use crate::domains::domain_matches_selection;

#[derive(Clone, Debug, Default)]
pub struct DetectedCredentials {
    pub github_token: bool,
    pub gh_token: bool,
    pub configured_github_token: bool,
    pub google_api_key: bool,
    pub hf_token: bool,
    pub openai_api_key: bool,
    pub codex_google_mcp: bool,
    pub codex_hf_mcp: bool,
    pub codex_github_remote_mcp: bool,
}

impl DetectedCredentials {
    pub fn github_any(&self) -> bool {
        self.github_token || self.gh_token || self.configured_github_token
    }
}

fn q(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn bool_s(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

fn select_github_bearer_token_env_var(
    config: &AgentactrConfig,
    creds: &DetectedCredentials,
) -> String {
    if creds.configured_github_token {
        config.tracker.token_env.clone()
    } else if creds.github_token {
        "GITHUB_TOKEN".to_string()
    } else if creds.gh_token {
        "GH_TOKEN".to_string()
    } else {
        "GITHUB_TOKEN".to_string()
    }
}

fn arr(values: &[String]) -> String {
    let rendered = values.iter().map(|v| q(v)).collect::<Vec<_>>().join(", ");
    format!("[{rendered}]")
}

fn possible_values_comment(dotted_key: &str) -> Option<&'static str> {
    let values = match dotted_key {
        "tracker.kind" => "github",
        "codex.mode" => "cli_json, app_server, codex_sdk",
        "codex.approval_policy" => "never, on-request",
        "codex.sandbox_mode" => "read-only, workspace-write, danger-full-access",
        "codex.network" => "off, on",
        "codex.model_reasoning_effort" => "minimal, low, medium, high, xhigh",
        "codex.auth_mode" => "auto, chatgpt, api_key",
        "codex.app_server_transport" => "stdio, websocket",
        "codex.app_server_experimental_api" => "true, false",
        "codex.sdk_bridge" => "typescript",
        "codex.fallback_mode" => "cli_json",
        "human_intervention.mode" => "fail_closed, interactive, review_required",
        "human_intervention.on_codex_approval_request" => "fail_run, prompt_operator",
        "human_intervention.on_ambiguous_diff" => "fail_quality_gate, prompt_operator",
        "human_intervention.on_review_disagreement" => "fail_quality_gate, prompt_operator",
        "human_intervention.on_missing_codex_auth" => "fail_startup, prompt_operator",
        "human_intervention.on_missing_github_token" => "fail_startup, prompt_operator",
        "human_intervention.run_start_banner" => "true, false",
        "human_intervention.print_override_steps" => "true, false",
        "github.finalization" => "automatic_after_quality_gates, require_human_review, disabled",
        "github.standard_label_policy" => "ensure_on_issue_create, disabled",
        "github.project_automation" => "disabled, ensure_on_issue_create",
        "mcp.default_policy" => "auto_setup_detected_credentials, disabled",
        "mcp.remote_research_servers" => "auto_enable_when_credentials_detected, disabled",
        "mcp.remote_github_read_tools" => "auto_enable_when_token_detected, disabled",
        "mcp.remote_github_write_tools" => "disabled_by_default, explicit_only",
        "mcp.openai_developer_docs" => "auto_enable_no_auth, disabled",
        "mcp.google_developer_api" => "auto_enable_with_GOOGLE_API_KEY, disabled",
        "mcp.huggingface" => "auto_enable_with_oauth_or_HF_TOKEN, disabled",
        "mcp.github_remote" => "auto_enable_read_only_with_token, disabled",
        "mcp.fail_on_required_mcp_missing" => "true, false",
        "repository.empty_repo_policy" => "fail_closed_unless_stack_declared, allow_empty",
        "repository.declared_primary_stack" => "auto, rust, typescript, golang, python",
        "repository.allowed_bootstrap" => "explicit_only, disabled",
        "repository.bootstrap_prereqs" => "minimal_for_declared_stack, none",
        "repository.fail_on_low_confidence_stack_detection" => "true, false",
        "vcs.kind" => "git",
        "vcs.workspace_strategy" => "worktree",
        "vcs.record_base_commit" => "true, false",
        "vcs.fail_on_dirty_source_checkout" => "true, false",
        "vcs.copy_runtime_config_to_worktree" => "true, false",
        "vcs.detect_cross_issue_file_overlap" => "true, false",
        "vcs.overlap_policy" => "fail_closed, warn, disabled",
        "quality.profile" => "strict, standard, minimal",
        "quality.pre_commit_mode" => "required, disabled",
        "quality.technology_detection" => "auto, declared_only",
        "quality.domains" => "auto, language, iac, database, streaming, storage, communications, observability, security, resilience, tenancy, service_patterns, api_contracts.protobuf, rpc.grpc",
        "quality.domain_gate_opt_ins" => "domain id, gate name, domain:gate, domain:*, all",
        "quality.run_existing_pre_commit_config" => "true, false",
        "quality.fail_on_missing_toolchain" => "true, false",
        "quality.fail_on_untracked_generated_files" => "true, false",
        "quality.allow_test_omission_reason" => "true, false",
        "quality.dependency_checks" => "true, false",
        "quality.architecture_checks" => "true, false",
        "quality.tool_pinning" => "required_for_strict, optional, disabled",
        "quality.typescript.enabled" => "auto, true, false",
        "quality.typescript.package_manager" => "auto, npm, pnpm, yarn, bun",
        "quality.typescript.install" => "frozen, skip",
        "quality.typescript.run_only_existing_scripts" => "true, false",
        "quality.rust.enabled" => "auto, true, false",
        "quality.golang.enabled" => "auto, true, false",
        "quality.python.enabled" => "auto, true, false",
        "architecture.domains" => "auto, detected_only, declared_only",
        "architecture.fail_on_domain_drift" => "true, false",
        "templates.enabled_domains" => "auto, detected_only, declared_only",
        "templates.framework_profile" => "auto, nextjs, none",
        "templates.agents_policy" => "generate_when_absent, artifact_only, disabled",
        "commit.mode" => "local_after_quality_gates, disabled",
        "commit.signoff" => "true, false",
        "commit.gpg_sign" => "inherit, true, false",
        "merge.mode" => "disabled, local, pull_request",
        "merge.push" => "disabled, enabled",
        "merge.strategy" => "fast_forward_only",
        "merge.require_clean_rebase" => "true, false",
        "merge.require_no_cross_issue_overlap" => "true, false",
        "merge.require_human_review_for_merge" => "true, false",
        "workspace.keep_successful" => "true, false",
        "workspace.keep_failed" => "true, false",
        "spawn.enabled" => "true, false",
        "spawn.allow_parallel_read_only" => "true, false",
        "spawn.allow_parallel_writers" => "true, false",
        "spawn.strategy" => "budget_aware_one_writer",
        "spawn.artifact_handoff" => "refs_summaries_and_digests",
        "spawn.pause_on_memory_pressure" => "true, false",
        "execution.backend" => {
            "auto, native_linux_cgroup_v2, docker_linux_vm, native_macos_observe_only, observe_only"
        }
        "execution.strict_memory_required" => "true, false",
        "execution.docker.pull_policy" => "if_missing, always, never",
        "execution.docker.network" => "bridge, none, host",
        "execution.docker.workspace_mount" => "rw, ro",
        "execution.docker.artifact_mount" => "rw, ro",
        "execution.docker.remove_containers" => "true, false",
        "linux_memory.enabled" => "true, false",
        "linux_memory.cgroup_root" => "auto, absolute cgroup v2 path",
        "linux_memory.mode" => "enforce_on_linux_observe_elsewhere, observe_only",
        "linux_memory.cgroup_v2_required" => "true, false",
        "linux_memory.psi_required" => "true, false",
        "linux_memory.setrlimit_address_space" => "disabled, memory size such as 4G",
        "linux_memory.setrlimit_file_size" => "disabled, memory size such as 1G",
        "linux_memory.kill_policy" => "cancel_lowest_priority_subagent, fail_run, observe",
        "linux_memory.oom_policy" => "fail_run_preserve_debug_bundle, fail_agent, observe",
        "observability.otel_enabled" => "true, false",
        "observability.redact_secrets" => "true, false",
        _ => return None,
    };
    Some(values)
}

pub fn annotate_agentactr_toml_possible_values(content: &str) -> String {
    annotate_toml_possible_values(content, possible_values_comment)
}

fn codex_possible_values_comment(dotted_key: &str) -> Option<&'static str> {
    let values = match dotted_key {
        "approval_policy" => "never, on-request",
        "sandbox_mode" => "read-only, workspace-write, danger-full-access",
        "model_reasoning_effort" => "minimal, low, medium, high, xhigh",
        "forced_login_method" => "chatgpt, api",
        "sandbox_workspace_write.network_access" => "true, false",
        "features.multi_agent" => "true, false",
        "mcp_servers.agentactr.required" => "true, false",
        "mcp_servers.openaiDeveloperDocs.enabled" => "true, false",
        "mcp_servers.openaiDeveloperDocs.required" => "true, false",
        "mcp_servers.GoogleDeveloperAPI.enabled" => "true, false",
        "mcp_servers.GoogleDeveloperAPI.required" => "true, false",
        "mcp_servers.hf-mcp-server.enabled" => "true, false",
        "mcp_servers.hf-mcp-server.required" => "true, false",
        "mcp_servers.github_remote.enabled" => "true, false",
        "mcp_servers.github_remote.required" => "true, false",
        _ => return None,
    };
    Some(values)
}

fn annotate_codex_toml_possible_values(content: &str) -> String {
    annotate_toml_possible_values(content, codex_possible_values_comment)
}

fn annotate_toml_possible_values(
    content: &str,
    possible_values: fn(&str) -> Option<&'static str>,
) -> String {
    let mut section = String::new();
    let mut rendered = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed.trim_matches(&['[', ']'][..]).to_string();
            rendered.push(line.to_string());
            continue;
        }
        let Some((key, _)) = trimmed.split_once('=') else {
            rendered.push(line.to_string());
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.starts_with('#') {
            rendered.push(line.to_string());
            continue;
        }
        let dotted_key = if section.is_empty() {
            key.to_string()
        } else {
            format!("{section}.{key}")
        };
        let Some(values) = possible_values(&dotted_key) else {
            rendered.push(line.to_string());
            continue;
        };
        let base = line
            .split(" # possible values:")
            .next()
            .unwrap_or(line)
            .trim_end();
        rendered.push(format!("{base} # possible values: {values}"));
    }
    let mut output = rendered.join("\n");
    if content.ends_with('\n') {
        output.push('\n');
    }
    output
}

pub fn render_agentactr_toml(config: &AgentactrConfig) -> String {
    let rendered = format!(
        r#"[tracker]
kind = {tracker_kind}
repo = {repo}
token_env = {token_env}
github_api_version = {github_api_version}
active_labels = {active_labels}
ignore_labels = {ignore_labels}
claim_label = {claim_label}
running_label = {running_label}
failed_label = {failed_label}
done_label = {done_label}

[codex]
command = {codex_command}
mode = {codex_mode}
profile = {codex_profile}
approval_policy = {approval_policy}
sandbox_mode = {sandbox_mode}
network = {network}
default_model = {default_model}
model_reasoning_effort = {model_reasoning_effort}
auth_mode = {auth_mode}
openai_api_key_env = {openai_api_key_env}
app_server_transport = {app_server_transport}
app_server_experimental_api = {app_server_experimental_api}
sdk_bridge = {sdk_bridge}
fallback_mode = {fallback_mode}

[human_intervention]
mode = {human_mode}
on_codex_approval_request = {on_codex_approval_request}
on_ambiguous_diff = {on_ambiguous_diff}
on_review_disagreement = {on_review_disagreement}
on_missing_codex_auth = {on_missing_codex_auth}
on_missing_github_token = {on_missing_github_token}
run_start_banner = {run_start_banner}
print_override_steps = {print_override_steps}

[github]
finalization = {github_finalization}
standard_label_policy = {github_standard_label_policy}
project_automation = {github_project_automation}
project_owner = {github_project_owner}
project_number = {github_project_number}
project_title = {github_project_title}
project_priority_field = {github_project_priority_field}
project_size_field = {github_project_size_field}

[mcp]
default_policy = {mcp_default_policy}
remote_research_servers = {mcp_remote_research_servers}
remote_github_read_tools = {mcp_remote_github_read_tools}
remote_github_write_tools = {mcp_remote_github_write_tools}
openai_developer_docs = {mcp_openai_developer_docs}
google_developer_api = {mcp_google_developer_api}
huggingface = {mcp_huggingface}
github_remote = {mcp_github_remote}
fail_on_required_mcp_missing = {mcp_fail_required}

[repository]
empty_repo_policy = {empty_repo_policy}
declared_primary_stack = {declared_primary_stack}
allowed_bootstrap = {allowed_bootstrap}
bootstrap_prereqs = {bootstrap_prereqs}
fail_on_low_confidence_stack_detection = {fail_on_low_confidence_stack_detection}

[vcs]
kind = {vcs_kind}
workspace_strategy = {workspace_strategy}
base_ref = {base_ref}
worktree_root = {worktree_root}
branch_template = {branch_template}
record_base_commit = {record_base_commit}
fail_on_dirty_source_checkout = {fail_on_dirty_source_checkout}
copy_runtime_config_to_worktree = {copy_runtime_config_to_worktree}
detect_cross_issue_file_overlap = {detect_cross_issue_file_overlap}
overlap_policy = {overlap_policy}

[quality]
profile = {quality_profile}
pre_commit_mode = {pre_commit_mode}
technology_detection = {technology_detection}
domains = {quality_domains}
domain_gate_opt_ins = {quality_domain_gate_opt_ins}
run_existing_pre_commit_config = {run_existing_pre_commit_config}
fail_on_missing_toolchain = {fail_on_missing_toolchain}
fail_on_untracked_generated_files = {fail_on_untracked_generated_files}
allow_test_omission_reason = {allow_test_omission_reason}
artifact_dir = {artifact_dir}
dependency_checks = {dependency_checks}
architecture_checks = {architecture_checks}
tool_pinning = {tool_pinning}

[quality.typescript]
enabled = "auto"
package_manager = "auto"
install = "frozen"
node_version = "nvmrc_or_node_version_required"
bun = "pinned_when_used"
biome = "pinned_when_used_or_config_present"
zod = "required_for_new_boundary_validation"
framework_detection = ["vite", "next", "remix", "sveltekit", "astro"]
commands = ["install", "biome", "lint", "typecheck", "test", "build", "framework_smoke"]
run_only_existing_scripts = true

[quality.rust]
enabled = "auto"
commands = [
  "cargo fmt --all -- --check",
  "cargo clippy --workspace --all-targets --all-features -- -D warnings",
  "cargo nextest run --workspace --all-features",
  "cargo test --doc --workspace --all-features",
  "cargo deny check",
  "cargo machete"
]
public_library_extra = ["cargo semver-checks"]
unsafe_parser_network_input_heavy_extra = ["cargo miri test", "cargo fuzz run"]

[quality.golang]
enabled = "auto"
golangci_lint = "pinned_required"
module_files = "go_mod_and_go_sum_required"
commands = [
  "gofmt-check",
  "go mod verify",
  "go mod tidy-check",
  "go vet ./...",
  "golangci-lint run",
  "govulncheck ./...",
  "go test ./..."
]
architecture_checks = ["golangci-lint-depguard", "import-boundary-check", "package-cycle-check"]

[quality.python]
enabled = "auto"
package_manager = "uv_preferred"
python_version = "requires_pin"
dependency_lock = "required"
commands = [
  "uv sync --frozen",
  "uv run ruff format --check .",
  "uv run ruff check .",
  "uv run pyright",
  "uv run pytest",
  "uv run pip-audit",
  "uv run deptry ."
]
optional_commands = [
  "uv run mypy",
  "uv run coverage run -m pytest",
  "uv run coverage report --fail-under CONFIGURED_THRESHOLD",
  "uv run bandit -r .",
  "uv run interrogate ."
]
library_extra = ["uv build", "uv run twine check dist/*"]
service_extra = ["contract-tests", "openapi-schema-check-if_present"]
parser_network_input_heavy_extra = ["uv run bandit -r .", "uv run semgrep --config auto", "uv run pytest --hypothesis-profile ci"]
architecture_checks = ["import-linter-if_config_present", "layer-boundary-review"]

[architecture]
domains = {architecture_domains}
domain_graph_artifact = {domain_graph_artifact}
fail_on_domain_drift = {fail_on_domain_drift}

[templates]
enabled_domains = {templates_enabled_domains}
framework_profile = {templates_framework_profile}
agents_policy = {templates_agents_policy}

[commit]
mode = {commit_mode}
signoff = {commit_signoff}
gpg_sign = {commit_gpg_sign}
message_template = {commit_message_template}
required_trailers = {commit_required_trailers}

[merge]
mode = {merge_mode}
push = {merge_push}
strategy = {merge_strategy}
require_clean_rebase = {require_clean_rebase}
require_no_cross_issue_overlap = {require_no_cross_issue_overlap}
require_human_review_for_merge = {require_human_review_for_merge}

[workspace]
root = {workspace_root}
keep_successful = {keep_successful}
keep_failed = {keep_failed}

[scheduling]
poll_interval_ms = {poll_interval_ms}
max_concurrent_issue_runs = {max_concurrent_issue_runs}
lease_ttl_ms = {lease_ttl_ms}
max_retries = {max_retries}

[spawn]
enabled = {spawn_enabled}
max_child_agents_per_issue = {max_child_agents_per_issue}
max_spawn_depth = {max_spawn_depth}
allow_parallel_read_only = {allow_parallel_read_only}
allow_parallel_writers = {allow_parallel_writers}
strategy = {spawn_strategy}
max_total_uncached_input_tokens = {max_total_uncached_input_tokens}
max_child_uncached_input_tokens = {max_child_uncached_input_tokens}
max_child_output_tokens = {max_child_output_tokens}
artifact_handoff = {artifact_handoff}
pause_on_memory_pressure = {pause_on_memory_pressure}

[execution]
backend = {execution_backend}
strict_memory_required = {execution_strict_memory_required}

[execution.docker]
command = {execution_docker_command}
image = {execution_docker_image}
pull_policy = {execution_docker_pull_policy}
network = {execution_docker_network}
workspace_mount = {execution_docker_workspace_mount}
artifact_mount = {execution_docker_artifact_mount}
remove_containers = {execution_docker_remove_containers}
container_prefix = {execution_docker_container_prefix}

[linux_memory]
enabled = {linux_memory_enabled}
cgroup_root = {cgroup_root}
root_group = {root_group}
mode = {linux_memory_mode}
cgroup_v2_required = {linux_memory_cgroup_v2_required}
psi_required = {linux_memory_psi_required}
per_issue_memory_high = {per_issue_memory_high}
per_issue_memory_max = {per_issue_memory_max}
per_agent_memory_high = {per_agent_memory_high}
per_agent_memory_max = {per_agent_memory_max}
psi_memory_some_threshold_us = {psi_memory_some_threshold_us}
psi_memory_window_us = {psi_memory_window_us}
oom_score_adj = {oom_score_adj}
setrlimit_address_space = {setrlimit_address_space}
setrlimit_file_size = {setrlimit_file_size}
kill_policy = {kill_policy}
oom_policy = {oom_policy}

[observability]
jsonl = {jsonl}
sqlite = {sqlite}
artifact_root = {artifact_root}
otel_enabled = {otel_enabled}
otel_endpoint = {otel_endpoint}
debug_bundle_root = {debug_bundle_root}
redact_secrets = {redact_secrets}
"#,
        tracker_kind = q(&config.tracker.kind),
        repo = q(&config.tracker.repo),
        token_env = q(&config.tracker.token_env),
        github_api_version = q(&config.tracker.github_api_version),
        active_labels = arr(&config.tracker.active_labels),
        ignore_labels = arr(&config.tracker.ignore_labels),
        claim_label = q(&config.tracker.claim_label),
        running_label = q(&config.tracker.running_label),
        failed_label = q(&config.tracker.failed_label),
        done_label = q(&config.tracker.done_label),
        codex_command = q(&config.codex.command),
        codex_mode = q(&config.codex.mode),
        codex_profile = q(&config.codex.profile),
        approval_policy = q(&config.codex.approval_policy),
        sandbox_mode = q(&config.codex.sandbox_mode),
        network = q(&config.codex.network),
        default_model = q(&config.codex.default_model),
        model_reasoning_effort = q(&config.codex.model_reasoning_effort),
        auth_mode = q(config.codex.auth_mode.as_str()),
        openai_api_key_env = q(&config.codex.openai_api_key_env),
        app_server_transport = q(&config.codex.app_server_transport),
        app_server_experimental_api = bool_s(config.codex.app_server_experimental_api),
        sdk_bridge = q(&config.codex.sdk_bridge),
        fallback_mode = q(&config.codex.fallback_mode),
        human_mode = q(&config.human_intervention.mode),
        on_codex_approval_request = q(&config.human_intervention.on_codex_approval_request),
        on_ambiguous_diff = q(&config.human_intervention.on_ambiguous_diff),
        on_review_disagreement = q(&config.human_intervention.on_review_disagreement),
        on_missing_codex_auth = q(&config.human_intervention.on_missing_codex_auth),
        on_missing_github_token = q(&config.human_intervention.on_missing_github_token),
        run_start_banner = bool_s(config.human_intervention.run_start_banner),
        print_override_steps = bool_s(config.human_intervention.print_override_steps),
        github_finalization = q(&config.github.finalization),
        github_standard_label_policy = q(&config.github.standard_label_policy),
        github_project_automation = q(&config.github.project_automation),
        github_project_owner = q(&config.github.project_owner),
        github_project_number = config.github.project_number,
        github_project_title = q(&config.github.project_title),
        github_project_priority_field = q(&config.github.project_priority_field),
        github_project_size_field = q(&config.github.project_size_field),
        mcp_default_policy = q(&config.mcp.default_policy),
        mcp_remote_research_servers = q(&config.mcp.remote_research_servers),
        mcp_remote_github_read_tools = q(&config.mcp.remote_github_read_tools),
        mcp_remote_github_write_tools = q(&config.mcp.remote_github_write_tools),
        mcp_openai_developer_docs = q(&config.mcp.openai_developer_docs),
        mcp_google_developer_api = q(&config.mcp.google_developer_api),
        mcp_huggingface = q(&config.mcp.huggingface),
        mcp_github_remote = q(&config.mcp.github_remote),
        mcp_fail_required = bool_s(config.mcp.fail_on_required_mcp_missing),
        empty_repo_policy = q(&config.repository.empty_repo_policy),
        declared_primary_stack = q(&config.repository.declared_primary_stack),
        allowed_bootstrap = q(&config.repository.allowed_bootstrap),
        bootstrap_prereqs = q(&config.repository.bootstrap_prereqs),
        fail_on_low_confidence_stack_detection =
            bool_s(config.repository.fail_on_low_confidence_stack_detection),
        vcs_kind = q(&config.vcs.kind),
        workspace_strategy = q(&config.vcs.workspace_strategy),
        base_ref = q(&config.vcs.base_ref),
        worktree_root = q(&config.vcs.worktree_root),
        branch_template = q(&config.vcs.branch_template),
        record_base_commit = bool_s(config.vcs.record_base_commit),
        fail_on_dirty_source_checkout = bool_s(config.vcs.fail_on_dirty_source_checkout),
        copy_runtime_config_to_worktree = bool_s(config.vcs.copy_runtime_config_to_worktree),
        detect_cross_issue_file_overlap = bool_s(config.vcs.detect_cross_issue_file_overlap),
        overlap_policy = q(&config.vcs.overlap_policy),
        quality_profile = q(&config.quality.profile),
        pre_commit_mode = q(&config.quality.pre_commit_mode),
        technology_detection = q(&config.quality.technology_detection),
        quality_domains = arr(&config.quality.domains),
        quality_domain_gate_opt_ins = arr(&config.quality.domain_gate_opt_ins),
        run_existing_pre_commit_config = bool_s(config.quality.run_existing_pre_commit_config),
        fail_on_missing_toolchain = bool_s(config.quality.fail_on_missing_toolchain),
        fail_on_untracked_generated_files =
            bool_s(config.quality.fail_on_untracked_generated_files),
        allow_test_omission_reason = bool_s(config.quality.allow_test_omission_reason),
        artifact_dir = q(&config.quality.artifact_dir),
        dependency_checks = bool_s(config.quality.dependency_checks),
        architecture_checks = bool_s(config.quality.architecture_checks),
        tool_pinning = q(&config.quality.tool_pinning),
        architecture_domains = arr(&config.architecture.domains),
        domain_graph_artifact = q(&config.architecture.domain_graph_artifact),
        fail_on_domain_drift = bool_s(config.architecture.fail_on_domain_drift),
        templates_enabled_domains = arr(&config.templates.enabled_domains),
        templates_framework_profile = q(&config.templates.framework_profile),
        templates_agents_policy = q(&config.templates.agents_policy),
        commit_mode = q(&config.commit.mode),
        commit_signoff = bool_s(config.commit.signoff),
        commit_gpg_sign = q(&config.commit.gpg_sign),
        commit_message_template = q(&config.commit.message_template),
        commit_required_trailers = arr(&config.commit.required_trailers),
        merge_mode = q(&config.merge.mode),
        merge_push = q(&config.merge.push),
        merge_strategy = q(&config.merge.strategy),
        require_clean_rebase = bool_s(config.merge.require_clean_rebase),
        require_no_cross_issue_overlap = bool_s(config.merge.require_no_cross_issue_overlap),
        require_human_review_for_merge = bool_s(config.merge.require_human_review_for_merge),
        workspace_root = q(&config.workspace.root),
        keep_successful = bool_s(config.workspace.keep_successful),
        keep_failed = bool_s(config.workspace.keep_failed),
        poll_interval_ms = config.scheduling.poll_interval_ms,
        max_concurrent_issue_runs = config.scheduling.max_concurrent_issue_runs,
        lease_ttl_ms = config.scheduling.lease_ttl_ms,
        max_retries = config.scheduling.max_retries,
        spawn_enabled = bool_s(config.spawn.enabled),
        max_child_agents_per_issue = config.spawn.max_child_agents_per_issue,
        max_spawn_depth = config.spawn.max_spawn_depth,
        allow_parallel_read_only = bool_s(config.spawn.allow_parallel_read_only),
        allow_parallel_writers = bool_s(config.spawn.allow_parallel_writers),
        spawn_strategy = q(&config.spawn.strategy),
        max_total_uncached_input_tokens = config.spawn.max_total_uncached_input_tokens,
        max_child_uncached_input_tokens = config.spawn.max_child_uncached_input_tokens,
        max_child_output_tokens = config.spawn.max_child_output_tokens,
        artifact_handoff = q(&config.spawn.artifact_handoff),
        pause_on_memory_pressure = bool_s(config.spawn.pause_on_memory_pressure),
        execution_backend = q(&config.execution.backend),
        execution_strict_memory_required = bool_s(config.execution.strict_memory_required),
        execution_docker_command = q(&config.execution.docker.command),
        execution_docker_image = q(&config.execution.docker.image),
        execution_docker_pull_policy = q(&config.execution.docker.pull_policy),
        execution_docker_network = q(&config.execution.docker.network),
        execution_docker_workspace_mount = q(&config.execution.docker.workspace_mount),
        execution_docker_artifact_mount = q(&config.execution.docker.artifact_mount),
        execution_docker_remove_containers = bool_s(config.execution.docker.remove_containers),
        execution_docker_container_prefix = q(&config.execution.docker.container_prefix),
        linux_memory_enabled = bool_s(config.linux_memory.enabled),
        cgroup_root = q(&config.linux_memory.cgroup_root),
        root_group = q(&config.linux_memory.root_group),
        linux_memory_mode = q(&config.linux_memory.mode),
        linux_memory_cgroup_v2_required = bool_s(config.linux_memory.cgroup_v2_required),
        linux_memory_psi_required = bool_s(config.linux_memory.psi_required),
        per_issue_memory_high = q(&config.linux_memory.per_issue_memory_high),
        per_issue_memory_max = q(&config.linux_memory.per_issue_memory_max),
        per_agent_memory_high = q(&config.linux_memory.per_agent_memory_high),
        per_agent_memory_max = q(&config.linux_memory.per_agent_memory_max),
        psi_memory_some_threshold_us = config.linux_memory.psi_memory_some_threshold_us,
        psi_memory_window_us = config.linux_memory.psi_memory_window_us,
        oom_score_adj = config.linux_memory.oom_score_adj,
        setrlimit_address_space = q(&config.linux_memory.setrlimit_address_space),
        setrlimit_file_size = q(&config.linux_memory.setrlimit_file_size),
        kill_policy = q(&config.linux_memory.kill_policy),
        oom_policy = q(&config.linux_memory.oom_policy),
        jsonl = q(&config.observability.jsonl),
        sqlite = q(&config.observability.sqlite),
        artifact_root = q(&config.observability.artifact_root),
        otel_enabled = bool_s(config.observability.otel_enabled),
        otel_endpoint = q(&config.observability.otel_endpoint),
        debug_bundle_root = q(&config.observability.debug_bundle_root),
        redact_secrets = bool_s(config.observability.redact_secrets),
    );
    annotate_agentactr_toml_possible_values(&rendered)
}

pub fn render_codex_config_toml(config: &AgentactrConfig, creds: &DetectedCredentials) -> String {
    let forced_login_method = match config.codex.auth_mode {
        CodexAuthMode::Auto => None,
        CodexAuthMode::ChatGptSubscription => Some("chatgpt"),
        CodexAuthMode::OpenAiApiKey => Some("api"),
    };
    let forced_login = forced_login_method
        .map(|m| format!("forced_login_method = {}\n", q(m)))
        .unwrap_or_default();

    let google_enabled = creds.google_api_key || creds.codex_google_mcp;
    let hf_enabled = creds.hf_token || creds.codex_hf_mcp;
    let github_remote_enabled = creds.github_any() || creds.codex_github_remote_mcp;
    let github_bearer_token_env_var = select_github_bearer_token_env_var(config, creds);
    let github_bearer_token_line = if creds.github_any() {
        format!(
            "bearer_token_env_var = {}\n",
            q(&github_bearer_token_env_var)
        )
    } else {
        String::new()
    };
    let network_access = matches!(
        config.codex.network.trim().to_ascii_lowercase().as_str(),
        "on" | "true" | "enabled" | "allow" | "allowed"
    );

    let rendered = format!(
        r#"approval_policy = {approval_policy}
sandbox_mode = {sandbox_mode}
model_reasoning_effort = {model_reasoning_effort}
{forced_login}
[sandbox_workspace_write]
network_access = {network_access}

[features]
multi_agent = true

[agents]
max_depth = 1
max_threads = 6
job_max_runtime_seconds = 1800

[agents.explorer]
description = "Read-only codebase exploration and scoped findings."

[agents.reviewer]
description = "Read-only code review and risk finding."

[mcp_servers.agentactr]
command = "agentactr"
args = ["mcp", "serve"]
cwd = "."
env_vars = ["AGENTACTR_ARTIFACT_ROOT", "AGENTACTR_REPO_ROOT", "AGENTACTR_RUN_ID", "AGENTACTR_AGENT_RUN_ID", "AGENTACTR_TRACE_PATH", "AGENTACTR_CONTEXT_MANIFEST"]
required = true
startup_timeout_sec = 10
tool_timeout_sec = 60
enabled_tools = [
  "agentactr.issue.read",
  "agentactr.run.status",
  "agentactr.trace.read",
  "agentactr.artifact.read",
  "agentactr.vcs.status",
  "agentactr.quality.report",
  "agentactr.memory.status",
  "agentactr.policy.read"
]

[mcp_servers.openaiDeveloperDocs]
url = "https://developers.openai.com/mcp"
enabled = true
required = false
startup_timeout_sec = 10
tool_timeout_sec = 60
enabled_tools = ["search_openai_docs", "fetch_openai_doc", "list_openai_docs", "list_api_endpoints", "get_openapi_spec"]

[mcp_servers.GoogleDeveloperAPI]
url = "https://developerknowledge.googleapis.com/mcp"
enabled = {google_enabled}
required = false
startup_timeout_sec = 10
tool_timeout_sec = 60
env_http_headers = {{ "X-Goog-Api-Key" = "GOOGLE_API_KEY" }}
http_headers = {{ "Accept" = "application/json" }}
enabled_tools = ["answer_query", "get_documents", "search_documents"]

[mcp_servers.hf-mcp-server]
url = "https://huggingface.co/mcp?login"
enabled = {hf_enabled}
required = false
startup_timeout_sec = 10
tool_timeout_sec = 60
bearer_token_env_var = "HF_TOKEN"
enabled_tools = [
  "hf_doc_fetch",
  "hf_doc_search",
  "hf_hub_query",
  "hf_whoami",
  "hub_repo_details",
  "hub_repo_search",
  "paper_search",
  "space_search"
]

[mcp_servers.github_remote]
url = "https://api.githubcopilot.com/mcp/"
enabled = {github_remote_enabled}
required = false
startup_timeout_sec = 10
tool_timeout_sec = 60
{github_bearer_token_line}enabled_tools = [
  "get_commit",
  "get_file_contents",
  "get_label",
  "get_latest_release",
  "get_me",
  "get_release_by_tag",
  "get_tag",
  "issue_read",
  "list_branches",
  "list_commits",
  "list_issues",
  "list_pull_requests",
  "list_releases",
  "list_tags",
  "pull_request_read",
  "search_code",
  "search_issues",
  "search_pull_requests",
  "search_repositories",
  "search_users"
]
disabled_tools = [
  "add_comment_to_pending_review",
  "add_issue_comment",
  "add_reply_to_pull_request_comment",
  "create_branch",
  "create_or_update_file",
  "create_pull_request",
  "create_repository",
  "delete_file",
  "fork_repository",
  "issue_write",
  "merge_pull_request",
  "pull_request_review_write",
  "push_files",
  "request_copilot_review",
  "run_secret_scanning",
  "sub_issue_write",
  "update_pull_request",
  "update_pull_request_branch"
]
"#,
        approval_policy = q(&config.codex.approval_policy.replace('_', "-")),
        sandbox_mode = q(&config.codex.sandbox_mode),
        model_reasoning_effort = q(&config.codex.model_reasoning_effort),
        forced_login = forced_login,
        google_enabled = bool_s(google_enabled),
        hf_enabled = bool_s(hf_enabled),
        github_remote_enabled = bool_s(github_remote_enabled),
        github_bearer_token_line = github_bearer_token_line,
        network_access = bool_s(network_access),
    );
    annotate_codex_toml_possible_values(&rendered)
}

pub fn render_workflow_md() -> String {
    r#"# agentactr Workflow

Default mode is unattended and fail-closed.

Required operator setup:

```bash
codex login
export GITHUB_TOKEN=...
agentactr doctor --fix-codex-config
```

For API-key based Codex auth:

```bash
export CODEX_API_KEY=...
agentactr init --yes --repo OWNER/REPO --codex-auth api-key
```

Strict defaults:

- Codex approval policy: never
- human intervention: fail_closed
- Git worktree isolation
- pre-commit required
- local commit only after quality gates
- no push or merge by default
- remote GitHub MCP write tools disabled
"#
    .to_string()
}

pub fn render_agents_md(config: &AgentactrConfig, inspection: &RepoInspection) -> String {
    let rendered_domains = filter_template_domains(config, inspection);
    let domains = if rendered_domains.is_empty() {
        "- none detected; declare domains explicitly before blank-project automation".to_string()
    } else {
        rendered_domains
            .iter()
            .map(|profile| {
                format!(
                    "- {} ({}, confidence={})",
                    profile.id, profile.kind, profile.confidence
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let rendered_gates = inspection
        .domain_quality_plan
        .iter()
        .filter(|gate| domain_enabled_for_templates(config, &gate.domain))
        .collect::<Vec<_>>();
    let gates = if rendered_gates.is_empty() {
        "- none".to_string()
    } else {
        rendered_gates
            .iter()
            .take(40)
            .map(|gate| match &gate.command {
                Some(command) => format!("- {} [{}] command: {}", gate.name, gate.domain, command),
                None => format!(
                    "- {} [{}] finding-only: {}",
                    gate.name,
                    gate.domain,
                    gate.setup_guidance.join("; ")
                ),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let platform_rules = render_provider_platform_rules(&rendered_domains, &rendered_gates);
    format!(
        r#"# AGENTS.md instructions for this repository

Follow strict SOLID principles across application code, infrastructure code, agentactr core integrations, SDK boundaries, and CLI/runtime adapters.

The `specs_agentactrSDK.md` specification is the architectural source of truth for agentactr behavior. Update it whenever architectural corrections or contract changes are introduced.

Enforce:

- Hexagonal/Clean Architecture
- Dependency Inversion
- Interface segregation
- Explicit domain boundaries
- Transport isolation
- Async-safe runtime patterns
- Strong typing over dynamic behavior
- Deterministic error handling
- Structured observability
- Repository/service separation
- Configuration-driven composition
- Testability-first abstractions
- Secure-by-default implementations

Default implementations must remain compatible with Codex, Codex App Server, Codex SDK, and GitHub integrations. Concrete providers, tools, clouds, and trackers must stay behind ports/adapters or template profiles.

Detected repository stack:

- primary_stack: {primary_stack}
- quality_profile: {quality_profile}
- tracker: {tracker_kind}

Detected domain profiles:

{domains}

Domain quality gates and architecture checks:

{gates}

Provider and platform rules:

{platform_rules}

Before introducing architectural decisions, refactors, dependency changes, or protocol changes:

- Verify through MCP where available.
- Verify through Skills where relevant.
- Verify through authoritative documentation for current protocol/cloud/tool behavior.

"#,
        primary_stack = inspection.primary_stack.as_str(),
        quality_profile = inspection.selected_quality_profile,
        tracker_kind = config.tracker.kind,
        domains = domains,
        gates = gates,
        platform_rules = platform_rules,
    )
}

fn render_provider_platform_rules(
    domains: &[&DomainProfile],
    gates: &[&DomainQualityGate],
) -> String {
    let active_domains = domains
        .iter()
        .map(|profile| profile.id.as_str())
        .chain(gates.iter().map(|gate| gate.domain.as_str()))
        .collect::<Vec<_>>();
    let mut rules = vec![
        "- Keep concrete providers, tools, clouds, and trackers behind ports, adapters, configuration, or template profiles.".to_string(),
        "- Do not introduce platform-specific dependencies unless the domain is detected or explicitly declared.".to_string(),
        "- Keep secrets out of source, prompts, generated artifacts, logs, and issue bodies; use configured secret stores or environment variables with redaction enabled.".to_string(),
    ];

    let postgres_active = domain_is_active(&active_domains, &["database.postgres_migrations"]);
    let clickhouse_active = domain_is_active(&active_domains, &["database.clickhouse_migrations"]);
    if postgres_active && clickhouse_active {
        rules.push("- Keep PostgreSQL and ClickHouse migrations/backfills explicit, reviewed, and separated by OLTP versus analytical schema-evolution concerns.".to_string());
    } else {
        if postgres_active {
            rules.push("- Keep PostgreSQL migrations, schema changes, backfills, destructive changes, rollback notes, and expand/contract sequencing explicit and reviewed.".to_string());
        }
        if clickhouse_active {
            rules.push("- Keep ClickHouse analytical schema evolution, materialized-view dependencies, ingestion compatibility, and backfill strategy explicit and reviewed.".to_string());
        }
    }

    if domain_is_active(&active_domains, &["streaming.valkey"]) {
        rules.push("- Treat Valkey Pub/Sub as transient and Valkey Streams as the durable/replayable Valkey pattern.".to_string());
    }
    if domain_is_active(&active_domains, &["streaming.kafka"]) {
        rules.push("- Treat Kafka as the durable high-throughput streaming pattern; use outbox/inbox for cross-boundary consistency.".to_string());
    }
    if domain_is_active(&active_domains, &["storage.object"]) {
        rules.push("- Keep object storage provider-neutral; S3, Google Cloud Storage, and Azure Blob details belong in adapters/templates/config.".to_string());
    }
    if domain_is_active(&active_domains, &["communications.email"]) {
        rules.push("- Keep email/communications provider-neutral; Resend is a template example, not a domain dependency.".to_string());
    }
    if domain_is_active(&active_domains, &["api_contracts.protobuf", "rpc.grpc"]) {
        rules.push("- Keep protobuf/gRPC generated DTOs and clients out of domain entities; map them at adapters/boundaries.".to_string());
    }
    if domain_is_active(
        &active_domains,
        &[
            "observability.otel_prometheus",
            "tenancy.multi_tenant",
            "security.auth_authz",
            "resilience.service_patterns",
        ],
    ) {
        rules.push("- Require trace, metric, log, propagation, redaction, and tenant/run correlation for service-facing changes.".to_string());
    }
    if domain_is_active(&active_domains, &["security.auth_authz"]) {
        rules.push("- Treat secrets management as part of the security boundary: rotate credentials, scope tokens narrowly, and never couple provider secret APIs directly to domain code.".to_string());
    }

    rules.join("\n")
}

fn domain_is_active(active_domains: &[&str], candidates: &[&str]) -> bool {
    active_domains
        .iter()
        .any(|active| candidates.iter().any(|candidate| active == candidate))
}

fn filter_template_domains<'a>(
    config: &AgentactrConfig,
    inspection: &'a RepoInspection,
) -> Vec<&'a agentactr_core::DomainProfile> {
    inspection
        .domain_profiles
        .iter()
        .filter(|profile| domain_enabled_for_templates(config, &profile.id))
        .collect()
}

fn domain_enabled_for_templates(config: &AgentactrConfig, domain: &str) -> bool {
    domain_matches_selection(domain, &config.templates.enabled_domains)
}

pub fn render_gitignore_additions() -> String {
    r#"
# agentactr generated runtime state
.agentactr/runs/
.agentactr/artifacts/
.agentactr/debug/
.agentactr/workspaces/
.agentactr/worktrees/
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentactr_core::AgentactrConfig;

    fn parse_toml_document(content: &str) -> Result<toml::Value, String> {
        toml::from_str::<toml::Table>(content)
            .map(toml::Value::Table)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn agentactr_toml_renders_codex_milestone_transport_policy() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");

        let rendered = render_agentactr_toml(&config);

        assert!(rendered
            .contains("app_server_transport = \"stdio\" # possible values: stdio, websocket"));
        assert!(
            rendered.contains("app_server_experimental_api = false # possible values: true, false")
        );
        assert!(rendered.contains("sdk_bridge = \"typescript\" # possible values: typescript"));
        assert!(rendered.contains("fallback_mode = \"cli_json\" # possible values: cli_json"));
        assert!(rendered.contains("max_total_uncached_input_tokens = 250000"));
        assert!(rendered.contains("max_child_output_tokens = 12000"));
        assert!(rendered.contains("domains = [\"auto\"]"));
        assert!(rendered.contains("[architecture]"));
        assert!(
            rendered.contains("domain_graph_artifact = \".agentactr/artifacts/domain_graph.json\"")
        );
        assert!(rendered.contains("[templates]"));
        assert!(rendered.contains("agents_policy = \"generate_when_absent\" # possible values: generate_when_absent, artifact_only, disabled"));
        assert!(rendered.contains(
            "artifact_handoff = \"refs_summaries_and_digests\" # possible values: refs_summaries_and_digests"
        ));
        assert!(rendered.contains("pause_on_memory_pressure = true # possible values: true, false"));
        assert!(rendered.contains(
            "mode = \"enforce_on_linux_observe_elsewhere\" # possible values: enforce_on_linux_observe_elsewhere, observe_only"
        ));
        assert!(rendered.contains("cgroup_v2_required = true # possible values: true, false"));
        assert!(rendered.contains("psi_required = true # possible values: true, false"));
        assert!(rendered.contains(
            "oom_policy = \"fail_run_preserve_debug_bundle\" # possible values: fail_run_preserve_debug_bundle, fail_agent, observe"
        ));
        assert!(
            !rendered.lines().any(|line| line.starts_with('\t')),
            "rendered agentactr.toml must not indent top-level sections or keys with tabs"
        );
        parse_toml_document(&rendered).unwrap();
    }

    #[test]
    fn agentactr_toml_annotation_is_idempotent() {
        let rendered = render_agentactr_toml(&AgentactrConfig::strict_defaults("OWNER/REPO"));
        let annotated = annotate_agentactr_toml_possible_values(&rendered);

        assert_eq!(rendered, annotated);
        assert_eq!(
            rendered.matches("codex_sdk").count(),
            annotated.matches("codex_sdk").count()
        );
    }

    #[test]
    fn codex_config_uses_documented_mcp_enabled_shape() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let creds = DetectedCredentials {
            github_token: true,
            gh_token: false,
            configured_github_token: true,
            google_api_key: false,
            hf_token: false,
            openai_api_key: false,
            codex_google_mcp: false,
            codex_hf_mcp: false,
            codex_github_remote_mcp: false,
        };

        let rendered = render_codex_config_toml(&config, &creds);

        assert!(!rendered.contains("model = \"inherit\""));
        assert!(!rendered.contains("enabled = \"auto\""));
        assert!(!rendered.contains("auto_enable_when"));
        assert!(rendered.contains("[mcp_servers.openaiDeveloperDocs]\nurl = \"https://developers.openai.com/mcp\"\nenabled = true"));
        assert!(rendered.contains("[mcp_servers.GoogleDeveloperAPI]\nurl = \"https://developerknowledge.googleapis.com/mcp\"\nenabled = false"));
        parse_toml_document(&rendered).unwrap();
    }

    #[test]
    fn codex_config_uses_project_local_top_level_defaults() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.codex.profile = "custom.profile".to_string();
        config.codex.network = "on".to_string();
        let rendered = render_codex_config_toml(&config, &DetectedCredentials::default());
        let parsed = parse_toml_document(&rendered).unwrap();

        assert!(!rendered.contains("[profiles."));
        assert_eq!(
            parsed.get("approval_policy").and_then(toml::Value::as_str),
            Some("never")
        );
        assert!(
            rendered.contains("approval_policy = \"never\" # possible values: never, on-request")
        );
        assert_eq!(
            parsed.get("sandbox_mode").and_then(toml::Value::as_str),
            Some("workspace-write")
        );
        assert!(rendered.contains("network_access = true # possible values: true, false"));
        assert_eq!(
            parsed
                .get("sandbox_workspace_write")
                .and_then(|sandbox| sandbox.get("network_access"))
                .and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn codex_config_uses_gh_token_when_that_is_the_detected_github_token() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let creds = DetectedCredentials {
            github_token: false,
            gh_token: true,
            configured_github_token: false,
            google_api_key: false,
            hf_token: false,
            openai_api_key: false,
            codex_google_mcp: false,
            codex_hf_mcp: false,
            codex_github_remote_mcp: false,
        };
        let rendered = render_codex_config_toml(&config, &creds);

        assert!(rendered.contains("enabled = true"));
        assert!(rendered.contains("bearer_token_env_var = \"GH_TOKEN\""));
    }

    #[test]
    fn codex_config_prefers_configured_github_token_for_mcp() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.tracker.token_env = "AGENTACTR_GITHUB_APP_TOKEN".to_string();
        let creds = DetectedCredentials {
            github_token: true,
            gh_token: true,
            configured_github_token: true,
            google_api_key: false,
            hf_token: false,
            openai_api_key: false,
            codex_google_mcp: false,
            codex_hf_mcp: false,
            codex_github_remote_mcp: false,
        };

        let rendered = render_codex_config_toml(&config, &creds);

        assert!(rendered.contains("bearer_token_env_var = \"AGENTACTR_GITHUB_APP_TOKEN\""));
    }

    #[test]
    fn codex_config_enables_existing_codex_mcp_without_token_env() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let creds = DetectedCredentials {
            github_token: false,
            gh_token: false,
            configured_github_token: false,
            google_api_key: false,
            hf_token: false,
            openai_api_key: false,
            codex_google_mcp: false,
            codex_hf_mcp: false,
            codex_github_remote_mcp: true,
        };
        let rendered = render_codex_config_toml(&config, &creds);

        assert!(rendered.contains("[mcp_servers.github_remote]\nurl = \"https://api.githubcopilot.com/mcp/\"\nenabled = true"));
        assert!(!rendered.contains("bearer_token_env_var = \"GITHUB_TOKEN\""));
    }

    #[test]
    fn agents_md_renders_detected_domains_and_boundaries() {
        let root =
            std::env::temp_dir().join(format!("agentactr-agents-render-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("buf.yaml"), "version: v2\n").unwrap();
        std::fs::write(
            root.join("service.proto"),
            "syntax = \"proto3\";\npackage example.v1;\nservice Example { rpc Get (Req) returns (Res); }\nmessage Req {}\nmessage Res {}\n",
        )
        .unwrap();
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let inspection = crate::discovery::discover_repository(&root);

        let rendered = render_agents_md(&config, &inspection);

        assert!(rendered.contains("api_contracts.protobuf"));
        assert!(rendered.contains("rpc.grpc"));
        assert!(rendered.contains("Transport isolation"));
        assert!(rendered.contains("protobuf/gRPC generated DTOs"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn agents_md_uses_declared_stack_for_blank_project_without_irrelevant_platform_rules() {
        let root = std::env::temp_dir().join(format!(
            "agentactr-agents-blank-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.repository.declared_primary_stack = "python".to_string();
        let inspection = crate::discovery::discover_repository_with_config(&root, &config);

        let rendered = render_agents_md(&config, &inspection);

        assert!(rendered.contains("- primary_stack: python"));
        assert!(rendered.contains("language.python"));
        assert!(
            rendered.contains("Keep concrete providers, tools, clouds, and trackers behind ports")
        );
        assert!(rendered.contains("Keep secrets out of source"));
        assert!(!rendered.contains("Treat Kafka"));
        assert!(!rendered.contains("Treat Valkey"));
        assert!(!rendered.contains("PostgreSQL and ClickHouse"));
        assert!(!rendered.contains("protobuf/gRPC generated DTOs"));
        let _ = std::fs::remove_dir_all(root);
    }
}
