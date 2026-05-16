use agentactr_core::{
    redaction_safe_issue_marker, FrameworkDeclaration, Issue, IssueCreateRequest,
    IssueDedupeStatus, IssueDraftPlanner, IssueDraftRequest, IssueDraftResult, IssueId,
    IssueLinkRequest, IssueProjectFieldValue, IssueProposal, IssueProposalId,
    IssueSubmissionLedgerEntry, IssueSubmissionLedgerKey, IssueSubmissionLedgerState,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IssueSubmissionDecision {
    Create(Box<IssueCreateRequest>),
    RecoverSubmitted(Box<IssueCreateRequest>),
    Link(IssueLinkRequest),
    AlreadyLinked,
    Blocked(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IssueSubmissionBeginDecision {
    InsertSubmitted,
    TransitionPendingToSubmitted,
    Blocked(String),
}

pub fn issue_submission_key(
    issue_set_id: &str,
    proposal: &IssueProposal,
) -> IssueSubmissionLedgerKey {
    IssueSubmissionLedgerKey {
        run_id: issue_set_id.to_string(),
        issue_set_id: issue_set_id.to_string(),
        proposal_id: proposal.proposal_id.clone(),
        repo: proposal.repo.clone(),
        parent_issue: proposal.parent_issue,
        parent_issue_key: parent_issue_key(proposal.parent_issue),
        proposal_digest: proposal.digest.clone(),
    }
}

pub fn parent_issue_key(parent_issue: Option<u64>) -> String {
    parent_issue
        .map(|issue| format!("parent:{issue}"))
        .unwrap_or_else(|| "top_level".to_string())
}

#[derive(Clone, Debug, Default)]
pub struct DeterministicIssueDraftPlanner;

impl IssueDraftPlanner for DeterministicIssueDraftPlanner {
    fn draft(&self, req: IssueDraftRequest) -> Result<IssueDraftResult, String> {
        draft_issue_proposals(req)
    }
}

pub fn draft_issue_proposals(req: IssueDraftRequest) -> Result<IssueDraftResult, String> {
    let stack = req.stack.as_deref().unwrap_or("unknown").trim();
    if stack.is_empty() || stack == "unknown" {
        return Err(
            "issue drafting requires an explicit stack or repository.declared_primary_stack"
                .to_string(),
        );
    }
    let prompt_digest = req
        .prompt
        .as_ref()
        .map(|prompt| sha256_hex(prompt.as_bytes()));
    if let (Some(prompt), Some(prompt_sha)) = (req.prompt.as_deref(), prompt_digest.as_deref()) {
        let proposals = prompt_issue_templates(prompt, &req, stack, prompt_sha)
            .into_iter()
            .map(|template| build_issue_proposal(&req, template))
            .collect::<Vec<_>>();
        return Ok(IssueDraftResult {
            proposals,
            discarded_partial_output: false,
            detail:
                "deterministic prompt issue draft planner completed with schema-valid proposals"
                    .to_string(),
        });
    }

    let title = format!("Bootstrap {stack} project prerequisites for agentactr");
    let body = format!(
        "Prepare this {stack} project so agentactr can run deterministic quality checks and issue-driven automation.\n\nThis proposal was generated from deterministic SDK policy using repository configuration and issue inventory evidence."
    );
    let provenance = vec!["repo_evidence:deterministic_no_prompt".to_string()];
    let template = DraftIssueTemplate {
        slug: "bootstrap-prerequisites".to_string(),
        title,
        body,
        provenance,
    };
    Ok(IssueDraftResult {
        proposals: vec![build_issue_proposal(&req, template)],
        discarded_partial_output: false,
        detail: "deterministic issue draft planner completed with schema-valid proposals"
            .to_string(),
    })
}

pub fn draft_issue_proposals_from_structured_json(
    req: IssueDraftRequest,
    raw_response: &str,
    planner: &str,
) -> Result<IssueDraftResult, String> {
    let stack = req.stack.as_deref().unwrap_or("unknown").trim();
    if stack.is_empty() || stack == "unknown" {
        return Err("Codex issue drafting requires an explicit stack".to_string());
    }
    let value = parse_structured_issue_draft_response(raw_response)?;
    let proposals = value
        .get("proposals")
        .and_then(Value::as_array)
        .ok_or("Codex issue draft response must contain a proposals array")?;
    if proposals.is_empty() {
        return Err("Codex issue draft response contained no proposals".to_string());
    }
    if proposals.len() > 50 {
        return Err("Codex issue draft response exceeded the 50 proposal limit".to_string());
    }
    let prompt_sha = req
        .prompt
        .as_ref()
        .map(|prompt| sha256_hex(prompt.as_bytes()));
    let mut parsed = Vec::with_capacity(proposals.len());
    for (index, value) in proposals.iter().enumerate() {
        let object = value
            .as_object()
            .ok_or_else(|| format!("proposal {} must be a JSON object", index + 1))?;
        let title = required_trimmed_str(value, "title", 180)?;
        let body = issue_body_from_structured_proposal(value)?;
        if body.chars().count() > 60_000 {
            return Err(format!(
                "proposal {} body exceeds 60000 characters",
                index + 1
            ));
        }
        if let Some(repo) = object.get("repo").and_then(Value::as_str) {
            if repo != req.repo {
                return Err(format!(
                    "proposal {} repo `{repo}` does not match requested repo `{}`",
                    index + 1,
                    req.repo
                ));
            }
        }
        if let Some(parent) = object.get("parent_issue") {
            let parent = if parent.is_null() {
                None
            } else {
                Some(parent.as_u64().ok_or_else(|| {
                    format!(
                        "proposal {} parent_issue must be a number or null",
                        index + 1
                    )
                })?)
            };
            if parent != req.parent_issue {
                return Err(format!(
                    "proposal {} parent_issue does not match requested parent",
                    index + 1
                ));
            }
        }
        let labels = optional_string_array(value, "labels", 50, 50)?;
        let assignees = optional_string_array(value, "assignees", 20, 100)?;
        let milestone = optional_trimmed_str(value, "milestone", 100)?;
        let issue_type = optional_trimmed_str(value, "issue_type", 100)?;
        let project_fields = optional_project_fields(value)?;
        let provenance = structured_provenance(planner, prompt_sha.as_deref(), index, value)?;
        let digest = proposal_digest(ProposalDigestInput {
            repo: &req.repo,
            parent_issue: req.parent_issue,
            title: &title,
            body: &body,
            labels: &labels,
            assignees: &assignees,
            milestone: milestone.as_deref(),
            issue_type: issue_type.as_deref(),
            issue_field_values: &[],
            project_fields: &project_fields,
            framework: req.framework.as_ref(),
            provenance: &provenance,
        });
        let (dedupe, related_issues) = classify_issue_dedupe(&title, &req.candidates);
        parsed.push(IssueProposal {
            proposal_id: IssueProposalId::new(format!(
                "proposal-{}",
                &sha256_hex(
                    format!(
                        "{}\n{}\n{}\n{}\n{}",
                        req.issue_set_id, req.repo, planner, index, title
                    )
                    .as_bytes()
                )[..12]
            )),
            repo: req.repo.clone(),
            parent_issue: req.parent_issue,
            title,
            body,
            labels,
            assignees,
            milestone,
            issue_type,
            issue_field_values: Vec::new(),
            project_fields,
            digest,
            dedupe,
            framework: req.framework.clone(),
            related_issues,
            provenance,
        });
    }
    Ok(IssueDraftResult {
        proposals: parsed,
        discarded_partial_output: false,
        detail: format!("{planner} produced schema-valid issue proposals"),
    })
}

fn parse_structured_issue_draft_response(raw_response: &str) -> Result<Value, String> {
    let trimmed = raw_response.trim();
    if trimmed.is_empty() {
        return Err("Codex issue draft response is empty".to_string());
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }
    if let Some(fenced) = extract_json_fence(trimmed) {
        if let Ok(value) = serde_json::from_str::<Value>(fenced.trim()) {
            return Ok(value);
        }
    }
    Err("Codex issue draft response was not valid JSON".to_string())
}

fn extract_json_fence(value: &str) -> Option<&str> {
    let start = value.find("```json")?;
    let after_start = &value[start + "```json".len()..];
    let after_newline = after_start.strip_prefix('\n').unwrap_or(after_start);
    let end = after_newline.find("```")?;
    Some(&after_newline[..end])
}

fn required_trimmed_str(value: &Value, key: &str, max_chars: usize) -> Result<String, String> {
    let text = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("proposal is missing string field `{key}`"))?
        .trim();
    if text.is_empty() {
        return Err(format!("proposal field `{key}` cannot be empty"));
    }
    if text.chars().count() > max_chars {
        return Err(format!(
            "proposal field `{key}` exceeds {max_chars} characters"
        ));
    }
    Ok(text.to_string())
}

fn optional_trimmed_str(
    value: &Value,
    key: &str,
    max_chars: usize,
) -> Result<Option<String>, String> {
    let Some(raw) = value.get(key) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let text = raw
        .as_str()
        .ok_or_else(|| format!("proposal field `{key}` must be a string when present"))?
        .trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.chars().count() > max_chars {
        return Err(format!(
            "proposal field `{key}` exceeds {max_chars} characters"
        ));
    }
    Ok(Some(text.to_string()))
}

fn optional_string_array(
    value: &Value,
    key: &str,
    max_items: usize,
    max_chars: usize,
) -> Result<Vec<String>, String> {
    let Some(raw) = value.get(key) else {
        return Ok(Vec::new());
    };
    if raw.is_null() {
        return Ok(Vec::new());
    }
    let items = raw
        .as_array()
        .ok_or_else(|| format!("proposal field `{key}` must be an array when present"))?;
    if items.len() > max_items {
        return Err(format!("proposal field `{key}` exceeds {max_items} items"));
    }
    let mut parsed = Vec::new();
    for item in items {
        let text = item
            .as_str()
            .ok_or_else(|| format!("proposal field `{key}` must contain only strings"))?
            .trim();
        if text.is_empty() {
            continue;
        }
        if text.chars().count() > max_chars {
            return Err(format!(
                "proposal field `{key}` contains an item over {max_chars} characters"
            ));
        }
        parsed.push(text.to_string());
    }
    parsed.sort();
    parsed.dedup();
    Ok(parsed)
}

fn optional_project_fields(value: &Value) -> Result<Vec<IssueProjectFieldValue>, String> {
    let mut fields = Vec::new();
    if let Some(priority) = optional_trimmed_str(value, "priority", 20)? {
        validate_project_field_value("Priority", &priority)?;
        fields.push(IssueProjectFieldValue {
            field_name: "Priority".to_string(),
            value: priority,
        });
    }
    if let Some(size) = optional_trimmed_str(value, "size", 20)? {
        validate_project_field_value("Size", &size)?;
        fields.push(IssueProjectFieldValue {
            field_name: "Size".to_string(),
            value: size,
        });
    }
    let Some(raw_fields) = value.get("project_fields") else {
        fields.sort_by(|a, b| a.field_name.cmp(&b.field_name));
        fields.dedup_by(|a, b| a.field_name == b.field_name);
        return Ok(fields);
    };
    if raw_fields.is_null() {
        fields.sort_by(|a, b| a.field_name.cmp(&b.field_name));
        fields.dedup_by(|a, b| a.field_name == b.field_name);
        return Ok(fields);
    }
    let raw_fields = raw_fields
        .as_array()
        .ok_or("proposal field `project_fields` must be an array when present")?;
    if raw_fields.len() > 20 {
        return Err("proposal field `project_fields` exceeds 20 items".to_string());
    }
    for field in raw_fields {
        let field_name = field
            .get("field_name")
            .or_else(|| field.get("name"))
            .and_then(Value::as_str)
            .ok_or("project_fields entries require string field_name")?
            .trim();
        let field_value = field
            .get("value")
            .and_then(Value::as_str)
            .ok_or("project_fields entries require string value")?
            .trim();
        if field_name.is_empty() || field_value.is_empty() {
            continue;
        }
        if field_name.chars().count() > 80 || field_value.chars().count() > 80 {
            return Err(
                "project_fields entries must be at most 80 characters per name/value".to_string(),
            );
        }
        validate_project_field_value(field_name, field_value)?;
        fields.retain(|existing| existing.field_name != field_name);
        fields.push(IssueProjectFieldValue {
            field_name: field_name.to_string(),
            value: field_value.to_string(),
        });
    }
    fields.sort_by(|a, b| a.field_name.cmp(&b.field_name));
    Ok(fields)
}

fn validate_project_field_value(field_name: &str, value: &str) -> Result<(), String> {
    match field_name {
        "Priority" if !matches!(value, "P0" | "P1" | "P2") => {
            Err("project field `Priority` must be one of P0, P1, or P2".to_string())
        }
        "Size" if !matches!(value, "XS" | "S" | "M" | "L" | "XL") => {
            Err("project field `Size` must be one of XS, S, M, L, or XL".to_string())
        }
        _ => Ok(()),
    }
}

fn issue_body_from_structured_proposal(value: &Value) -> Result<String, String> {
    if let Some(body) = optional_trimmed_str(value, "body", 60_000)? {
        return Ok(body);
    }
    let summary = optional_trimmed_str(value, "summary", 2_000)?.unwrap_or_default();
    let scope = optional_string_array(value, "scope", 20, 1_000)?;
    let acceptance = optional_string_array(value, "acceptance_criteria", 20, 1_000)?;
    if summary.is_empty() && scope.is_empty() && acceptance.is_empty() {
        return Err(
            "proposal must include body, or summary plus scope/acceptance_criteria".to_string(),
        );
    }
    let mut body = String::new();
    if !summary.is_empty() {
        body.push_str(&summary);
        body.push_str("\n\n");
    }
    if !scope.is_empty() {
        body.push_str("Scope:\n");
        body.push_str(&markdown_bullets_from_strings(&scope));
        body.push_str("\n\n");
    }
    if !acceptance.is_empty() {
        body.push_str("Acceptance criteria:\n");
        body.push_str(&markdown_bullets_from_strings(&acceptance));
    }
    Ok(body.trim().to_string())
}

fn markdown_bullets_from_strings(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn structured_provenance(
    planner: &str,
    prompt_sha: Option<&str>,
    index: usize,
    value: &Value,
) -> Result<Vec<String>, String> {
    let mut provenance = vec![
        format!("planner:{planner}"),
        format!("planner_proposal_index:{}", index + 1),
    ];
    if let Some(prompt_sha) = prompt_sha {
        provenance.push(format!("prompt_sha256:{prompt_sha}"));
    }
    for item in optional_string_array(value, "provenance", 20, 200)? {
        provenance.push(format!("llm:{item}"));
    }
    Ok(provenance)
}

struct DraftIssueTemplate {
    slug: String,
    title: String,
    body: String,
    provenance: Vec<String>,
}

fn build_issue_proposal(req: &IssueDraftRequest, template: DraftIssueTemplate) -> IssueProposal {
    let (dedupe, related_issues) = classify_issue_dedupe(&template.title, &req.candidates);
    let digest = proposal_digest(ProposalDigestInput {
        repo: &req.repo,
        parent_issue: req.parent_issue,
        title: &template.title,
        body: &template.body,
        labels: &[],
        assignees: &[],
        milestone: None,
        issue_type: None,
        issue_field_values: &[],
        project_fields: &[],
        framework: req.framework.as_ref(),
        provenance: &template.provenance,
    });
    IssueProposal {
        proposal_id: IssueProposalId::new(format!(
            "proposal-{}",
            &sha256_hex(
                format!(
                    "{}\n{}\n{}\n{}",
                    req.issue_set_id, req.repo, template.slug, template.title
                )
                .as_bytes()
            )[..12]
        )),
        repo: req.repo.clone(),
        parent_issue: req.parent_issue,
        title: template.title,
        body: template.body,
        labels: Vec::new(),
        assignees: Vec::new(),
        milestone: None,
        issue_type: None,
        issue_field_values: Vec::new(),
        project_fields: Vec::new(),
        digest,
        dedupe,
        framework: req.framework.clone(),
        related_issues,
        provenance: template.provenance,
    }
}

fn prompt_issue_templates(
    prompt: &str,
    req: &IssueDraftRequest,
    stack: &str,
    prompt_sha: &str,
) -> Vec<DraftIssueTemplate> {
    let normalized = prompt.to_lowercase();
    let framework_id = req
        .framework
        .as_ref()
        .map(|framework| framework.id.to_lowercase())
        .unwrap_or_default();
    let is_nextjs = framework_id == "nextjs";
    let mut templates = Vec::new();

    if is_nextjs && contains_any(&normalized, &["scalable", "architecture", "structure"]) {
        templates.push(nextjs_prompt_template(
            req,
            stack,
            prompt_sha,
            "nextjs-architecture",
            "Define scalable Next.js TypeScript application architecture",
            &[
                "Document the application module boundaries for UI, route handlers, data access, validation, and shared packages.",
                "Define a route and folder convention that supports future CRUD resources without ad hoc placement.",
                "Capture configuration, environment, and observability boundaries needed before implementation work starts.",
            ],
            &[
                "Architecture notes identify the app router areas, API route-handler areas, and shared TypeScript package boundaries.",
                "The plan includes where validation schemas, data adapters, and feature modules live.",
                "The plan is small enough for follow-up implementation issues to proceed independently.",
            ],
        ));
    }

    if is_nextjs
        && contains_any(
            &normalized,
            &["backend", "route handler", "route handlers", "api"],
        )
    {
        templates.push(nextjs_prompt_template(
            req,
            stack,
            prompt_sha,
            "nextjs-route-handlers",
            "Implement typed Next.js route handlers for CRUD APIs",
            &[
                "Create the backend route-handler structure for one representative CRUD resource.",
                "Use typed request parsing, response shaping, and deterministic error responses.",
                "Keep persistence behind a narrow data-access interface so storage can be swapped later.",
            ],
            &[
                "Route handlers expose create, read, update, and delete operations for the chosen resource.",
                "Invalid input returns typed validation errors without leaking internals.",
                "The implementation has focused tests or documented test commands for success and failure paths.",
            ],
        ));
    }

    if is_nextjs && contains_any(&normalized, &["crud", "create", "read", "update", "delete"]) {
        templates.push(nextjs_prompt_template(
            req,
            stack,
            prompt_sha,
            "nextjs-crud-domain",
            "Add CRUD domain model, validation, and data-access contracts",
            &[
                "Define a representative domain entity for the IELTS study companion workflow.",
                "Add schema validation for create and update inputs.",
                "Expose data-access contracts that route handlers can call without coupling to a concrete database client.",
            ],
            &[
                "Create/update payloads are validated through shared TypeScript schemas.",
                "The data-access surface supports list, get, create, update, and delete operations.",
                "Domain and persistence code can be tested independently of Next.js request handling.",
            ],
        ));
        templates.push(nextjs_prompt_template(
            req,
            stack,
            prompt_sha,
            "nextjs-crud-ui",
            "Build Next.js CRUD UI flows for the selected resource",
            &[
                "Create the list, detail, create, and edit UI flow for the selected CRUD resource.",
                "Connect UI actions to the typed API route handlers.",
                "Represent loading, empty, validation-error, and failure states clearly.",
            ],
            &[
                "Users can list, create, edit, and delete records through the UI.",
                "Client-side and server-side validation errors are visible and actionable.",
                "The UI uses the repository's existing component and styling conventions.",
            ],
        ));
    }

    if is_nextjs && contains_any(&normalized, &["routing", "router", "routes", "navigation"]) {
        templates.push(nextjs_prompt_template(
            req,
            stack,
            prompt_sha,
            "nextjs-routing",
            "Create a scalable Next.js routing structure for feature workflows",
            &[
                "Define app routes for the primary IELTS study companion workflows and CRUD screens.",
                "Group related routes so feature areas can grow without route sprawl.",
                "Add navigation entry points that make the new workflows reachable.",
            ],
            &[
                "Routes are organized using the project's Next.js app-router conventions.",
                "Navigation reaches the new feature screens without hardcoded one-off paths.",
                "Route names and folders are documented enough for future issues to extend.",
            ],
        ));
    }

    if is_nextjs
        && contains_any(
            &normalized,
            &["auth", "authentication", "authorization", "permission"],
        )
    {
        templates.push(nextjs_prompt_template(
            req,
            stack,
            prompt_sha,
            "nextjs-auth-ready-boundaries",
            "Add authentication-ready boundaries for Next.js CRUD workflows",
            &[
                "Introduce authorization guard seams around route handlers and protected UI flows.",
                "Keep the initial implementation compatible with the current auth state of the repository.",
                "Document how a concrete auth provider can be connected later without rewriting CRUD code.",
            ],
            &[
                "Protected route handlers call an auth boundary before mutating data.",
                "UI flows have a clear place to handle unauthenticated or unauthorized states.",
                "The auth boundary is interface-driven and does not force a provider choice in this issue.",
            ],
        ));
    }

    if templates.is_empty() {
        templates.push(nextjs_prompt_template(
            req,
            stack,
            prompt_sha,
            "reviewed-prompt-implementation",
            &format!("Implement reviewed {stack} work from local issue draft"),
            &[
                "Turn the reviewed prompt artifact into a focused implementation plan.",
                "Keep source changes scoped to the repository stack and configured framework.",
                "Record follow-up issues if the implementation cannot be completed in one safe change.",
            ],
            &[
                "The issue body references only redacted prompt artifacts and prompt digest.",
                "Implementation scope is explicit and independently reviewable.",
                "Quality checks required by the repository are listed before execution.",
            ],
        ));
    }

    templates
}

fn nextjs_prompt_template(
    req: &IssueDraftRequest,
    stack: &str,
    prompt_sha: &str,
    slug: &str,
    title: &str,
    scope: &[&str],
    acceptance: &[&str],
) -> DraftIssueTemplate {
    let body = format!(
        "Drafted from a reviewed operator prompt for `{}`.\n\nRaw prompt persistence is disabled by default. Review `planner_prompt.redacted.txt` and `planner_prompt_metadata.json` in the issue-set artifact before submission.\n\nPrompt digest: `sha256:{prompt_sha}`\nStack: `{stack}`{}\n\nScope:\n{}\n\nAcceptance criteria:\n{}",
        req.repo,
        framework_suffix(req.framework.as_ref()),
        markdown_bullets(scope),
        markdown_bullets(acceptance),
    );
    DraftIssueTemplate {
        slug: slug.to_string(),
        title: title.to_string(),
        body,
        provenance: vec![
            format!("prompt_sha256:{prompt_sha}"),
            format!("prompt_template:{slug}"),
        ],
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn markdown_bullets(items: &[&str]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn framework_suffix(framework: Option<&FrameworkDeclaration>) -> String {
    framework
        .map(|framework| {
            format!(
                "\nFramework: `{}/{}`{}",
                framework.ecosystem,
                framework.id,
                framework
                    .version_or_profile
                    .as_ref()
                    .map(|version| format!(" `{version}`"))
                    .unwrap_or_default()
            )
        })
        .unwrap_or_default()
}

fn classify_issue_dedupe(title: &str, candidates: &[Issue]) -> (IssueDedupeStatus, Vec<IssueId>) {
    let normalized = normalize_issue_title(title);
    let mut related = Vec::new();
    let mut possible = false;
    for candidate in candidates {
        let candidate_title = normalize_issue_title(&candidate.title);
        if candidate_title == normalized {
            related.push(IssueId(format!("{}#{}", candidate.repo, candidate.number)));
            return (IssueDedupeStatus::DuplicateBlocked, related);
        }
        if !normalized.is_empty()
            && (candidate_title.contains(&normalized) || normalized.contains(&candidate_title))
        {
            possible = true;
            related.push(IssueId(format!("{}#{}", candidate.repo, candidate.number)));
        }
    }
    if possible {
        (IssueDedupeStatus::PossibleDuplicate, related)
    } else {
        (IssueDedupeStatus::Unique, related)
    }
}

fn normalize_issue_title(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_lowercase()
}

struct ProposalDigestInput<'a> {
    repo: &'a str,
    parent_issue: Option<u64>,
    title: &'a str,
    body: &'a str,
    labels: &'a [String],
    assignees: &'a [String],
    milestone: Option<&'a str>,
    issue_type: Option<&'a str>,
    issue_field_values: &'a [agentactr_core::IssueFieldValue],
    project_fields: &'a [IssueProjectFieldValue],
    framework: Option<&'a FrameworkDeclaration>,
    provenance: &'a [String],
}

fn proposal_digest(input: ProposalDigestInput<'_>) -> String {
    let mut labels = input.labels.to_vec();
    labels.sort();
    let mut assignees = input.assignees.to_vec();
    assignees.sort();
    let mut fields = input
        .issue_field_values
        .iter()
        .map(|field| {
            format!(
                "{}:{}:{}",
                field.field_id,
                field.value_type.clone().unwrap_or_default(),
                field.value
            )
        })
        .collect::<Vec<_>>();
    fields.sort();
    let mut project_fields = input
        .project_fields
        .iter()
        .map(|field| format!("{}:{}", field.field_name, field.value))
        .collect::<Vec<_>>();
    project_fields.sort();
    let mut provenance = input.provenance.to_vec();
    provenance.sort();
    let framework = input
        .framework
        .map(|framework| {
            format!(
                "{}:{}:{}",
                framework.ecosystem,
                framework.id,
                framework.version_or_profile.clone().unwrap_or_default()
            )
        })
        .unwrap_or_default();
    sha256_hex(
        format!(
            "repo={repo}\nparent={}\ntitle={title}\nbody={body}\nlabels={}\nassignees={}\nmilestone={}\ntype={}\nfields={}\nproject_fields={}\nprovenance={}\nframework={framework}\n",
            parent_issue_key(input.parent_issue),
            labels.join("\u{1f}"),
            assignees.join("\u{1f}"),
            input.milestone.unwrap_or_default(),
            input.issue_type.unwrap_or_default(),
            fields.join("\u{1f}"),
            project_fields.join("\u{1f}"),
            provenance.join("\u{1f}"),
            repo = input.repo,
            title = input.title,
            body = input.body,
        )
        .as_bytes(),
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn validate_issue_submission_policy(
    proposal: &IssueProposal,
    allow_possible_duplicate: bool,
    duplicate_reason: Option<&str>,
) -> Result<(), String> {
    match proposal.dedupe {
        IssueDedupeStatus::Unique => Ok(()),
        IssueDedupeStatus::DuplicateBlocked => Err(format!(
            "proposal {} is duplicate_blocked and cannot be submitted",
            proposal.proposal_id.as_str()
        )),
        IssueDedupeStatus::PossibleDuplicate => {
            if allow_possible_duplicate {
                let reason = duplicate_reason.unwrap_or("").trim();
                if reason.is_empty() {
                    Err(
                        "--allow-possible-duplicate requires --reason to record operator rationale"
                            .to_string(),
                    )
                } else {
                    Ok(())
                }
            } else {
                Err(format!(
                    "proposal {} is possible_duplicate; pass --allow-possible-duplicate --reason TEXT after review",
                    proposal.proposal_id.as_str()
                ))
            }
        }
    }
}

pub fn plan_issue_submission_begin(
    existing: Option<&IssueSubmissionLedgerEntry>,
) -> IssueSubmissionBeginDecision {
    match existing {
        None => IssueSubmissionBeginDecision::InsertSubmitted,
        Some(existing) if existing.state == IssueSubmissionLedgerState::Pending => {
            IssueSubmissionBeginDecision::TransitionPendingToSubmitted
        }
        Some(existing) => IssueSubmissionBeginDecision::Blocked(format!(
            "issue proposal {} already has ledger state {}; use --resume for recoverable created/unlinked issues",
            existing.key.proposal_id.as_str(),
            existing.state.as_str()
        )),
    }
}

pub fn plan_issue_submission(
    issue_set_id: &str,
    proposal: IssueProposal,
    existing: Option<&IssueSubmissionLedgerEntry>,
    resume: bool,
) -> IssueSubmissionDecision {
    if let Some(existing) = existing {
        match existing.state {
            IssueSubmissionLedgerState::Linked => return IssueSubmissionDecision::AlreadyLinked,
            IssueSubmissionLedgerState::Pending => {}
            IssueSubmissionLedgerState::Created | IssueSubmissionLedgerState::CreatedUnlinked
                if resume =>
            {
                let (Some(child_issue_number), Some(child_issue_id)) =
                    (existing.created_issue_number, existing.created_issue_id)
                else {
                    return IssueSubmissionDecision::Blocked(
                        "ledger entry cannot resume link because created issue identity is missing"
                            .to_string(),
                    );
                };
                let Some(parent_issue) = existing.key.parent_issue else {
                    return IssueSubmissionDecision::AlreadyLinked;
                };
                return IssueSubmissionDecision::Link(IssueLinkRequest {
                    repo: existing.key.repo.clone(),
                    parent_issue,
                    child_issue_number,
                    child_issue_id,
                });
            }
            IssueSubmissionLedgerState::Submitted if resume => {
                let marker = redaction_safe_issue_marker(
                    issue_set_id,
                    &proposal.proposal_id,
                    &proposal.digest,
                );
                return IssueSubmissionDecision::RecoverSubmitted(Box::new(IssueCreateRequest {
                    proposal,
                    body_marker: marker,
                }));
            }
            IssueSubmissionLedgerState::Submitted => {
                return IssueSubmissionDecision::Blocked(
                    "issue submission is already in submitted state; rerun with --resume to search for the marker before retrying"
                        .to_string(),
                );
            }
            IssueSubmissionLedgerState::CreatedMetadataMismatch => {
                return IssueSubmissionDecision::Blocked(
                    "created issue metadata did not match request; human review is required before linking"
                        .to_string(),
                );
            }
            _ if !resume => {
                return IssueSubmissionDecision::Blocked(
                    "ledger entry already exists; use --resume for recoverable created/unlinked issues"
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    let marker = redaction_safe_issue_marker(issue_set_id, &proposal.proposal_id, &proposal.digest);
    IssueSubmissionDecision::Create(Box::new(IssueCreateRequest {
        proposal,
        body_marker: marker,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentactr_core::{IssueDedupeStatus, IssueProposalId, IssueSubmissionLedgerState};

    fn proposal() -> IssueProposal {
        IssueProposal {
            proposal_id: IssueProposalId::new("p1"),
            repo: "owner/repo".to_string(),
            parent_issue: Some(7),
            title: "child".to_string(),
            body: "body".to_string(),
            labels: Vec::new(),
            assignees: Vec::new(),
            milestone: None,
            issue_type: None,
            issue_field_values: Vec::new(),
            project_fields: Vec::new(),
            digest: "abc123".to_string(),
            dedupe: IssueDedupeStatus::Unique,
            framework: None,
            related_issues: Vec::new(),
            provenance: Vec::new(),
        }
    }

    #[test]
    fn creates_when_no_ledger_entry_exists() {
        let decision = plan_issue_submission("run-1", proposal(), None, false);
        assert!(matches!(decision, IssueSubmissionDecision::Create(_)));
    }

    #[test]
    fn resumes_created_unlinked_without_create() {
        let proposal = proposal();
        let entry = IssueSubmissionLedgerEntry {
            key: issue_submission_key("run-1", &proposal),
            state: IssueSubmissionLedgerState::CreatedUnlinked,
            created_issue_number: Some(11),
            created_issue_id: Some(99),
            created_issue_url: Some("https://example.test/11".to_string()),
            detail: String::new(),
        };
        let decision = plan_issue_submission("run-1", proposal, Some(&entry), true);
        assert!(matches!(decision, IssueSubmissionDecision::Link(req) if req.child_issue_id == 99));
    }

    #[test]
    fn submitted_state_uses_recovery_when_resuming() {
        let proposal = proposal();
        let entry = IssueSubmissionLedgerEntry {
            key: issue_submission_key("run-1", &proposal),
            state: IssueSubmissionLedgerState::Submitted,
            created_issue_number: None,
            created_issue_id: None,
            created_issue_url: None,
            detail: String::new(),
        };
        let decision = plan_issue_submission("run-1", proposal, Some(&entry), true);
        assert!(matches!(
            decision,
            IssueSubmissionDecision::RecoverSubmitted(_)
        ));
    }

    #[test]
    fn begin_decision_transitions_pending() {
        let proposal = proposal();
        let entry = IssueSubmissionLedgerEntry {
            key: issue_submission_key("run-1", &proposal),
            state: IssueSubmissionLedgerState::Pending,
            created_issue_number: None,
            created_issue_id: None,
            created_issue_url: None,
            detail: String::new(),
        };
        assert_eq!(
            plan_issue_submission_begin(Some(&entry)),
            IssueSubmissionBeginDecision::TransitionPendingToSubmitted
        );
    }

    #[test]
    fn pending_ledger_entry_allows_first_create() {
        let proposal = proposal();
        let entry = IssueSubmissionLedgerEntry {
            key: issue_submission_key("run-1", &proposal),
            state: IssueSubmissionLedgerState::Pending,
            created_issue_number: None,
            created_issue_id: None,
            created_issue_url: None,
            detail: String::new(),
        };
        let decision = plan_issue_submission("run-1", proposal, Some(&entry), false);

        assert!(matches!(decision, IssueSubmissionDecision::Create(_)));
    }

    #[test]
    fn parent_issue_key_uses_frozen_canonical_values() {
        assert_eq!(parent_issue_key(None), "top_level");
        assert_eq!(parent_issue_key(Some(42)), "parent:42");
    }

    #[test]
    fn prompt_drafting_excludes_raw_prompt_from_body_and_digest_inputs() {
        let prompt = "SECRET=abc123\nBuild an admin dashboard";
        let result = draft_issue_proposals(IssueDraftRequest {
            issue_set_id: "draft-1".to_string(),
            repo: "owner/repo".to_string(),
            parent_issue: None,
            prompt: Some(prompt.to_string()),
            framework: Some(FrameworkDeclaration {
                ecosystem: "typescript".to_string(),
                id: "nextjs".to_string(),
                version_or_profile: None,
            }),
            stack: Some("typescript".to_string()),
            candidates: Vec::new(),
            query: agentactr_core::CandidateQuery::default(),
        })
        .unwrap();

        let proposal = &result.proposals[0];
        assert!(!proposal.body.contains("SECRET=abc123"));
        assert!(!proposal.body.contains("Build an admin dashboard"));
        assert!(proposal.body.contains("Raw prompt persistence is disabled"));
        assert!(proposal
            .provenance
            .iter()
            .any(|entry| entry.starts_with("prompt_sha256:")));
        assert_eq!(proposal.parent_issue, None);
    }

    #[test]
    fn nextjs_crud_prompt_drafts_concrete_implementation_issues() {
        let result = draft_issue_proposals(IssueDraftRequest {
            issue_set_id: "draft-2".to_string(),
            repo: "owner/repo".to_string(),
            parent_issue: None,
            prompt: Some(
                "Create implementation issues for a scalable Next.js TypeScript app with backend route handlers, routing structure, authentication-ready architecture, and full CRUD operations."
                    .to_string(),
            ),
            framework: Some(FrameworkDeclaration {
                ecosystem: "typescript".to_string(),
                id: "nextjs".to_string(),
                version_or_profile: None,
            }),
            stack: Some("typescript".to_string()),
            candidates: Vec::new(),
            query: agentactr_core::CandidateQuery::default(),
        })
        .unwrap();

        let titles = result
            .proposals
            .iter()
            .map(|proposal| proposal.title.as_str())
            .collect::<Vec<_>>();

        assert!(result.proposals.len() >= 5);
        assert!(titles
            .iter()
            .any(|title| title.contains("application architecture")));
        assert!(titles.iter().any(|title| title.contains("route handlers")));
        assert!(titles
            .iter()
            .any(|title| title.contains("CRUD domain model")));
        assert!(titles.iter().any(|title| title.contains("CRUD UI")));
        assert!(titles
            .iter()
            .any(|title| title.contains("authentication-ready")));
        assert!(result
            .proposals
            .iter()
            .all(|proposal| !proposal.title.starts_with("Plan nextjs")));
        assert!(result.proposals.iter().all(|proposal| {
            proposal
                .provenance
                .iter()
                .any(|entry| entry.starts_with("prompt_template:"))
        }));
    }

    #[test]
    fn structured_codex_draft_json_becomes_valid_proposals() {
        let result = draft_issue_proposals_from_structured_json(
            IssueDraftRequest {
                issue_set_id: "draft-3".to_string(),
                repo: "owner/repo".to_string(),
                parent_issue: Some(12),
                prompt: Some("[redacted prompt]".to_string()),
                framework: Some(FrameworkDeclaration {
                    ecosystem: "typescript".to_string(),
                    id: "nextjs".to_string(),
                    version_or_profile: None,
                }),
                stack: Some("typescript".to_string()),
                candidates: vec![Issue {
                    id: "owner/repo#9".to_string(),
                    repo: "owner/repo".to_string(),
                    number: 9,
                    title: "Improve tenant settings validation".to_string(),
                    body: String::new(),
                    state: "open".to_string(),
                    author: "octocat".to_string(),
                    labels: Vec::new(),
                    created_at: None,
                    updated_at: None,
                    is_pull_request: false,
                    html_url: Some("https://example.test/9".to_string()),
                    source_artifact: None,
                }],
                query: agentactr_core::CandidateQuery::default(),
            },
            r#"{
              "proposals": [
                {
                  "title": "Improve tenant settings validation",
                  "body": "Problem\n\nScope\n\nAcceptance criteria",
                  "labels": [],
                  "assignees": [],
                  "provenance": ["repo:apps/web/app/api/tenant-settings/route.ts"]
                }
              ]
            }"#,
            "codex_read_only_structured_issue_draft_planner",
        )
        .unwrap();

        let proposal = &result.proposals[0];
        assert_eq!(proposal.parent_issue, Some(12));
        assert_eq!(proposal.dedupe, IssueDedupeStatus::DuplicateBlocked);
        assert!(proposal.proposal_id.as_str().starts_with("proposal-"));
        assert!(proposal
            .provenance
            .iter()
            .any(|entry| entry == "planner:codex_read_only_structured_issue_draft_planner"));
        assert!(proposal
            .provenance
            .iter()
            .any(|entry| entry.starts_with("prompt_sha256:")));
        assert!(!proposal.body.contains("[redacted prompt]"));
    }

    #[test]
    fn structured_codex_draft_json_rejects_repo_mismatch() {
        let err = draft_issue_proposals_from_structured_json(
            IssueDraftRequest {
                issue_set_id: "draft-4".to_string(),
                repo: "owner/repo".to_string(),
                parent_issue: None,
                prompt: Some("work".to_string()),
                framework: None,
                stack: Some("typescript".to_string()),
                candidates: Vec::new(),
                query: agentactr_core::CandidateQuery::default(),
            },
            r#"{"proposals":[{"repo":"other/repo","title":"Do work","body":"Body"}]}"#,
            "codex_read_only_structured_issue_draft_planner",
        )
        .unwrap_err();

        assert!(err.contains("does not match requested repo"));
    }
}
