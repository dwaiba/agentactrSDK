use crate::adapters::{
    append_codex_project_profile_overrides, validate_github_repo, GithubRestAdapter,
};
use crate::artifacts::sha256_hex_bytes;
use crate::{
    classify_codex_exec_failure, codex_probe_output_has_error_event, configured_repo_inspection,
    create_dir, flag_value, flag_values, forward_codex_api_key_env, has_flag,
    load_agentactr_config, load_run_artifact_context, new_run_id, require_codex_exec_auth,
    resolve_config_path, run_command_capture_timeout, timestamp_rfc3339_millis, validate_run_id,
    write_file, RunArtifactContext,
};
use agentactr_sdk::{
    AdapterCapabilities, AgentactrConfig, IssueDraftPlanner, IssueFieldValue, IssueId,
    IssueProjectFieldValue, IssueProposal, IssueProposalId, IssueSetArtifactContext,
    IssueSetSource, IssueSubmissionDecision, IssueSubmissionLedgerEntry,
    IssueSubmissionLedgerState, IssueTracker, RepoInspection, StackKind,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub(crate) fn cmd_issue(args: &mut [String]) -> Result<(), String> {
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

pub(crate) fn create_issue_set_context(
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

pub(crate) fn write_issue_set_manifest(
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

pub(crate) fn parse_candidate_query(
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

pub(crate) fn codex_issue_review_status_path(context: &IssueSetArtifactContext) -> PathBuf {
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

pub(crate) fn require_codex_review_for_proposal(
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

pub(crate) fn load_issue_submission_ledger(
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

pub(crate) fn begin_issue_submission(
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

pub(crate) fn with_issue_ledger_pool<T>(
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

pub(crate) async fn ensure_issue_submission_ledger_table(
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

pub(crate) fn ledger_parent_issue_value(parent_issue: Option<u64>) -> i64 {
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
