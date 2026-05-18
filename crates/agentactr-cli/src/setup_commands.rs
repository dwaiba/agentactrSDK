use crate::adapters::{CodexRuntimeAdapter, GithubRestAdapter};
use crate::linux_memory::LinuxMemoryController;
use crate::vcs_adapter::LocalGitAdapter;
use crate::{
    append_gitignore, codex_exec_capacity_probe, configured_repo_inspection, create_dir,
    current_epoch_millis, docker_runtime_tools_probe, flag_value, has_flag, load_agentactr_config,
    merge_config_from_toml, parse_toml_document, print_adapter_version_reports,
    print_repo_inspection, resolve_config_path, run_status, should_probe_host_codex, toml_path,
    write_file, CONFIG_KEY_VALUES, GITHUB_PROJECT_AUTOMATION_VALUES,
    GITHUB_STANDARD_LABEL_POLICY_VALUES,
};
use agentactr_execution::{resolve_execution_backend, ExecutionBackend};
use agentactr_sdk::{
    domain_findings, domain_findings_to_json, domain_graph_to_json, domain_quality_plan_to_json,
    is_generated_agents_md, is_generated_project_spec_md, project_spec_filename,
    refresh_project_spec_md, render_agentactr_toml, render_agents_md, render_codex_config_toml,
    render_gitignore_additions, render_project_spec_md, render_workflow_md, AdapterVersionReport,
    AgentRuntime, AgentactrConfig, CodexAppServerTransport, CodexAuthMode, CodexFallbackMode,
    CodexMode, CodexSdkBridge, DetectedCredentials, IssueTracker, RepoInspection, VersionControl,
};
use std::env;
use std::fs;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub(crate) fn cmd_init(args: &mut [String]) -> Result<(), String> {
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

pub(crate) fn cmd_doctor(args: &mut [String]) -> Result<(), String> {
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
    write_project_spec_if_absent(config, inspection)?;
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
        write_project_spec_if_absent(config, inspection)?;
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

fn write_project_spec_if_absent(
    config: &AgentactrConfig,
    inspection: &RepoInspection,
) -> Result<(), String> {
    let spec_path = project_spec_filename(config);
    let path = Path::new(&spec_path);
    if path.exists() {
        return Ok(());
    }
    write_file(&spec_path, &render_project_spec_md(config, inspection))
}

fn write_or_refresh_generated_project_spec(
    config: &AgentactrConfig,
    inspection: &RepoInspection,
) -> Result<(), String> {
    let spec_path = project_spec_filename(config);
    let path = Path::new(&spec_path);
    if !path.exists() {
        return write_file(&spec_path, &render_project_spec_md(config, inspection));
    }
    let content = fs::read_to_string(path).map_err(|e| format!("read {spec_path}: {e}"))?;
    if !is_generated_project_spec_md(&content) {
        return Ok(());
    }
    let refreshed = refresh_project_spec_md(&content, config, inspection);
    if refreshed != content {
        write_file(&spec_path, &refreshed)?;
    }
    Ok(())
}

fn refresh_generated_agents_after_config_set() -> Result<bool, String> {
    let path = Path::new("AGENTS.md");
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(path).map_err(|e| format!("read AGENTS.md: {e}"))?;
    if !is_generated_agents_md(&content) {
        return Ok(false);
    }
    let config = load_agentactr_config(None)?;
    let inspection = configured_repo_inspection(Path::new("."), &config);
    write_or_refresh_generated_project_spec(&config, &inspection)?;
    write_file("AGENTS.md", &render_agents_md(&config, &inspection))?;
    Ok(true)
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

pub(crate) fn cmd_config(args: &mut [String]) -> Result<(), String> {
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
            if refresh_generated_agents_after_config_set()? {
                println!("refreshed generated AGENTS.md");
            }
            Ok(())
        }
        other => Err(format!("unknown config subcommand `{other}`")),
    }
}

pub(crate) fn cmd_auth(args: &mut [String]) -> Result<(), String> {
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

pub(crate) fn detect_credentials(config: &AgentactrConfig) -> DetectedCredentials {
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

#[cfg(test)]
pub(crate) fn codex_config_mcp_server_enabled(path: &Path, server: &str) -> bool {
    codex_config_mcp_server_enabled_impl(path, server)
}

#[cfg(not(test))]
fn codex_config_mcp_server_enabled(path: &Path, server: &str) -> bool {
    codex_config_mcp_server_enabled_impl(path, server)
}

fn codex_config_mcp_server_enabled_impl(path: &Path, server: &str) -> bool {
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

pub(crate) fn print_mcp_summary(creds: &DetectedCredentials) {
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

pub(crate) fn print_memory_status() {
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

fn check_docker_runtime_tools(config: &AgentactrConfig) {
    match docker_runtime_tools_probe(config) {
        Ok(()) => println!("  ok: Docker runtime tools include codex and agentactr"),
        Err(err) => println!("  warning: {err}"),
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

fn print_doctor_adapter_versions(config: &AgentactrConfig) -> Result<(), String> {
    let reports = configured_adapter_version_reports(config)?;
    print_adapter_version_reports(&reports);
    Ok(())
}

#[cfg(test)]
pub(crate) fn configured_adapter_version_reports(
    config: &AgentactrConfig,
) -> Result<Vec<AdapterVersionReport>, String> {
    configured_adapter_version_reports_impl(config)
}

#[cfg(not(test))]
fn configured_adapter_version_reports(
    config: &AgentactrConfig,
) -> Result<Vec<AdapterVersionReport>, String> {
    configured_adapter_version_reports_impl(config)
}

fn configured_adapter_version_reports_impl(
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

#[cfg_attr(test, derive(Debug))]
pub(crate) struct GithubApiVersionSupport {
    pub(crate) version: &'static str,
    pub(crate) end_of_support: Option<&'static str>,
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

#[cfg(test)]
pub(crate) fn github_api_version_support(
    version: &str,
) -> Option<&'static GithubApiVersionSupport> {
    github_api_version_support_impl(version)
}

#[cfg(not(test))]
fn github_api_version_support(version: &str) -> Option<&'static GithubApiVersionSupport> {
    github_api_version_support_impl(version)
}

fn github_api_version_support_impl(version: &str) -> Option<&'static GithubApiVersionSupport> {
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
        Ok(status) if status.success() => println!("ok: {command} login status"),
        Ok(_) => println!("missing: Codex auth; run `codex login` for subscription auth or set `{api_key_env}` for codex exec automation"),
        Err(_) => println!("missing: {command} login status unavailable"),
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
    match codex_exec_capacity_probe(config, std::time::Duration::from_secs(60)) {
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

fn codex_user_config_path() -> Result<PathBuf, String> {
    if let Some(codex_home) = env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(codex_home).join("config.toml"));
    }
    let home = env::var_os("HOME").ok_or("HOME is not set; cannot locate Codex user config")?;
    Ok(PathBuf::from(home).join(".codex").join("config.toml"))
}

pub(crate) fn codex_project_trusted(worktree: &Path) -> Result<bool, String> {
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

#[cfg(test)]
pub(crate) fn render_codex_project_trust(
    content: &str,
    project_path: &Path,
) -> Result<String, String> {
    render_codex_project_trust_impl(content, project_path)
}

#[cfg(not(test))]
fn render_codex_project_trust(content: &str, project_path: &Path) -> Result<String, String> {
    render_codex_project_trust_impl(content, project_path)
}

fn render_codex_project_trust_impl(content: &str, project_path: &Path) -> Result<String, String> {
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

pub(crate) fn find_config_value(content: &str, dotted_key: &str) -> Option<String> {
    let parsed = parse_toml_document(content).ok()?;
    let current = toml_path(&parsed, dotted_key)?;
    current
        .as_str()
        .map(ToString::to_string)
        .or_else(|| Some(current.to_string()))
}

pub(crate) fn set_config_value(path: &str, dotted_key: &str, value: &str) -> Result<(), String> {
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
        "quality.profile" => {
            canonical_static_value(value, &["strict", "standard", "minimal"], "quality.profile")
        }
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

pub(crate) fn set_toml_edit_path(
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
