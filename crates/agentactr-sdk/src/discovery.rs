use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::domains::{
    build_domain_graph, build_domain_graph_with_config, detect_domain_profiles,
    detect_domain_profiles_with_config, domain_quality_plan, domain_quality_plan_with_config,
};
use agentactr_core::{AgentactrConfig, DomainGraph, DomainProfile, DomainQualityGate};

const DEFAULT_QUALITY_PROFILE: &str = "strict";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StackKind {
    TypeScript,
    Rust,
    Golang,
    Python,
    Mixed,
    Unknown,
}

impl StackKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
            Self::Rust => "rust",
            Self::Golang => "golang",
            Self::Python => "python",
            Self::Mixed => "mixed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RepoInspection {
    pub root: PathBuf,
    pub is_git: bool,
    pub is_empty: bool,
    pub detected_stack: StackKind,
    pub primary_stack: StackKind,
    pub confidence: u8,
    pub evidence_files: Vec<String>,
    pub missing_prerequisites: Vec<String>,
    pub setup_guidance: Vec<String>,
    pub selected_quality_profile: String,
    pub quality_plan: Vec<QualityCommand>,
    pub domain_profiles: Vec<DomainProfile>,
    pub domain_quality_plan: Vec<DomainQualityGate>,
    pub domain_graph: DomainGraph,
}

#[derive(Clone, Debug)]
pub struct QualityCommand {
    pub name: String,
    pub command: String,
    pub required: bool,
    pub non_mutating_final_gate: bool,
}

#[derive(Default)]
struct StackEvidence {
    typescript: u16,
    rust: u16,
    golang: u16,
    python: u16,
    files: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageManager {
    Bun,
    Pnpm,
    Npm,
    Yarn,
    Deno,
}

pub fn discover_repository(root: &Path) -> RepoInspection {
    discover_repository_inner(root, None)
}

pub fn discover_repository_with_config(root: &Path, config: &AgentactrConfig) -> RepoInspection {
    discover_repository_inner(root, Some(config))
}

fn discover_repository_inner(root: &Path, config: Option<&AgentactrConfig>) -> RepoInspection {
    let is_git = root.join(".git").exists();
    let files = collect_repo_files(root);
    let is_empty = files.is_empty();
    let evidence = score_evidence(&files);
    let (primary_stack, confidence) = select_stack(&evidence);
    let mut missing_prerequisites = missing_prerequisites(root, &primary_stack, is_empty);
    let mut setup_guidance = setup_guidance_for(root, &primary_stack, is_empty);

    if is_empty {
        missing_prerequisites.push(
            "empty repository requires repository.declared_primary_stack before bootstrap"
                .to_string(),
        );
        setup_guidance.push(
            "agentactr config set repository.declared_primary_stack rust|typescript|golang|python"
                .to_string(),
        );
    }

    let detected_stack = if is_empty {
        StackKind::Unknown
    } else {
        primary_stack.clone()
    };

    let domain_profiles = config.map_or_else(
        || detect_domain_profiles(root),
        |config| detect_domain_profiles_with_config(root, config),
    );
    let base_domain_quality_plan = config.map_or_else(
        || domain_quality_plan(root),
        |config| domain_quality_plan_with_config(root, config),
    );
    let quality_plan = quality_plan_for_repository(root, &primary_stack);
    let domain_quality_plan =
        compose_typed_domain_quality_plan(&primary_stack, &quality_plan, base_domain_quality_plan);
    let domain_graph = config.map_or_else(
        || build_domain_graph(root, "local"),
        |config| build_domain_graph_with_config(root, "local", config),
    );

    RepoInspection {
        root: root.to_path_buf(),
        is_git,
        is_empty,
        detected_stack: detected_stack.clone(),
        primary_stack: detected_stack,
        confidence: if is_empty { 0 } else { confidence },
        evidence_files: evidence.files,
        missing_prerequisites,
        setup_guidance,
        selected_quality_profile: DEFAULT_QUALITY_PROFILE.to_string(),
        quality_plan,
        domain_profiles,
        domain_quality_plan,
        domain_graph,
    }
}

pub fn apply_declared_stack_to_inspection(
    mut inspection: RepoInspection,
    stack: &StackKind,
) -> RepoInspection {
    inspection.primary_stack = stack.clone();
    inspection.confidence = 100;
    inspection.evidence_files = vec![format!(
        "config:repository.declared_primary_stack={}",
        stack.as_str()
    )];
    inspection.missing_prerequisites =
        missing_prerequisites(&inspection.root, stack, inspection.is_empty);
    inspection.setup_guidance = setup_guidance_for(&inspection.root, stack, inspection.is_empty);
    inspection.quality_plan = quality_plan_for_repository(&inspection.root, stack);
    inspection.domain_profiles = detect_domain_profiles(&inspection.root);
    inspection.domain_quality_plan = compose_typed_domain_quality_plan(
        stack,
        &inspection.quality_plan,
        domain_quality_plan(&inspection.root),
    );
    inspection.domain_graph = build_domain_graph(&inspection.root, "local");
    inspection
}

pub fn apply_declared_stack_to_inspection_with_config(
    inspection: RepoInspection,
    stack: &StackKind,
    config: &AgentactrConfig,
) -> RepoInspection {
    let mut inspection = apply_declared_stack_to_inspection(inspection, stack);
    inspection.domain_profiles = detect_domain_profiles_with_config(&inspection.root, config);
    inspection.domain_quality_plan = compose_typed_domain_quality_plan(
        stack,
        &inspection.quality_plan,
        domain_quality_plan_with_config(&inspection.root, config),
    );
    inspection.domain_graph = build_domain_graph_with_config(&inspection.root, "local", config);
    inspection
}

fn compose_typed_domain_quality_plan(
    stack: &StackKind,
    quality_plan: &[QualityCommand],
    mut domain_plan: Vec<DomainQualityGate>,
) -> Vec<DomainQualityGate> {
    let language_gates = language_quality_command_gates(stack, quality_plan);
    if language_gates.is_empty() {
        return domain_plan;
    }
    domain_plan.retain(|gate| {
        !(gate.command.is_none()
            && gate.domain.starts_with("language.")
            && gate.name.ends_with("_stack_contract"))
    });
    domain_plan.extend(language_gates);
    domain_plan.sort_by(|a, b| a.domain.cmp(&b.domain).then(a.name.cmp(&b.name)));
    domain_plan
}

fn language_quality_command_gates(
    stack: &StackKind,
    quality_plan: &[QualityCommand],
) -> Vec<DomainQualityGate> {
    quality_plan
        .iter()
        .filter_map(|command| {
            let domain = language_domain_for_quality_command(stack, command)?;
            Some(DomainQualityGate {
                name: format!("{}_{}", domain.replace('.', "_"), gate_name(&command.name)),
                domain: domain.to_string(),
                tool: quality_command_tool(&command.command),
                command: Some(command.command.clone()),
                required: command.required,
                mutates: false,
                network_required: language_command_network_required(
                    &command.name,
                    &command.command,
                ),
                credential_required: false,
                opt_in_required: false,
                degraded_if_missing: false,
                artifact_paths: Vec::new(),
                setup_guidance: vec![
                    "migrated from repository stack quality plan into typed domain quality gates"
                        .to_string(),
                ],
                failure_policy: if command.required {
                    "fail_closed".to_string()
                } else {
                    "record_only".to_string()
                },
            })
        })
        .collect()
}

fn language_domain_for_quality_command<'a>(
    stack: &'a StackKind,
    command: &QualityCommand,
) -> Option<&'a str> {
    match stack {
        StackKind::Rust => Some("language.rust"),
        StackKind::TypeScript => Some("language.typescript"),
        StackKind::Golang => Some("language.golang"),
        StackKind::Python => Some("language.python"),
        StackKind::Mixed => infer_language_domain_from_command(command),
        StackKind::Unknown => None,
    }
}

fn infer_language_domain_from_command(command: &QualityCommand) -> Option<&'static str> {
    let value = format!(
        "{} {}",
        command.name.to_ascii_lowercase(),
        command.command.to_ascii_lowercase()
    );
    if value.contains("cargo ") || value.contains("rust") {
        Some("language.rust")
    } else if value.contains("go ") || value.contains("gofmt") || value.contains("golangci") {
        Some("language.golang")
    } else if value.contains("uv ")
        || value.contains("ruff")
        || value.contains("pytest")
        || value.contains("pyright")
    {
        Some("language.python")
    } else if value.contains("bun ")
        || value.contains("npm ")
        || value.contains("pnpm ")
        || value.contains("yarn ")
        || value.contains("deno ")
        || value.contains("npx ")
    {
        Some("language.typescript")
    } else {
        None
    }
}

fn gate_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn quality_command_tool(command: &str) -> String {
    command
        .split_whitespace()
        .find(|part| *part != "cd" && *part != "&&")
        .unwrap_or("agentactr")
        .trim_matches('\'')
        .to_string()
}

fn language_command_network_required(name: &str, command: &str) -> bool {
    let value = format!(
        "{} {}",
        name.to_ascii_lowercase(),
        command.to_ascii_lowercase()
    );
    value.contains(" install")
        || value.starts_with("install ")
        || value.contains("npm ci")
        || value.contains("bun install")
        || value.contains("pnpm install")
        || value.contains("yarn install")
        || value.contains("uv sync")
}

pub fn quality_plan_for_stack(stack: &StackKind) -> Vec<QualityCommand> {
    match stack {
        StackKind::TypeScript => fallback_typescript_quality_plan(),
        StackKind::Rust => commands(&[
            ("fmt", "cargo fmt --all -- --check"),
            (
                "clippy",
                "cargo clippy --workspace --all-targets --all-features -- -D warnings",
            ),
            ("nextest", "cargo nextest run --workspace --all-features"),
            ("doc_tests", "cargo test --doc --workspace --all-features"),
            ("deny", "cargo deny check"),
            ("machete", "cargo machete"),
        ]),
        StackKind::Golang => fallback_golang_quality_plan(),
        StackKind::Python => commands(&[
            ("sync", "uv sync --frozen"),
            ("format", "uv run ruff format --check ."),
            ("lint", "uv run ruff check ."),
            ("types", "uv run pyright"),
            ("test", "uv run pytest"),
            ("audit", "uv run pip-audit"),
            ("deps", "uv run deptry ."),
        ]),
        StackKind::Mixed => Vec::new(),
        StackKind::Unknown => Vec::new(),
    }
}

pub fn quality_plan_for_repository(root: &Path, stack: &StackKind) -> Vec<QualityCommand> {
    match stack {
        StackKind::TypeScript => typescript_quality_plan(root),
        StackKind::Golang => golang_quality_plan(root),
        StackKind::Mixed => {
            let mut out = Vec::new();
            for detected in detected_stacks_for_root(root) {
                match detected {
                    StackKind::TypeScript => out.extend(typescript_quality_plan(root)),
                    StackKind::Golang => out.extend(golang_quality_plan(root)),
                    StackKind::Rust | StackKind::Python => {
                        out.extend(quality_plan_for_stack(&detected))
                    }
                    StackKind::Mixed | StackKind::Unknown => {}
                }
            }
            out
        }
        _ => quality_plan_for_stack(stack),
    }
}

fn commands(values: &[(&str, &str)]) -> Vec<QualityCommand> {
    values
        .iter()
        .map(|(name, command)| QualityCommand {
            name: (*name).to_string(),
            command: (*command).to_string(),
            required: true,
            non_mutating_final_gate: true,
        })
        .collect()
}

fn fallback_typescript_quality_plan() -> Vec<QualityCommand> {
    commands(&[
        ("install", "npm ci"),
        ("lint", "npm run lint --if-present"),
        ("typecheck", "npm run typecheck --if-present"),
        ("test", "npm run test --if-present"),
        ("build", "npm run build --if-present"),
    ])
}

fn fallback_golang_quality_plan() -> Vec<QualityCommand> {
    commands(&[
        ("gofmt", "gofmt-check"),
        ("mod_verify", "go mod verify"),
        ("tidy_check", "go mod tidy-check"),
        ("vet", "go vet ./..."),
        ("golangci_lint", "golangci-lint run"),
        ("vulncheck", "govulncheck ./..."),
        ("test", "go test ./..."),
    ])
}

fn golang_quality_plan(root: &Path) -> Vec<QualityCommand> {
    let module_roots = go_module_roots(root);
    if module_roots.is_empty() {
        return fallback_golang_quality_plan();
    }

    module_roots
        .iter()
        .flat_map(|module_root| {
            let rel = relative_dir(root, module_root);
            [
                ("gofmt", "gofmt-check"),
                ("mod_verify", "go mod verify"),
                ("tidy_check", "go mod tidy-check"),
                ("vet", "go vet ./..."),
                ("golangci_lint", "golangci-lint run"),
                ("vulncheck", "govulncheck ./..."),
                ("test", "go test ./..."),
            ]
            .into_iter()
            .map(move |(name, command)| {
                let scoped_name = if rel == "." {
                    name.to_string()
                } else {
                    format!("{name}:{rel}")
                };
                quality_command(&scoped_name, scoped_command(&rel, command))
            })
        })
        .collect()
}

fn typescript_quality_plan(root: &Path) -> Vec<QualityCommand> {
    let project_roots = typescript_project_roots(root);
    if !project_roots.is_empty() {
        let root_package_json = read_package_json(root);
        let inherited_package_manager = detect_package_manager(root, root_package_json.as_ref());
        let root_declares_workspaces = package_json_declares_workspaces(root_package_json.as_ref());
        return project_roots
            .iter()
            .flat_map(|project_root| {
                let rel = relative_dir(root, project_root);
                let package_json = read_package_json(project_root);
                let has_local_package_manager =
                    has_package_manager_signal(project_root, package_json.as_ref());
                let package_manager = if rel == "." || has_local_package_manager {
                    detect_package_manager(project_root, package_json.as_ref())
                } else {
                    inherited_package_manager
                };
                let include_install =
                    rel == "." || (has_local_package_manager && !root_declares_workspaces);
                typescript_quality_plan_for_project(project_root, package_manager, include_install)
                    .into_iter()
                    .map(move |cmd| scope_quality_command(&rel, cmd))
            })
            .collect();
    }
    let package_json = read_package_json(root);
    let package_manager = detect_package_manager(root, package_json.as_ref());
    typescript_quality_plan_for_project(root, package_manager, true)
}

fn typescript_quality_plan_for_project(
    root: &Path,
    package_manager: PackageManager,
    include_install: bool,
) -> Vec<QualityCommand> {
    let package_json = read_package_json(root);
    if package_manager == PackageManager::Deno {
        return deno_quality_plan();
    }
    let mut plan = Vec::new();
    if include_install {
        plan.push(quality_command(
            "install",
            install_command(root, package_manager),
        ));
    }

    if uses_biome(root, package_json.as_ref()) {
        plan.push(quality_command("biome", "npx --no-install biome check ."));
    }

    for script in ["lint", "typecheck", "test", "build"] {
        if has_script(package_json.as_ref(), script) {
            plan.push(quality_command(
                script,
                run_script_command(package_manager, script),
            ));
        }
    }

    if uses_framework(package_json.as_ref()) {
        if let Some(script) = ["smoke:check", "test:smoke", "smoke"]
            .into_iter()
            .find(|script| has_script(package_json.as_ref(), script))
        {
            plan.push(quality_command(
                "framework_smoke",
                run_script_command(package_manager, script),
            ));
        }
    }

    plan
}

fn scope_quality_command(relative_dir: &str, mut command: QualityCommand) -> QualityCommand {
    if relative_dir != "." {
        command.name = format!("{}:{relative_dir}", command.name);
        command.command = scoped_command(relative_dir, &command.command);
    }
    command
}

fn deno_quality_plan() -> Vec<QualityCommand> {
    commands(&[
        ("fmt", "deno fmt --check"),
        ("lint", "deno lint"),
        ("test", "deno test --frozen"),
    ])
}

fn quality_command(name: impl Into<String>, command: impl Into<String>) -> QualityCommand {
    QualityCommand {
        name: name.into(),
        command: command.into(),
        required: true,
        non_mutating_final_gate: true,
    }
}

fn scoped_command(relative_dir: &str, command: &str) -> String {
    if relative_dir == "." {
        command.to_string()
    } else {
        format!("cd {} && {command}", shell_quote(relative_dir))
    }
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn read_package_json(root: &Path) -> Option<Value> {
    let content = fs::read_to_string(root.join("package.json")).ok()?;
    serde_json::from_str(&content).ok()
}

fn typescript_project_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = collect_repo_files(root)
        .into_iter()
        .filter(|file| {
            matches!(
                stack_marker_name(file).as_deref(),
                Some("package.json" | "deno.json" | "deno.jsonc")
            )
        })
        .filter_map(|file| {
            let rel = Path::new(&file);
            rel.parent().map(|parent| {
                if parent.as_os_str().is_empty() {
                    root.to_path_buf()
                } else {
                    root.join(parent)
                }
            })
        })
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    roots
}

fn typescript_lockfile_exists(root: &Path) -> bool {
    any_exists(
        root,
        &[
            "bun.lock",
            "bun.lockb",
            "pnpm-lock.yaml",
            "package-lock.json",
            "npm-shrinkwrap.json",
            "yarn.lock",
        ],
    )
}

fn detect_package_manager(root: &Path, package_json: Option<&Value>) -> PackageManager {
    lockfile_package_manager(root)
        .or_else(|| package_json_package_manager(package_json))
        .or_else(|| deno_package_manager(root))
        .unwrap_or(PackageManager::Npm)
}

fn lockfile_package_manager(root: &Path) -> Option<PackageManager> {
    if root.join("pnpm-lock.yaml").exists() {
        Some(PackageManager::Pnpm)
    } else if root.join("yarn.lock").exists() {
        Some(PackageManager::Yarn)
    } else if any_exists(root, &["package-lock.json", "npm-shrinkwrap.json"]) {
        Some(PackageManager::Npm)
    } else if any_exists(root, &["bun.lock", "bun.lockb"]) {
        Some(PackageManager::Bun)
    } else if root.join("deno.lock").exists() {
        Some(PackageManager::Deno)
    } else {
        None
    }
}

fn deno_package_manager(root: &Path) -> Option<PackageManager> {
    any_exists(root, &["deno.json", "deno.jsonc"]).then_some(PackageManager::Deno)
}

fn package_json_package_manager(package_json: Option<&Value>) -> Option<PackageManager> {
    let value = package_json?
        .get("packageManager")
        .and_then(Value::as_str)?
        .split('@')
        .next()?;
    match value {
        "bun" => Some(PackageManager::Bun),
        "pnpm" => Some(PackageManager::Pnpm),
        "npm" => Some(PackageManager::Npm),
        "yarn" => Some(PackageManager::Yarn),
        _ => None,
    }
}

fn package_json_declares_workspaces(package_json: Option<&Value>) -> bool {
    let Some(value) = package_json.and_then(|json| json.get("workspaces")) else {
        return false;
    };
    value.as_array().is_some_and(|items| !items.is_empty())
        || value
            .as_object()
            .and_then(|object| object.get("packages"))
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
}

fn has_package_manager_signal(root: &Path, package_json: Option<&Value>) -> bool {
    lockfile_package_manager(root).is_some()
        || package_json_package_manager(package_json).is_some()
        || deno_package_manager(root).is_some()
}

fn install_command(root: &Path, package_manager: PackageManager) -> &'static str {
    match package_manager {
        PackageManager::Bun => "bun install --frozen-lockfile",
        PackageManager::Pnpm => "pnpm install --frozen-lockfile",
        PackageManager::Npm => "npm ci",
        PackageManager::Yarn if uses_yarn_classic(root) => "yarn install --frozen-lockfile",
        PackageManager::Yarn => "yarn install --immutable",
        PackageManager::Deno => "deno cache --frozen",
    }
}

fn uses_yarn_classic(root: &Path) -> bool {
    let Some(package_json) = read_package_json(root) else {
        return false;
    };
    package_json
        .get("packageManager")
        .and_then(Value::as_str)
        .is_some_and(|value| value.starts_with("yarn@1."))
}

fn run_script_command(package_manager: PackageManager, script: &str) -> String {
    match package_manager {
        PackageManager::Bun => format!("bun run {script}"),
        PackageManager::Pnpm => format!("pnpm run {script}"),
        PackageManager::Npm => format!("npm run {script}"),
        PackageManager::Yarn => format!("yarn run {script}"),
        PackageManager::Deno => format!("deno task {script}"),
    }
}

fn uses_biome(root: &Path, package_json: Option<&Value>) -> bool {
    any_exists(root, &["biome.json", "biome.jsonc"])
        || has_dependency(package_json, &["@biomejs/biome", "biome"])
}

fn uses_framework(package_json: Option<&Value>) -> bool {
    has_dependency(
        package_json,
        &["vite", "next", "@remix-run/dev", "@sveltejs/kit", "astro"],
    )
}

fn has_script(package_json: Option<&Value>, script: &str) -> bool {
    package_json
        .and_then(|json| json.get("scripts"))
        .and_then(Value::as_object)
        .and_then(|scripts| scripts.get(script))
        .and_then(Value::as_str)
        .is_some()
}

fn has_dependency(package_json: Option<&Value>, names: &[&str]) -> bool {
    ["dependencies", "devDependencies", "peerDependencies"]
        .into_iter()
        .filter_map(|section| package_json?.get(section)?.as_object())
        .any(|deps| names.iter().any(|name| deps.contains_key(*name)))
}

fn collect_repo_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_repo_files_inner(root, root, &mut files);
    files
}

fn collect_repo_files_inner(root: &Path, dir: &Path, files: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if ignored_entry(&name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_repo_files_inner(root, &path, files);
        } else if file_type.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rendered = rel.to_string_lossy().replace('\\', "/");
                if !ignored_file(&rendered) {
                    files.push(rendered);
                }
            }
        }
    }
}

fn ignored_entry(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".agentactr"
            | ".trunk"
            | ".gomodcache"
            | ".cache"
            | "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".next"
            | ".venv"
            | "vendor"
            | "__pycache__"
    )
}

fn ignored_file(name: &str) -> bool {
    matches!(
        name,
        ".gitignore" | "agentactr.toml" | "WORKFLOW.md" | "Specs.md" | "specs_v01.md"
    ) || name.starts_with("specs_")
        || name.starts_with("internal_specs_agentactrSDK/")
}

fn score_evidence(files: &[String]) -> StackEvidence {
    let mut e = StackEvidence::default();
    for file in files {
        let marker = stack_marker_name(file);
        match marker.as_deref().unwrap_or(file.as_str()) {
            "package.json" | "tsconfig.json" | "bun.lockb" | "bun.lock" | "pnpm-lock.yaml"
            | "package-lock.json" | "yarn.lock" | "biome.json" | "biome.jsonc" | "deno.json"
            | "deno.jsonc" | "deno.lock" => {
                e.typescript += 4;
                e.files.push(file.clone());
            }
            "Cargo.toml" | "Cargo.lock" | "deny.toml" => {
                e.rust += 5;
                e.files.push(file.clone());
            }
            "go.mod" | "go.sum" | ".golangci.yml" | ".golangci.yaml" => {
                e.golang += 5;
                e.files.push(file.clone());
            }
            name if python_marker_name(name) => {
                e.python += 5;
                e.files.push(file.clone());
            }
            _ => {
                if file.ends_with(".ts") || file.ends_with(".tsx") {
                    e.typescript += 1;
                } else if file.ends_with(".rs") {
                    e.rust += 1;
                } else if file.ends_with(".go") {
                    e.golang += 1;
                } else if file.ends_with(".py") {
                    e.python += 1;
                }
            }
        }
    }
    e.files.sort();
    e.files.dedup();
    e
}

fn stack_marker_name(file: &str) -> Option<String> {
    let path = Path::new(file);
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            matches!(
                *name,
                "package.json"
                    | "tsconfig.json"
                    | "bun.lockb"
                    | "bun.lock"
                    | "pnpm-lock.yaml"
                    | "package-lock.json"
                    | "yarn.lock"
                    | "biome.json"
                    | "biome.jsonc"
                    | "deno.json"
                    | "deno.jsonc"
                    | "deno.lock"
                    | "Cargo.toml"
                    | "Cargo.lock"
                    | "deny.toml"
                    | "go.mod"
                    | "go.sum"
                    | ".golangci.yml"
                    | ".golangci.yaml"
            ) || python_marker_name(name)
        })
        .map(ToString::to_string)
}

fn python_marker_name(name: &str) -> bool {
    matches!(
        name,
        "pyproject.toml"
            | "uv.lock"
            | "poetry.lock"
            | "pdm.lock"
            | "Pipfile.lock"
            | "setup.py"
            | "setup.cfg"
            | "tox.ini"
            | "noxfile.py"
            | "pytest.ini"
            | "mypy.ini"
    ) || (name.starts_with("requirements") && name.ends_with(".txt"))
}

fn select_stack(e: &StackEvidence) -> (StackKind, u8) {
    let scores = [
        (StackKind::TypeScript, e.typescript),
        (StackKind::Rust, e.rust),
        (StackKind::Golang, e.golang),
        (StackKind::Python, e.python),
    ];
    let active = scores.iter().filter(|(_, score)| *score > 0).count();
    if active == 0 {
        return (StackKind::Unknown, 0);
    }
    let mut sorted = scores.to_vec();
    sorted.sort_by_key(|item| Reverse(item.1));
    if active > 1 && sorted[1].1 >= sorted[0].1 / 2 {
        return (StackKind::Mixed, 70);
    }
    let confidence = (50 + sorted[0].1.min(50)) as u8;
    (sorted[0].0.clone(), confidence)
}

fn detected_stacks_for_root(root: &Path) -> Vec<StackKind> {
    detected_stacks(&score_evidence(&collect_repo_files(root)))
}

fn detected_stacks(e: &StackEvidence) -> Vec<StackKind> {
    let mut stacks = Vec::new();
    if e.typescript > 0 {
        stacks.push(StackKind::TypeScript);
    }
    if e.rust > 0 {
        stacks.push(StackKind::Rust);
    }
    if e.golang > 0 {
        stacks.push(StackKind::Golang);
    }
    if e.python > 0 {
        stacks.push(StackKind::Python);
    }
    stacks
}

fn missing_prerequisites(root: &Path, stack: &StackKind, is_empty: bool) -> Vec<String> {
    if is_empty {
        return Vec::new();
    }
    let mut missing = Vec::new();
    match stack {
        StackKind::TypeScript => {
            let project_roots = typescript_project_roots(root);
            let project_roots = if project_roots.is_empty() {
                vec![root.to_path_buf()]
            } else {
                project_roots
            };
            for project_root in project_roots {
                let deno_only = is_deno_only_typescript(&project_root);
                if !deno_only
                    && !typescript_node_version_pinned(&project_root)
                    && !typescript_node_version_pinned(root)
                {
                    missing.push(format!(
                        "TypeScript strict profile requires .nvmrc, .node-version, mise.toml, volta.node, or engines.node ({})",
                        relative_dir(root, &project_root)
                    ));
                }
                if !deno_only && !typescript_lockfile_exists(&project_root) {
                    missing.push(format!(
                        "TypeScript strict profile requires a package manager lockfile ({})",
                        relative_dir(root, &project_root)
                    ));
                }
            }
        }
        StackKind::Rust => {
            if !root.join("Cargo.lock").exists() {
                missing.push(
                    "Rust strict profile requires Cargo.lock for applications/workspaces".into(),
                );
            }
        }
        StackKind::Golang => {
            let module_roots = go_module_roots(root);
            if module_roots.is_empty() {
                missing.push("Golang strict profile requires go.mod".into());
            }
            for module_root in &module_roots {
                if !go_version_declared(root, module_root) {
                    missing.push(format!(
                        "Golang strict profile requires Go version/toolchain declaration ({})",
                        relative_dir(root, module_root)
                    ));
                }
                if go_module_has_external_dependencies(module_root)
                    && !module_root.join("go.sum").exists()
                {
                    missing.push(format!(
                        "Golang strict profile requires go.sum when dependencies exist ({})",
                        relative_dir(root, module_root)
                    ));
                }
            }
            if !golangci_pin_exists(root, &module_roots) {
                missing.push(
                    "Golang strict profile requires pinned golangci-lint config or repo tooling"
                        .into(),
                );
            }
        }
        StackKind::Python => {
            if !root.join("uv.lock").exists() {
                missing.push("Python strict profile requires uv.lock".into());
            }
            if !python_version_pinned(root) {
                missing.push(
                    "Python strict profile requires .python-version, runtime.txt, requires-python, mise.toml, uv config, or explicit tool version".into(),
                );
            }
        }
        StackKind::Mixed => {
            let detected = detected_stacks_for_root(root);
            if detected.is_empty() {
                missing.push("mixed stack requires detected or configured member stacks".into());
            } else {
                for stack in detected {
                    missing.extend(missing_prerequisites(root, &stack, false));
                }
                missing.sort();
                missing.dedup();
            }
        }
        StackKind::Unknown => {
            missing.push("unable to detect a supported stack with high confidence".into());
        }
    }
    missing
}

fn go_module_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = collect_repo_files(root)
        .into_iter()
        .filter(|file| stack_marker_name(file.as_str()).as_deref() == Some("go.mod"))
        .filter_map(|file| {
            let rel = Path::new(&file);
            rel.parent().map(|parent| {
                if parent.as_os_str().is_empty() {
                    root.to_path_buf()
                } else {
                    root.join(parent)
                }
            })
        })
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    roots
}

fn is_deno_only_typescript(root: &Path) -> bool {
    any_exists(root, &["deno.json", "deno.jsonc", "deno.lock"])
        && !root.join("package.json").exists()
}

fn typescript_node_version_pinned(root: &Path) -> bool {
    any_exists(root, &[".nvmrc", ".node-version"])
        || text_file_mentions_any(&root.join("mise.toml"), &["node", "nodejs"])
        || package_json_has_string_path(root, &["volta", "node"])
        || package_json_has_string_path(root, &["engines", "node"])
}

fn package_json_has_string_path(root: &Path, path: &[&str]) -> bool {
    let Ok(content) = fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&content) else {
        return false;
    };
    let mut current = &parsed;
    for segment in path {
        let Some(next) = current.get(segment) else {
            return false;
        };
        current = next;
    }
    current
        .as_str()
        .is_some_and(|value| !value.trim().is_empty())
}

fn golangci_pin_exists(root: &Path, module_roots: &[PathBuf]) -> bool {
    golangci_config_exists(root, module_roots)
        || golangci_tool_version_exists(root, module_roots)
        || golangci_repo_tooling_exists(root, module_roots)
}

fn golangci_config_exists(root: &Path, module_roots: &[PathBuf]) -> bool {
    roots_with_repo_root(root, module_roots)
        .iter()
        .any(|candidate| any_exists(candidate, &[".golangci.yml", ".golangci.yaml"]))
}

fn golangci_tool_version_exists(root: &Path, module_roots: &[PathBuf]) -> bool {
    roots_with_repo_root(root, module_roots)
        .iter()
        .any(|candidate| {
            text_file_mentions_any(
                &candidate.join("mise.toml"),
                &["golangci-lint", "golangci_lint"],
            ) || text_file_mentions_any(&candidate.join(".tool-versions"), &["golangci-lint"])
                || text_file_mentions_any(&candidate.join("agentactr.toml"), &["golangci-lint"])
        })
}

fn golangci_repo_tooling_exists(root: &Path, module_roots: &[PathBuf]) -> bool {
    let repo_tooling_files = [
        "tools.go",
        "go.mod",
        "Makefile",
        "Taskfile.yml",
        "Taskfile.yaml",
        "justfile",
        "magefile.go",
        "scripts/install-golangci-lint.sh",
        "scripts/tools.sh",
    ];
    roots_with_repo_root(root, module_roots)
        .iter()
        .any(|candidate| {
            repo_tooling_files.iter().any(|file| {
                text_file_mentions_any(
                    &candidate.join(file),
                    &["golangci-lint", "github.com/golangci/golangci-lint"],
                )
            })
        })
}

fn roots_with_repo_root(root: &Path, module_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(module_roots.len() + 1);
    roots.push(root.to_path_buf());
    roots.extend(module_roots.iter().cloned());
    roots.sort();
    roots.dedup();
    roots
}

fn go_version_declared(root: &Path, module_root: &Path) -> bool {
    fs::read_to_string(module_root.join("go.mod")).is_ok_and(|content| {
        content.lines().any(|raw| {
            let line = strip_go_mod_line_comment(raw);
            line.starts_with("go ") || line.starts_with("toolchain ")
        })
    }) || text_file_mentions_any(&root.join(".tool-versions"), &["golang", "go"])
        || text_file_mentions_any(&root.join("mise.toml"), &["golang", "go"])
}

fn python_version_pinned(root: &Path) -> bool {
    any_exists(root, &[".python-version", "runtime.txt"])
        || text_file_mentions_any(&root.join("pyproject.toml"), &["requires-python"])
        || text_file_mentions_any(&root.join("mise.toml"), &["python"])
        || text_file_mentions_any(&root.join(".tool-versions"), &["python"])
        || text_file_mentions_any(&root.join("uv.toml"), &["python"])
        || text_file_mentions_any(&root.join("pyproject.toml"), &["python-preference"])
}

fn text_file_mentions_any(path: &Path, needles: &[&str]) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let lower = content.to_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

fn relative_dir(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .filter(|rel| !rel.as_os_str().is_empty())
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| ".".to_string())
}

fn go_module_has_external_dependencies(root: &Path) -> bool {
    let Ok(content) = fs::read_to_string(root.join("go.mod")) else {
        return false;
    };
    let requirements = go_mod_requirements(&content);
    if requirements.is_empty() {
        return false;
    }
    let local_replacements = go_mod_local_replacements(&content);
    requirements
        .iter()
        .any(|requirement| !go_requirement_has_local_replacement(requirement, &local_replacements))
}

#[derive(Debug, Eq, PartialEq)]
struct GoModuleRequirement {
    module: String,
    version: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct GoLocalReplacement {
    module: String,
    version: Option<String>,
}

fn go_mod_requirements(content: &str) -> Vec<GoModuleRequirement> {
    let mut requirements = Vec::new();
    let mut in_require_block = false;

    for raw_line in content.lines() {
        let line = strip_go_mod_line_comment(raw_line);
        if in_require_block {
            if line == ")" {
                in_require_block = false;
                continue;
            }
            if let Some(requirement) = parse_go_requirement(line) {
                requirements.push(requirement);
            }
            continue;
        }
        if line == "require (" {
            in_require_block = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("require ") {
            if let Some(requirement) = parse_go_requirement(rest.trim()) {
                requirements.push(requirement);
            }
        }
    }

    requirements
}

fn go_mod_local_replacements(content: &str) -> Vec<GoLocalReplacement> {
    let mut replacements = Vec::new();
    let mut in_replace_block = false;

    for raw_line in content.lines() {
        let line = strip_go_mod_line_comment(raw_line);
        if in_replace_block {
            if line == ")" {
                in_replace_block = false;
                continue;
            }
            if let Some(replacement) = parse_go_local_replacement(line) {
                replacements.push(replacement);
            }
            continue;
        }
        if line == "replace (" {
            in_replace_block = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("replace ") {
            if let Some(replacement) = parse_go_local_replacement(rest.trim()) {
                replacements.push(replacement);
            }
        }
    }

    replacements
}

fn strip_go_mod_line_comment(line: &str) -> &str {
    line.split("//").next().unwrap_or_default().trim()
}

fn parse_go_requirement(line: &str) -> Option<GoModuleRequirement> {
    let mut tokens = line.split_whitespace();
    let module = tokens.next()?;
    let version = tokens.next().map(ToString::to_string);
    Some(GoModuleRequirement {
        module: module.to_string(),
        version,
    })
}

fn parse_go_local_replacement(line: &str) -> Option<GoLocalReplacement> {
    let (left, right) = line.split_once("=>")?;
    let target = right.split_whitespace().next()?;
    if !is_local_go_replacement_target(target) {
        return None;
    }
    let mut left_tokens = left.split_whitespace();
    let module = left_tokens.next()?;
    let version = left_tokens.next().map(ToString::to_string);
    Some(GoLocalReplacement {
        module: module.to_string(),
        version,
    })
}

fn is_local_go_replacement_target(target: &str) -> bool {
    target == "."
        || target == ".."
        || target.starts_with("./")
        || target.starts_with("../")
        || target.starts_with('/')
        || target.starts_with(".\\")
        || target.starts_with("..\\")
        || target.starts_with('\\')
        || target.as_bytes().get(1) == Some(&b':')
}

fn go_requirement_has_local_replacement(
    requirement: &GoModuleRequirement,
    replacements: &[GoLocalReplacement],
) -> bool {
    replacements.iter().any(|replacement| {
        replacement.module == requirement.module
            && match replacement.version.as_ref() {
                Some(version) => Some(version) == requirement.version.as_ref(),
                None => true,
            }
    })
}

fn setup_guidance_for(root: &Path, stack: &StackKind, is_empty: bool) -> Vec<String> {
    if is_empty {
        return Vec::new();
    }
    let mut guidance = Vec::new();
    match stack {
        StackKind::TypeScript => {
            let project_roots = typescript_project_roots(root);
            let project_roots = if project_roots.is_empty() {
                vec![root.to_path_buf()]
            } else {
                project_roots
            };
            for project_root in project_roots {
                let deno_only = is_deno_only_typescript(&project_root);
                if !deno_only
                    && !typescript_node_version_pinned(&project_root)
                    && !typescript_node_version_pinned(root)
                {
                    guidance.push(format!(
                        "pin Node with .nvmrc, .node-version, mise.toml, volta.node, or engines.node ({})",
                        relative_dir(root, &project_root)
                    ));
                }
                if !deno_only && !typescript_lockfile_exists(&project_root) {
                    guidance.push(format!(
                        "install dependencies with the selected pinned package manager to create a lockfile ({})",
                        relative_dir(root, &project_root)
                    ));
                }
            }
        }
        StackKind::Rust => {
            guidance.push(
                "install strict Rust gates: cargo install cargo-nextest cargo-deny cargo-machete"
                    .into(),
            );
        }
        StackKind::Golang => {
            guidance.push(
                "declare Go version/toolchain and pin golangci-lint/govulncheck in repo tooling before unattended runs".into(),
            );
        }
        StackKind::Python => {
            guidance.push(
                "use uv and pin Python: uv lock && uv add --dev ruff pyright pytest pip-audit deptry".into(),
            );
        }
        StackKind::Mixed => {
            let detected = detected_stacks_for_root(root);
            if detected.is_empty() {
                guidance.push("set explicit stack policy, for example agentactr config set repository.declared_primary_stack typescript".into());
            } else {
                for stack in detected {
                    guidance.extend(setup_guidance_for(root, &stack, false));
                }
                guidance.sort();
                guidance.dedup();
            }
        }
        StackKind::Unknown => {
            guidance.push("declare a stack: agentactr config set repository.declared_primary_stack rust|typescript|golang|python".into());
        }
    }
    guidance
}

fn any_exists(root: &Path, paths: &[&str]) -> bool {
    paths.iter().any(|path| root.join(path).exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn detects_rust_workspace_and_quality_plan() {
        let root = temp_root("rust");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(root.join("Cargo.lock"), "").unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::Rust);
        assert_eq!(inspection.selected_quality_profile, "strict");
        assert!(!inspection.is_empty);
        assert!(inspection
            .quality_plan
            .iter()
            .any(|cmd| cmd.command == "cargo fmt --all -- --check"));
        assert!(inspection.domain_quality_plan.iter().any(|gate| {
            gate.domain == "language.rust"
                && gate.name == "language_rust_fmt"
                && gate.command.as_deref() == Some("cargo fmt --all -- --check")
                && gate.failure_policy == "fail_closed"
        }));
        assert!(!inspection
            .domain_quality_plan
            .iter()
            .any(|gate| gate.name == "language_rust_stack_contract"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_repository_fails_closed_with_guidance() {
        let root = temp_root("empty");

        let inspection = discover_repository(&root);

        assert!(inspection.is_empty);
        assert_eq!(inspection.primary_stack, StackKind::Unknown);
        assert!(inspection
            .setup_guidance
            .iter()
            .any(|item| item.contains("repository.declared_primary_stack")));

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_skips_symlinked_directories() {
        let root = temp_root("symlinked-dir");
        let outside = temp_root("symlinked-dir-outside");
        fs::write(
            outside.join("Cargo.toml"),
            "[package]\nname = \"outside\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked-outside")).unwrap();

        let inspection = discover_repository(&root);

        assert!(inspection.is_empty);
        assert_eq!(inspection.primary_stack, StackKind::Unknown);
        assert!(!inspection
            .evidence_files
            .iter()
            .any(|file| file.contains("Cargo.toml")));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn detects_python_uv_prerequisites() {
        let root = temp_root("python");
        fs::write(root.join("pyproject.toml"), "[project]\nname = \"demo\"\n").unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::Python);
        assert!(inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("uv.lock")));
        assert!(inspection
            .quality_plan
            .iter()
            .any(|cmd| cmd.command == "uv sync --frozen"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn python_detection_accepts_spec_marker_files() {
        let markers = [
            "requirements-dev.txt",
            "Pipfile.lock",
            "setup.py",
            "setup.cfg",
            "tox.ini",
            "noxfile.py",
            "pytest.ini",
            "mypy.ini",
        ];

        for marker in markers {
            let root = temp_root(&format!("python-marker-{marker}").replace('.', "-"));
            fs::write(root.join(marker), "").unwrap();

            let inspection = discover_repository(&root);

            assert_eq!(inspection.primary_stack, StackKind::Python, "{marker}");
            assert!(inspection.evidence_files.iter().any(|file| file == marker));
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn golang_module_without_external_dependencies_does_not_require_go_sum() {
        let root = temp_root("golang-no-external-deps");
        fs::write(root.join("go.mod"), "module example.com/demo\n\ngo 1.24\n").unwrap();
        fs::write(root.join(".golangci.yml"), "version: \"2\"\n").unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::Golang);
        assert!(!inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("go.sum")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn golang_module_in_subdirectory_satisfies_prereqs_and_scopes_quality_plan() {
        let root = temp_root("golang-subdir");
        let service = root.join("services").join("api");
        fs::create_dir_all(&service).unwrap();
        fs::write(root.join("pyproject.toml"), "[project]\nname = \"demo\"\n").unwrap();
        fs::write(root.join("uv.lock"), "").unwrap();
        fs::write(
            service.join("go.mod"),
            "module example.com/demo\n\ngo 1.24\n\nrequire example.com/lib v1.2.3\n",
        )
        .unwrap();
        fs::write(service.join("go.sum"), "").unwrap();
        fs::write(service.join(".golangci.yml"), "version: \"2\"\n").unwrap();
        fs::write(service.join("main.go"), "package main\n").unwrap();

        let inspection = discover_repository(&root);
        let commands = inspection
            .quality_plan
            .iter()
            .map(|cmd| cmd.command.as_str())
            .collect::<Vec<_>>();

        assert_eq!(inspection.primary_stack, StackKind::Mixed);
        assert!(inspection
            .evidence_files
            .iter()
            .any(|file| file == "services/api/go.mod"));
        assert!(!inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("go.mod") || item.contains("go.sum")));
        assert!(commands.contains(&"cd services/api && go mod verify"));
        assert!(commands.contains(&"cd services/api && go test ./..."));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn golang_mise_pin_satisfies_golangci_prereq() {
        let root = temp_root("golang-mise-golangci");
        fs::write(root.join("go.mod"), "module example.com/demo\n\ngo 1.24\n").unwrap();
        fs::write(
            root.join("mise.toml"),
            "[tools]\ngolangci-lint = \"1.64.8\"\n",
        )
        .unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::Golang);
        assert!(!inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("golangci-lint")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn golang_asdf_pin_satisfies_golangci_prereq() {
        let root = temp_root("golang-asdf-golangci");
        fs::write(root.join("go.mod"), "module example.com/demo\n\ngo 1.24\n").unwrap();
        fs::write(root.join(".tool-versions"), "golangci-lint 1.64.8\n").unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::Golang);
        assert!(!inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("golangci-lint")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn golang_repo_tooling_pin_satisfies_golangci_prereq() {
        let root = temp_root("golang-tools-golangci");
        fs::write(root.join("go.mod"), "module example.com/demo\n\ngo 1.24\n").unwrap();
        fs::write(
            root.join("tools.go"),
            "package tools\n\nimport _ \"github.com/golangci/golangci-lint/cmd/golangci-lint\"\n",
        )
        .unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::Golang);
        assert!(!inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("golangci-lint")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn golang_module_with_external_dependencies_requires_go_sum() {
        let root = temp_root("golang-external-deps");
        fs::write(
            root.join("go.mod"),
            "module example.com/demo\n\ngo 1.24\n\nrequire example.com/lib v1.2.3\n",
        )
        .unwrap();
        fs::write(root.join(".golangci.yml"), "version: \"2\"\n").unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::Golang);
        assert!(inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("go.sum")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn golang_module_with_only_local_replacements_does_not_require_go_sum() {
        let root = temp_root("golang-local-replacements");
        fs::write(
            root.join("go.mod"),
            r#"module example.com/demo

go 1.24

require (
    example.com/lib v0.0.0
    example.com/tool v1.2.3
)

replace (
    example.com/lib => ./lib
    example.com/tool v1.2.3 => ../tool
)
"#,
        )
        .unwrap();
        fs::write(root.join(".golangci.yml"), "version: \"2\"\n").unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::Golang);
        assert!(!inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("go.sum")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn golang_module_with_mixed_local_and_external_dependencies_requires_go_sum() {
        let root = temp_root("golang-mixed-replacements");
        fs::write(
            root.join("go.mod"),
            r#"module example.com/demo

go 1.24

require (
    example.com/lib v0.0.0
    example.com/external v1.2.3
)

replace example.com/lib => ./lib
"#,
        )
        .unwrap();
        fs::write(root.join(".golangci.yml"), "version: \"2\"\n").unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::Golang);
        assert!(inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("go.sum")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn golang_version_specific_local_replace_only_matches_same_required_version() {
        let root = temp_root("golang-version-specific-replace");
        fs::write(
            root.join("go.mod"),
            r#"module example.com/demo

go 1.24

require example.com/lib v1.2.3

replace example.com/lib v1.2.2 => ./lib
"#,
        )
        .unwrap();
        fs::write(root.join(".golangci.yml"), "version: \"2\"\n").unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::Golang);
        assert!(inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("go.sum")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn typescript_lockfile_precedence_matches_spec() {
        let root = temp_root("typescript-lockfile-precedence");
        fs::write(root.join(".nvmrc"), "22\n").unwrap();
        fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        fs::write(root.join("yarn.lock"), "").unwrap();
        fs::write(root.join("package-lock.json"), "{}\n").unwrap();
        fs::write(root.join("bun.lockb"), "").unwrap();
        fs::write(root.join("deno.lock"), "").unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"scripts":{"test":"vitest run"}}"#,
        )
        .unwrap();

        let inspection = discover_repository(&root);
        let commands = inspection
            .quality_plan
            .iter()
            .map(|cmd| cmd.command.as_str())
            .collect::<Vec<_>>();

        assert_eq!(inspection.primary_stack, StackKind::TypeScript);
        assert_eq!(
            commands.first().copied(),
            Some("pnpm install --frozen-lockfile")
        );
        assert!(commands.contains(&"pnpm run test"));
        assert!(!commands.contains(&"bun install --frozen-lockfile"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn typescript_quality_plan_uses_detected_package_manager_and_scripts() {
        let root = temp_root("typescript-pnpm");
        fs::write(root.join(".nvmrc"), "22\n").unwrap();
        fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        fs::write(root.join("biome.json"), "{}\n").unwrap();
        fs::write(
            root.join("package.json"),
            r#"{
  "packageManager": "npm@10.8.0",
  "scripts": {
    "lint": "eslint .",
    "typecheck": "tsc --noEmit",
    "build": "vite build",
    "smoke:check": "node smoke.js"
  },
  "devDependencies": {
    "@biomejs/biome": "1.9.4",
    "vite": "5.4.0"
  }
}"#,
        )
        .unwrap();

        let inspection = discover_repository(&root);
        let commands = inspection
            .quality_plan
            .iter()
            .map(|cmd| cmd.command.as_str())
            .collect::<Vec<_>>();

        assert_eq!(inspection.primary_stack, StackKind::TypeScript);
        assert!(commands.contains(&"pnpm install --frozen-lockfile"));
        assert!(commands.contains(&"npx --no-install biome check ."));
        assert!(commands.contains(&"pnpm run lint"));
        assert!(commands.contains(&"pnpm run typecheck"));
        assert!(commands.contains(&"pnpm run build"));
        assert!(commands.contains(&"pnpm run smoke:check"));
        assert!(!commands
            .iter()
            .any(|command| command.contains("package-manager")));
        assert!(!commands.iter().any(|command| command.contains("run dev")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn typescript_quality_plan_omits_missing_optional_scripts() {
        let root = temp_root("typescript-npm");
        fs::write(root.join(".node-version"), "22\n").unwrap();
        fs::write(root.join("package-lock.json"), "{}\n").unwrap();
        fs::write(
            root.join("package.json"),
            r#"{
  "scripts": {
    "test": "vitest run"
  }
}"#,
        )
        .unwrap();

        let inspection = discover_repository(&root);
        let commands = inspection
            .quality_plan
            .iter()
            .map(|cmd| cmd.command.as_str())
            .collect::<Vec<_>>();

        assert_eq!(commands, vec!["npm ci", "npm run test"]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn typescript_accepts_engines_node_pin() {
        let root = temp_root("typescript-engines-node");
        fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"engines":{"node":">=22"},"scripts":{"test":"vitest run"}}"#,
        )
        .unwrap();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();

        let inspection = discover_repository(&root);

        assert_eq!(inspection.primary_stack, StackKind::TypeScript);
        assert!(!inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("requires .nvmrc")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn typescript_quality_plan_scopes_nested_package_root() {
        let root = temp_root("typescript-nested-package");
        let frontend = root.join("frontend");
        fs::create_dir_all(&frontend).unwrap();
        fs::write(root.join(".node-version"), "22\n").unwrap();
        fs::write(frontend.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        fs::write(
            frontend.join("package.json"),
            r#"{"scripts":{"test":"vitest run","build":"vite build"}}"#,
        )
        .unwrap();

        let inspection = discover_repository(&root);
        let commands = inspection
            .quality_plan
            .iter()
            .map(|cmd| cmd.command.as_str())
            .collect::<Vec<_>>();

        assert_eq!(inspection.primary_stack, StackKind::TypeScript);
        assert!(inspection.missing_prerequisites.is_empty());
        assert!(commands.contains(&"cd frontend && pnpm install --frozen-lockfile"));
        assert!(commands.contains(&"cd frontend && pnpm run test"));
        assert!(commands.contains(&"cd frontend && pnpm run build"));
        assert!(!commands.contains(&"npm ci"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn typescript_bun_workspace_nested_packages_inherit_root_manager_without_nested_install() {
        let root = temp_root("typescript-bun-workspace");
        let web = root.join("apps/web");
        fs::create_dir_all(&web).unwrap();
        fs::write(root.join(".node-version"), "26.1.0\n").unwrap();
        fs::write(root.join("bun.lock"), "").unwrap();
        fs::write(
            root.join("package.json"),
            r#"{
  "packageManager": "bun@1.3.13",
  "workspaces": ["apps/*"],
  "scripts": {
    "lint": "biome check ."
  },
  "devDependencies": {
    "@biomejs/biome": "2.4.12"
  }
}"#,
        )
        .unwrap();
        fs::write(
            web.join("package.json"),
            r#"{"name":"@demo/web","scripts":{"typecheck":"tsc --noEmit","build":"next build"},"dependencies":{"next":"16.0.0"}}"#,
        )
        .unwrap();

        let inspection = discover_repository(&root);
        let commands = inspection
            .quality_plan
            .iter()
            .map(|cmd| cmd.command.as_str())
            .collect::<Vec<_>>();

        assert!(commands.contains(&"bun install --frozen-lockfile"));
        assert!(commands.contains(&"bun run lint"));
        assert!(commands.contains(&"cd apps/web && bun run typecheck"));
        assert!(commands.contains(&"cd apps/web && bun run build"));
        assert!(!commands.contains(&"cd apps/web && bun install --frozen-lockfile"));
        assert!(!commands.contains(&"cd apps/web && npm ci"));
        assert!(!commands.iter().any(|command| command == &"npm ci"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deno_only_typescript_does_not_require_node_lockfile() {
        let root = temp_root("typescript-deno-only");
        fs::write(root.join("deno.json"), r#"{"tasks":{"test":"deno test"}}"#).unwrap();
        fs::write(root.join("mod.ts"), "export const value: number = 1;\n").unwrap();

        let inspection =
            apply_declared_stack_to_inspection(discover_repository(&root), &StackKind::TypeScript);

        assert_eq!(inspection.primary_stack, StackKind::TypeScript);
        assert!(inspection.missing_prerequisites.is_empty());
        let commands = inspection
            .quality_plan
            .iter()
            .map(|cmd| cmd.command.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            commands,
            vec!["deno fmt --check", "deno lint", "deno test --frozen"]
        );
        assert!(!commands.iter().any(|command| command.contains("npm")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mixed_quality_plan_uses_only_detected_member_stacks() {
        let root = temp_root("mixed-typescript-python");
        fs::write(root.join(".nvmrc"), "22\n").unwrap();
        fs::write(root.join("package-lock.json"), "{}\n").unwrap();
        fs::write(
            root.join("package.json"),
            r#"{
  "scripts": {
    "test": "vitest run"
  }
}"#,
        )
        .unwrap();
        fs::write(root.join(".python-version"), "3.12\n").unwrap();
        fs::write(root.join("pyproject.toml"), "[project]\nname = \"demo\"\n").unwrap();
        fs::write(root.join("uv.lock"), "").unwrap();

        let inspection = discover_repository(&root);
        let commands = inspection
            .quality_plan
            .iter()
            .map(|cmd| cmd.command.as_str())
            .collect::<Vec<_>>();

        assert_eq!(inspection.primary_stack, StackKind::Mixed);
        assert!(commands.contains(&"npm ci"));
        assert!(commands.contains(&"npm run test"));
        assert!(commands.contains(&"uv sync --frozen"));
        assert!(!commands.iter().any(|command| command.starts_with("cargo ")));
        assert!(!commands.iter().any(|command| command.starts_with("go ")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn declared_stack_recomputes_prerequisites_for_selected_stack() {
        let root = temp_root("declared-rust-over-typescript");
        fs::write(root.join("package.json"), "{}\n").unwrap();

        let inspection = discover_repository(&root);
        let inspection = apply_declared_stack_to_inspection(inspection, &StackKind::Rust);

        assert_eq!(inspection.detected_stack, StackKind::TypeScript);
        assert_eq!(inspection.primary_stack, StackKind::Rust);
        assert_eq!(
            inspection.evidence_files,
            vec!["config:repository.declared_primary_stack=rust".to_string()]
        );
        assert!(inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("Cargo.lock")));
        assert!(!inspection
            .missing_prerequisites
            .iter()
            .any(|item| item.contains("TypeScript")));

        fs::remove_dir_all(root).unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let epoch_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "agentactr-sdk-discovery-{name}-{epoch_nanos}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
