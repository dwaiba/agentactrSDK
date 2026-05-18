#[derive(Clone, Copy)]
pub(crate) struct CommandCatalogEntry {
    pub(crate) command: &'static str,
    pub(crate) status: &'static str,
    pub(crate) purpose: &'static str,
    pub(crate) side_effects: &'static str,
    pub(crate) required_credentials: &'static [&'static str],
    pub(crate) platform_constraints: &'static [&'static str],
    pub(crate) sdk_use_case_owner: &'static str,
}

pub(crate) fn command_catalog() -> &'static [CommandCatalogEntry] {
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
            command: "init --yes [--repo OWNER/REPO] [--codex-auth auto|chatgpt|api-key]",
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
            command: "bootstrap project --stack python|golang|rust|typescript|pulumi|terraform|sql --yes [--force] [--allow-non-empty]",
            status: "implemented",
            purpose: "Scaffold a blank project with stack-specific tools, pre-commit hooks, tests, and helper start commands.",
            side_effects: "writes_scaffold_files_refuses_non_empty_or_overwrite_without_explicit_flags",
            required_credentials: &[],
            platform_constraints: &[],
            sdk_use_case_owner: "bootstrap_project_templates",
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
            command: "issue draft (--repo OWNER/REPO|--local) [--prompt TEXT|--prompt-file PATH] --stack STACK [--framework nextjs|none] [--domain DOMAIN] [--parent ISSUE_NUMBER] [--artifact-root PATH] [--codex-draft] [--codex-review] [--json]",
            status: "implemented",
            purpose: "Draft tracker-backed or tracker-offline local issue proposals from repo evidence, deterministic stack/domain template policy, or read-only Codex-authored structured output.",
            side_effects: "creates_issue_set_artifacts_and_optional_codex_draft_or_review_artifacts_local_mode_never_calls_tracker",
            required_credentials: &[
                "github_token_for_tracker_backed_draft_or_submit",
                "codex_auth_when_codex_draft_or_review_enabled",
            ],
            platform_constraints: &["github_rest_for_tracker_backed_dedupe_inventory", "codex_cli_for_codex_draft_or_review"],
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
            command: "issue submit ISSUE_SET_ID --proposal PROPOSAL_ID --yes [--repo OWNER/REPO for local issue sets] [--resume] [--allow-possible-duplicate --reason REASON] [--require-codex-review]",
            status: "implemented",
            purpose: "Submit or resume one review-gated issue proposal through capability-gated tracker ports, recomputing deferred local dedupe against the explicit target repo before mutation.",
            side_effects: "writes_sqlite_and_may_mutate_tracker_when_adapter_supports_issue_create_link",
            required_credentials: &["github_token"],
            platform_constraints: &["recorded_issue_set_or_run_artifacts"],
            sdk_use_case_owner: "issue_submission",
        },
        CommandCatalogEntry {
            command: "tui run RUN_ID [--refresh 1s] [--snapshot] [--no-color]",
            status: "implemented",
            purpose: "Render a read-only terminal view of one run's agent graph, trace, quality, and lifecycle artifacts.",
            side_effects: "read_only_artifact_trace_rendering",
            required_credentials: &[],
            platform_constraints: &["recorded_run_artifacts"],
            sdk_use_case_owner: "cli_operator_visibility",
        },
        CommandCatalogEntry {
            command: "tui latest [--refresh 1s] [--no-color]",
            status: "implemented",
            purpose: "Resolve the latest run from trace timestamps and render the read-only terminal visibility view.",
            side_effects: "read_only_artifact_trace_rendering",
            required_credentials: &[],
            platform_constraints: &["recorded_trace_and_run_artifacts"],
            sdk_use_case_owner: "cli_operator_visibility",
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
            command: "vcs apply RUN_ID --check|--yes [--3way] [--allow-dirty]",
            status: "implemented",
            purpose: "Validate or apply a recorded run patch into the current source checkout with conflict-aware diagnostics.",
            side_effects: "read_only_check_or_writes_source_checkout_when_yes",
            required_credentials: &[],
            platform_constraints: &["git", "recorded_worktree"],
            sdk_use_case_owner: "bootstrap_vcs_patch_apply",
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

pub(crate) fn cmd_commands(args: &[String]) -> Result<(), String> {
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

pub(crate) fn command_catalog_entry_json(entry: &CommandCatalogEntry) -> serde_json::Value {
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

pub(crate) fn cmd_menu(args: &[String]) -> Result<(), String> {
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

pub(crate) fn menu_json_payload() -> serde_json::Value {
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
