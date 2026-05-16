#!/usr/bin/env sh
set -eu

forbid_text() {
  pattern=$1
  message=$2
  shift 2
  if rg -n "$pattern" "$@"; then
    printf '%s\n' "$message" >&2
    exit 1
  fi
}

require_text() {
  pattern=$1
  message=$2
  shift 2
  if ! rg -q "$pattern" "$@"; then
    printf '%s\n' "$message" >&2
    exit 1
  fi
}

forbid_text 'reqwest|clap|sqlx|rusqlite|agentactr_cli|agentactr_codex|agentactr_execution|GithubRestAdapter|CodexRuntimeAdapter|LocalGitAdapter|LinuxMemoryController|Command::new|std::process::Command|/sys/fs/cgroup|/proc/pressure|api\.github\.com|CODEX_HOME|docker run' \
  "agentactr-core must not import concrete adapters, CLI, HTTP, SQLite, process, Docker, Codex home, or cgroup details." \
  crates/agentactr-core

forbid_text 'agentactr_codex|CodexRuntimeAdapter|GithubRestAdapter|LocalGitAdapter|LinuxMemoryController|CliCodexMemorySupervisor|std::process::Command|Command::new|use sqlx|sqlx::|use rusqlite|rusqlite::|api\.github\.com|/sys/fs/cgroup|/proc/pressure|docker run' \
  "agentactr-sdk must expose ports, use cases, config, renderers, and planners without concrete default adapters or transport/process details." \
  crates/agentactr-sdk/src

require_text 'pub trait IssueTracker' "agentactr-core must own the IssueTracker port." crates/agentactr-core/src/ports.rs
require_text 'pub trait AgentRuntime' "agentactr-core must own the AgentRuntime port." crates/agentactr-core/src/ports.rs
require_text 'pub trait MemoryController' "agentactr-core must own the MemoryController port." crates/agentactr-core/src/ports.rs
require_text 'pub trait VersionControl' "agentactr-core must own the VersionControl port." crates/agentactr-core/src/ports.rs
require_text 'pub trait PreCommitRunner' "agentactr-core must own the PreCommitRunner port." crates/agentactr-core/src/ports.rs

require_text 'pub struct CodexRuntimeAdapter' "agentactr-codex must own the default Codex runtime adapter." crates/agentactr-codex/src/lib.rs
require_text 'pub struct CodexAppServerAdapter' "agentactr-codex must own the fail-closed app-server adapter surface." crates/agentactr-codex/src/lib.rs
require_text 'pub struct CodexSdkAdapter' "agentactr-codex must own the fail-closed Codex SDK adapter surface." crates/agentactr-codex/src/lib.rs
require_text 'CODEX_HOME' "agentactr-codex must own run-scoped Codex home wiring." crates/agentactr-codex/src/lib.rs

require_text 'pub struct ProcessCommandSpec' "agentactr-execution must own process command specs." crates/agentactr-execution/src/lib.rs
require_text 'pub fn resolve_execution_backend' "agentactr-execution must own execution backend resolution." crates/agentactr-execution/src/lib.rs
require_text 'pub fn docker_command' "agentactr-execution must own Docker command wrapping." crates/agentactr-execution/src/lib.rs
require_text 'pub fn should_pull_image' "agentactr-execution must own Docker image pull policy helpers." crates/agentactr-execution/src/lib.rs

require_text 'struct GithubRestAdapter' "agentactr-cli must own the current default GitHub adapter packaging shortcut." crates/agentactr-cli/src/adapters.rs
require_text 'struct LocalGitAdapter' "agentactr-cli must own the current default local Git adapter packaging shortcut." crates/agentactr-cli/src/adapters.rs
require_text 'struct CliCodexMemorySupervisor' "agentactr-cli must own the current Codex memory supervisor wiring." crates/agentactr-cli/src/adapters.rs
require_text 'struct LinuxMemoryController' "agentactr-cli must own the current Linux memory controller packaging shortcut." crates/agentactr-cli/src/linux_memory.rs
require_text 'memory\.pressure' "Linux memory implementation must read PSI memory pressure evidence." crates/agentactr-cli/src/linux_memory.rs
require_text 'cgroup\.procs' "Linux memory implementation must attach processes through cgroup v2 membership." crates/agentactr-cli/src/linux_memory.rs

require_text 'pub templates: TemplatesConfig' "core config must expose template policy as typed configuration." crates/agentactr-core/src/config.rs
require_text 'pub struct ArchitectureConfig' "core config must expose architecture-domain policy as typed configuration." crates/agentactr-core/src/config.rs
require_text 'pub struct TemplatesConfig' "core config must expose template policy as typed configuration." crates/agentactr-core/src/config.rs
require_text 'standard_label_policy' "core config must expose GitHub standard label policy." crates/agentactr-core/src/config.rs
require_text 'project_automation' "core config must expose GitHub project automation policy." crates/agentactr-core/src/config.rs
require_text 'container_prefix' "core config must expose Docker container naming policy." crates/agentactr-core/src/config.rs
require_text 'setrlimit_address_space' "core config must expose native Linux address-space limits." crates/agentactr-core/src/config.rs
require_text 'setrlimit_file_size' "core config must expose native Linux file-size limits." crates/agentactr-core/src/config.rs

require_text 'github\.standard_label_policy' "SDK renderer must annotate GitHub label policy possible values." crates/agentactr-sdk/src/render.rs
require_text 'architecture\.domains' "SDK renderer must render architecture domain policy." crates/agentactr-sdk/src/render.rs
require_text 'templates\.enabled_domains' "SDK renderer must render template domain policy." crates/agentactr-sdk/src/render.rs
require_text 'container_prefix' "SDK renderer must render Docker container naming policy." crates/agentactr-sdk/src/render.rs
require_text 'linux_memory\.setrlimit_address_space' "SDK renderer must annotate Linux address-space limit policy." crates/agentactr-sdk/src/render.rs
require_text 'linux_memory\.setrlimit_file_size' "SDK renderer must annotate Linux file-size limit policy." crates/agentactr-sdk/src/render.rs

require_text 'oom_policy = "fail_run_preserve_debug_bundle"' "strict Linux OOM policy must stay synchronized across root config, spec, and SDK renderer." agentactr.toml specs_agentactrSDK.md crates/agentactr-sdk/src/render.rs
require_text 'cgroup_v2_required = true' "strict cgroup v2 requirement must stay synchronized across root config, spec, and SDK renderer." agentactr.toml specs_agentactrSDK.md crates/agentactr-sdk/src/render.rs
require_text 'psi_required = true' "strict PSI requirement must stay synchronized across root config, spec, and SDK renderer." agentactr.toml specs_agentactrSDK.md crates/agentactr-sdk/src/render.rs
require_text 'mode = "enforce_on_linux_observe_elsewhere"' "strict Linux memory mode must stay synchronized across root config, spec, and SDK renderer." agentactr.toml specs_agentactrSDK.md crates/agentactr-sdk/src/render.rs

require_text 'specs_agentactrSDK\.md' "AGENTS.md must name the architectural source-of-truth spec." AGENTS.md
require_text 'README\.md' "AGENTS.md must require the living operator README to stay synchronized." AGENTS.md
require_text 'internal_readme/' "AGENTS.md must require README diagrams from internal_readme/." AGENTS.md
require_text 'internal_specs_agentactrSDK/svgs/' "AGENTS.md must require spec diagrams from internal_specs_agentactrSDK/svgs/." AGENTS.md
require_text 'Hexagonal/Clean Architecture' "AGENTS.md must enforce clean architecture." AGENTS.md
require_text 'Dependency Inversion' "AGENTS.md must enforce dependency inversion." AGENTS.md
require_text 'Configuration-driven composition' "AGENTS.md must enforce configuration-driven composition." AGENTS.md

require_text 'internal_specs_agentactrSDK/svgs/sdk_cli_boundary\.svg' "spec must embed the SDK/CLI boundary diagram." specs_agentactrSDK.md
require_text 'agentactr-execution' "spec must document the execution crate in the present implementation state." specs_agentactrSDK.md
require_text 'Domain Graph and Platform Profiles' "spec must document the domain graph architecture surface." specs_agentactrSDK.md
require_text 'quality\.domains' "spec must document domain quality config keys." specs_agentactrSDK.md
require_text 'templates\.agents_policy' "spec must document AGENTS generation policy config keys." specs_agentactrSDK.md
require_text 'github\.standard_label_policy' "spec must document GitHub label mutation policy." specs_agentactrSDK.md
require_text 'linux_memory\.setrlimit_address_space' "spec must document native Linux limit policy." specs_agentactrSDK.md
require_text 'execution\.backend' "spec must document execution backend policy." specs_agentactrSDK.md

require_text '!\[Present repository architecture\]\(internal_readme/architecture\.svg\)' "README must embed the present architecture diagram from internal_readme/." README.md
require_text '`agentactr-core`' "README must document agentactr-core." README.md
require_text '`agentactr-sdk`' "README must document agentactr-sdk." README.md
require_text '`agentactr-codex`' "README must document agentactr-codex." README.md
require_text '`agentactr-execution`' "README must document agentactr-execution." README.md
require_text '`agentactr-cli`' "README must document agentactr-cli." README.md

require_text 'agentactr-core' "architecture.svg must show agentactr-core." internal_readme/architecture.svg
require_text 'agentactr-sdk' "architecture.svg must show agentactr-sdk." internal_readme/architecture.svg
require_text 'agentactr-codex' "architecture.svg must show agentactr-codex." internal_readme/architecture.svg
require_text 'agentactr-execution' "architecture.svg must show agentactr-execution." internal_readme/architecture.svg
require_text 'agentactr CLI' "architecture.svg must show the CLI product boundary." internal_readme/architecture.svg
require_text 'CLI-local adapters' "architecture.svg must show temporary CLI-local adapters." internal_readme/architecture.svg
require_text 'Linux memory' "architecture.svg must show the Linux memory adapter boundary." internal_readme/architecture.svg
require_text 'domain graph' "architecture.svg must show the current domain graph SDK surface." internal_readme/architecture.svg
require_text 'issue planners' "architecture.svg must show the current issue planning SDK surface." internal_readme/architecture.svg
require_text 'run-scoped CODEX_HOME' "architecture.svg must show run-scoped Codex home ownership." internal_readme/architecture.svg
require_text 'Docker command wrapper' "architecture.svg must show Docker wrapping in agentactr-execution." internal_readme/architecture.svg
require_text 'provider-neutral process spec' "architecture.svg must show provider-neutral process specs." internal_readme/architecture.svg
forbid_text 'agentactr-codex.*Docker backend wrapper|Docker backend wrapper.*agentactr-codex' \
  "architecture.svg must not attribute Docker backend wrapping to agentactr-codex." \
  internal_readme/architecture.svg

require_text 'scripts/check-architecture-boundaries\.sh' "architecture workflow must delegate to the shared boundary script." .github/workflows/architecture.yml
forbid_text 'reqwest\|clap\|sqlx|GithubRestAdapter|CodexRuntimeAdapter|oom_policy = "fail_run_preserve_debug_bundle"' \
  "architecture workflow must not duplicate boundary regexes; keep checks in scripts/check-architecture-boundaries.sh." \
  .github/workflows/architecture.yml

require_text 'id: architecture-boundaries' "pre-commit config must include the architecture boundary hook." .pre-commit-config.yaml
require_text 'entry: scripts/check-architecture-boundaries\.sh' "pre-commit architecture hook must call the shared boundary script." .pre-commit-config.yaml
