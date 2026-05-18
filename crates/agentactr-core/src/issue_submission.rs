use crate::{CandidateQuery, Issue, IssueId};
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct IssueProposalId(pub String);

impl IssueProposalId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueProposal {
    pub proposal_id: IssueProposalId,
    pub repo: String,
    pub parent_issue: Option<u64>,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
    pub issue_type: Option<String>,
    pub issue_field_values: Vec<IssueFieldValue>,
    pub project_fields: Vec<IssueProjectFieldValue>,
    pub digest: String,
    pub dedupe: IssueDedupeStatus,
    pub framework: Option<FrameworkDeclaration>,
    pub related_issues: Vec<IssueId>,
    pub provenance: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueFieldValue {
    pub field_id: i64,
    pub value: String,
    pub value_type: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueProjectFieldValue {
    pub field_name: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IssueMutationCapability {
    pub issue_create: bool,
    pub issue_link: bool,
    pub issue_labels: bool,
    pub issue_assignees: bool,
    pub issue_milestone: bool,
    pub issue_type: bool,
    pub issue_field_values: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueCreateRequest {
    pub proposal: IssueProposal,
    pub body_marker: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueCreateResult {
    pub issue: Issue,
    pub tracker_issue_id: Option<u64>,
    pub requested_metadata: IssueRequestedMetadata,
    pub applied_metadata: IssueAppliedMetadata,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IssueRequestedMetadata {
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
    pub issue_type: Option<String>,
    pub issue_field_values: Vec<IssueFieldValue>,
    pub project_fields: Vec<IssueProjectFieldValue>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IssueAppliedMetadata {
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
    pub issue_type: Option<String>,
    pub issue_field_values: Vec<IssueFieldValue>,
    pub project_fields: Vec<IssueProjectFieldValue>,
}

impl IssueCreateResult {
    pub fn metadata_mismatches(&self) -> Vec<String> {
        let mut mismatches = Vec::new();
        push_vec_mismatch(
            &mut mismatches,
            "labels",
            &self.requested_metadata.labels,
            &self.applied_metadata.labels,
        );
        push_vec_mismatch(
            &mut mismatches,
            "assignees",
            &self.requested_metadata.assignees,
            &self.applied_metadata.assignees,
        );
        if self.requested_metadata.milestone != self.applied_metadata.milestone {
            mismatches.push("milestone".to_string());
        }
        if self.requested_metadata.issue_type != self.applied_metadata.issue_type {
            mismatches.push("issue_type".to_string());
        }
        if self.requested_metadata.issue_field_values != self.applied_metadata.issue_field_values {
            mismatches.push("issue_field_values".to_string());
        }
        if self.requested_metadata.project_fields != self.applied_metadata.project_fields {
            mismatches.push("project_fields".to_string());
        }
        mismatches
    }
}

fn push_vec_mismatch(
    mismatches: &mut Vec<String>,
    name: &str,
    requested: &[String],
    applied: &[String],
) {
    let mut requested = requested.to_vec();
    let mut applied = applied.to_vec();
    requested.sort();
    applied.sort();
    if requested != applied {
        mismatches.push(name.to_string());
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueLinkRequest {
    pub repo: String,
    pub parent_issue: u64,
    pub child_issue_number: u64,
    pub child_issue_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueLinkResult {
    pub parent_issue: u64,
    pub child_issue_number: u64,
    pub linked: bool,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IssueSubmissionLedgerState {
    Pending,
    Submitted,
    Created,
    Linked,
    CreatedUnlinked,
    CreatedMetadataMismatch,
    Failed,
}

impl IssueSubmissionLedgerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Submitted => "submitted",
            Self::Created => "created",
            Self::Linked => "linked",
            Self::CreatedUnlinked => "created_unlinked",
            Self::CreatedMetadataMismatch => "created_metadata_mismatch",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueSubmissionLedgerKey {
    pub run_id: String,
    pub issue_set_id: String,
    pub proposal_id: IssueProposalId,
    pub repo: String,
    pub parent_issue: Option<u64>,
    pub parent_issue_key: String,
    pub proposal_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueSubmissionLedgerEntry {
    pub key: IssueSubmissionLedgerKey,
    pub state: IssueSubmissionLedgerState,
    pub created_issue_number: Option<u64>,
    pub created_issue_id: Option<u64>,
    pub created_issue_url: Option<String>,
    pub detail: String,
}

pub fn redaction_safe_issue_marker(
    issue_set_id: &str,
    proposal_id: &IssueProposalId,
    digest: &str,
) -> String {
    format!(
        "<!-- agentactr:issue-proposal issue_set_id={} proposal_id={} digest={} -->",
        marker_token(issue_set_id),
        marker_token(proposal_id.as_str()),
        marker_token(digest)
    )
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IssueDedupeStatus {
    #[default]
    Unique,
    PossibleDuplicate,
    DuplicateBlocked,
    Deferred,
}

impl IssueDedupeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unique => "unique",
            Self::PossibleDuplicate => "possible_duplicate",
            Self::DuplicateBlocked => "duplicate_blocked",
            Self::Deferred => "deferred",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IssueDraftMode {
    #[default]
    TrackerBacked,
    LocalOnly,
}

impl IssueDraftMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TrackerBacked => "tracker_backed",
            Self::LocalOnly => "local_only",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "local_only" => Self::LocalOnly,
            _ => Self::TrackerBacked,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameworkDeclaration {
    pub ecosystem: String,
    pub id: String,
    pub version_or_profile: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IssueSetArtifactContext {
    pub schema_version: u32,
    pub artifact_format_version: u32,
    pub issue_set_id: String,
    pub compat_run_id: Option<String>,
    pub created_at: String,
    pub producer: String,
    pub source: IssueSetSource,
    pub draft_mode: IssueDraftMode,
    pub repo: String,
    pub parent_issue: Option<u64>,
    pub framework: Option<FrameworkDeclaration>,
    pub tracker_network_required: bool,
    pub planner_network_required: bool,
    pub submit_requires_repo: bool,
    pub submit_target_repo: Option<String>,
    pub dedupe_deferred: bool,
    pub artifact_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub candidates_path: PathBuf,
    pub proposals_path: PathBuf,
    pub dedupe_report_path: PathBuf,
    pub planner_prompt_path: Option<PathBuf>,
    pub planner_metadata_path: Option<PathBuf>,
    pub trace_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IssueSetSource {
    Find,
    Draft,
    #[default]
    RunLegacy,
}

impl IssueSetSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Find => "find",
            Self::Draft => "draft",
            Self::RunLegacy => "run_legacy",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueDraftRequest {
    pub issue_set_id: String,
    pub repo: String,
    pub parent_issue: Option<u64>,
    pub prompt: Option<String>,
    pub framework: Option<FrameworkDeclaration>,
    pub domain: Option<String>,
    pub stack: Option<String>,
    pub candidates: Vec<Issue>,
    pub query: CandidateQuery,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueDraftResult {
    pub proposals: Vec<IssueProposal>,
    pub discarded_partial_output: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IssueTemplateProfile {
    pub template_id: String,
    pub template_family: String,
    pub template_variant: String,
    pub template_version: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IssueTemplateContext {
    pub stack: Option<String>,
    pub domain: Option<String>,
    pub framework: Option<FrameworkDeclaration>,
    pub prompt_digest: Option<String>,
    pub repo: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IssueTemplateRenderResult {
    pub profile: IssueTemplateProfile,
    pub title: String,
    pub body: String,
    pub provenance: Vec<String>,
}

fn marker_token(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':' | '.'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_strips_unsafe_content() {
        let marker = redaction_safe_issue_marker(
            "run/one",
            &IssueProposalId::new("proposal one"),
            "abc123 secret=value",
        );
        assert!(marker.contains("issue_set_id=runone"));
        assert!(marker.contains("proposal_id=proposalone"));
        assert!(marker.contains("digest=abc123secretvalue"));
        assert!(!marker.contains("secret="));
        assert!(!marker.contains("proposal one"));
    }

    #[test]
    fn detects_metadata_mismatch() {
        let result = IssueCreateResult {
            issue: Issue::default(),
            tracker_issue_id: Some(1),
            requested_metadata: IssueRequestedMetadata {
                labels: vec!["bug".to_string()],
                assignees: vec!["octocat".to_string()],
                milestone: Some("v1".to_string()),
                issue_type: Some("task".to_string()),
                issue_field_values: Vec::new(),
                project_fields: vec![IssueProjectFieldValue {
                    field_name: "Priority".to_string(),
                    value: "P1".to_string(),
                }],
            },
            applied_metadata: IssueAppliedMetadata {
                labels: Vec::new(),
                assignees: vec!["octocat".to_string()],
                milestone: Some("v1".to_string()),
                issue_type: None,
                issue_field_values: Vec::new(),
                project_fields: Vec::new(),
            },
        };
        assert_eq!(
            result.metadata_mismatches(),
            vec![
                "labels".to_string(),
                "issue_type".to_string(),
                "project_fields".to_string()
            ]
        );
    }
}
