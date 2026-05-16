use crate::{
    IssueCreateRequest, IssueCreateResult, IssueDraftRequest, IssueDraftResult, IssueLinkRequest,
    IssueLinkResult, MemoryActionResult, RuntimeProcessEvent,
};
use std::path::{Path, PathBuf};

pub trait IssueTracker: Send + Sync {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport::unknown("issue_tracker")
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::unknown("issue_tracker")
    }
    fn fetch_candidates(&self, _q: CandidateQuery) -> Result<Vec<Issue>, String>;
    fn fetch_by_ids(&self, _ids: &[IssueId]) -> Result<Vec<Issue>, String>;
    fn claim(&self, _req: ClaimRequest) -> Result<ClaimResult, String>;
    fn release(&self, _req: ReleaseRequest) -> Result<ReleaseResult, String>;
    fn comment(&self, _req: CommentRequest) -> Result<CommentRef, String>;
    fn create_issue(&self, _req: IssueCreateRequest) -> Result<IssueCreateResult, String> {
        Err("issue creation is not implemented by this issue tracker adapter".to_string())
    }
    fn link_issue(&self, _req: IssueLinkRequest) -> Result<IssueLinkResult, String> {
        Err("issue linking is not implemented by this issue tracker adapter".to_string())
    }
}

pub trait IssueDraftPlanner: Send + Sync {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport::unknown("issue_draft_planner")
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::unknown("issue_draft_planner")
    }
    fn draft(&self, _req: IssueDraftRequest) -> Result<IssueDraftResult, String> {
        Err("issue drafting is not implemented by this planner adapter".to_string())
    }
}

pub trait AgentRuntime: Send + Sync {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport::unknown("agent_runtime")
    }
    fn capabilities(&self) -> AgentRuntimeCapabilities;
    fn start(&self, _req: AgentStartRequest) -> Result<AgentSession, String>;
    fn run_turn(&self, _req: AgentTurnRequest) -> Result<AgentTurnStream, String>;
    fn run_issue(&self, _req: AgentIssueRunRequest) -> Result<AgentRunReport, String> {
        Err("run_issue is not implemented by this runtime adapter".to_string())
    }
    fn cancel(&self, _session_id: &str, _reason: CancelReason) -> Result<(), String>;
}

pub trait MemoryController: Send + Sync {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport::unknown("memory_controller")
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::unknown("memory_controller")
    }
    fn create_run_group(&self, _req: MemoryGroupRequest) -> Result<MemoryGroup, String>;
    fn attach_pid(&self, _group: &MemoryGroup, _pid: u32) -> Result<(), String>;
    fn sample(&self, _group: &MemoryGroup) -> Result<MemorySample, String>;
    fn reclaim(&self, _group: &MemoryGroup, _bytes: u64) -> Result<MemoryActionResult, String> {
        Err("memory reclaim is not implemented by this memory controller".to_string())
    }
    fn kill_group(
        &self,
        _group: &MemoryGroup,
        _terminal_cleanup: bool,
    ) -> Result<MemoryActionResult, String> {
        Err("cgroup kill is not implemented by this memory controller".to_string())
    }
    fn finalize_group(&self, _group: &MemoryGroup) -> Result<MemoryActionResult, String> {
        Err("memory group finalization is not implemented by this memory controller".to_string())
    }
    fn enforce(
        &self,
        _group: &MemoryGroup,
        _policy: MemoryPolicy,
    ) -> Result<MemoryDecision, String> {
        Err("MemoryController::enforce is deprecated; policy belongs to RunResourceGovernor and adapters expose primitive actions".to_string())
    }
}

pub trait RuntimeProcessSupervisor: Send + Sync {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport::unknown("runtime_process_supervisor")
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::unknown("runtime_process_supervisor")
    }
    fn observe(&self, _event: &RuntimeProcessEvent) -> Result<(), String> {
        Ok(())
    }
    fn start(
        &self,
        _event: &RuntimeProcessEvent,
        _artifact_dir: &Path,
    ) -> Result<Option<Box<dyn RuntimeProcessMonitor>>, String>;
    fn preserve_debug_bundle(
        &self,
        _event: Option<&RuntimeProcessEvent>,
        _artifact_dir: &Path,
        _reason: &str,
    ) -> Result<(), String>;
    fn cancel_process_tree(
        &self,
        _event: &RuntimeProcessEvent,
        _reason: &str,
    ) -> Result<String, String> {
        Err("runtime process supervisor does not support process-tree cancellation".to_string())
    }
}

pub trait RuntimeProcessMonitor: Send {
    fn failure(&self) -> Option<String>;
    fn stop(self: Box<Self>) -> Result<(), String>;
}

pub trait TraceSink: Send + Sync {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport::unknown("trace_sink")
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::unknown("trace_sink")
    }
    fn emit(&self, _event: TraceEvent) -> Result<(), String>;
}

pub trait HumanIntervention: Send + Sync {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport::unknown("human_intervention")
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::unknown("human_intervention")
    }
    fn mode(&self) -> HumanInterventionMode;
    fn resolve(&self, _req: InterventionRequest) -> Result<InterventionDecision, String>;
}

pub trait VersionControl: Send + Sync {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport::unknown("version_control")
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::unknown("version_control")
    }
    fn detect(&self, _root: &Path) -> Result<VcsCapabilities, String>;
    fn prepare_workspace(&self, _req: WorktreeRequest) -> Result<WorktreeRef, String>;
    fn diff(&self, _worktree: &WorktreeRef) -> Result<WorkspaceDiff, String>;
    fn commit(&self, _req: CommitRequest) -> Result<CommitRef, String>;
    fn merge_plan(&self, _req: MergePlanRequest) -> Result<MergePlan, String>;
}

pub trait PreCommitRunner: Send + Sync {
    fn version_report(&self) -> AdapterVersionReport {
        AdapterVersionReport::unknown("pre_commit_runner")
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::unknown("pre_commit_runner")
    }
    fn detect_stack(&self, _worktree: &WorktreeRef) -> Result<TechnologyStack, String>;
    fn plan(&self, _req: PreCommitPlanRequest) -> Result<PreCommitPlan, String>;
    fn run(&self, _plan: PreCommitPlan) -> Result<PreCommitReport, String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterCapabilities {
    pub adapter_kind: String,
    pub supported_features: Vec<String>,
    pub degraded_features: Vec<String>,
    pub required_actions: Vec<String>,
}

impl AdapterCapabilities {
    pub fn unknown(adapter_kind: impl Into<String>) -> Self {
        Self {
            adapter_kind: adapter_kind.into(),
            supported_features: Vec::new(),
            degraded_features: vec!["capability report unavailable".to_string()],
            required_actions: vec!["implement adapter capabilities()".to_string()],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterVersionReport {
    pub adapter_kind: String,
    pub adapter_name: String,
    pub adapter_version: String,
    pub product_name: String,
    pub product_version: String,
    pub api_version: String,
    pub capability_digest: String,
    pub degraded_features: Vec<String>,
    pub required_actions: Vec<String>,
    pub warnings: Vec<String>,
}

impl AdapterVersionReport {
    pub fn unknown(adapter_kind: impl Into<String>) -> Self {
        Self {
            adapter_kind: adapter_kind.into(),
            adapter_name: "unknown".to_string(),
            adapter_version: "unknown".to_string(),
            product_name: "unknown".to_string(),
            product_version: "unknown".to_string(),
            api_version: "unknown".to_string(),
            capability_digest: "unknown".to_string(),
            degraded_features: vec!["version report unavailable".to_string()],
            required_actions: vec!["implement adapter version_report()".to_string()],
            warnings: vec!["adapter did not provide a concrete version report".to_string()],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CandidateState {
    #[default]
    Open,
    Closed,
    All,
}

impl CandidateState {
    pub fn as_github_value(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::All => "all",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CandidateSort {
    Created,
    #[default]
    Updated,
    Comments,
}

impl CandidateSort {
    pub fn as_github_value(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Comments => "comments",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortDirection {
    Asc,
    #[default]
    Desc,
}

impl SortDirection {
    pub fn as_github_value(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateQuery {
    pub repo: String,
    pub state: CandidateState,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub author: Option<String>,
    pub since: Option<String>,
    pub text_query: Option<String>,
    pub include_pull_requests: bool,
    pub sort: CandidateSort,
    pub direction: SortDirection,
    pub page: Option<u32>,
    pub per_page: u32,
    pub limit: u32,
}

impl Default for CandidateQuery {
    fn default() -> Self {
        Self {
            repo: String::new(),
            state: CandidateState::Open,
            labels: Vec::new(),
            assignee: None,
            author: None,
            since: None,
            text_query: None,
            include_pull_requests: false,
            sort: CandidateSort::Updated,
            direction: SortDirection::Desc,
            page: None,
            per_page: 50,
            limit: 50,
        }
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IssueId(pub String);
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Issue {
    pub id: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub author: String,
    pub labels: Vec<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub is_pull_request: bool,
    pub html_url: Option<String>,
    pub source_artifact: Option<PathBuf>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimRequest {
    pub repo: String,
    pub issue_number: u64,
    pub run_id: String,
    pub owner_id: String,
    pub fencing_token: String,
    pub lease_expires_at: String,
    pub claim_label: String,
    pub running_label: String,
    pub ignore_labels: Vec<String>,
    pub allow_pull_request: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClaimResult {
    pub accepted: bool,
    pub fencing_token: String,
    pub previous_lease: Option<String>,
    pub applied_labels: Vec<String>,
    pub marker_comment: Option<CommentRef>,
    pub source_artifacts: Vec<PathBuf>,
    pub verification_status: String,
    pub detail: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IssueCommentKind {
    Claim,
    Progress,
    ReviewRequired,
    FinalSummary,
    FailureSummary,
    RejectionSummary,
}
impl IssueCommentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claim => "claim",
            Self::Progress => "progress",
            Self::ReviewRequired => "review_required",
            Self::FinalSummary => "final_summary",
            Self::FailureSummary => "failure_summary",
            Self::RejectionSummary => "rejection_summary",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentRequest {
    pub repo: String,
    pub issue_number: u64,
    pub run_id: String,
    pub kind: IssueCommentKind,
    pub idempotency_key: String,
    pub body: String,
    pub update_existing: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommentRef {
    pub provider_id: String,
    pub html_url: Option<String>,
    pub artifact_path: Option<PathBuf>,
    pub created_or_updated: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IssueReleaseOutcome {
    Failure,
    ReviewRequired,
    Finalized,
    ReviewRejected,
}
impl IssueReleaseOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Failure => "failure",
            Self::ReviewRequired => "review_required",
            Self::Finalized => "finalized",
            Self::ReviewRejected => "review_rejected",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseRequest {
    pub repo: String,
    pub issue_number: u64,
    pub run_id: String,
    pub outcome: IssueReleaseOutcome,
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub close_state_reason: Option<String>,
    pub final_comment: Option<CommentRequest>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReleaseResult {
    pub applied_labels: Vec<String>,
    pub removed_labels: Vec<String>,
    pub final_issue_state: String,
    pub state_reason: Option<String>,
    pub comment_refs: Vec<CommentRef>,
    pub source_artifacts: Vec<PathBuf>,
    pub verification_status: String,
    pub mismatch_details: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualityGateSummary {
    pub success: bool,
    pub report_path: Option<PathBuf>,
    pub failed_reason: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunOutcomeSummary {
    pub run_id: String,
    pub repo: String,
    pub issue_number: u64,
    pub runtime_success: bool,
    pub quality: QualityGateSummary,
    pub artifact_dir: PathBuf,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentRuntimeCapabilities {
    pub single_shot_issue_run: bool,
    pub session_start: bool,
    pub turn_streaming: bool,
    pub cancellation: bool,
    pub exec_json: bool,
    pub app_server: bool,
    pub codex_sdk: bool,
    pub child_agent_execution: bool,
    pub parallel_read_only_child_agents: bool,
}
#[derive(Clone, Debug, Default)]
pub struct AgentStartRequest;
#[derive(Clone, Debug, Default)]
pub struct AgentSession;
#[derive(Clone, Debug, Default)]
pub struct AgentTurnRequest;
#[derive(Clone, Debug, Default)]
pub struct AgentTurnStream;
#[derive(Clone, Debug, Default)]
pub struct AgentIssueRunRequest {
    pub run_id: String,
    pub agent_run_id: String,
    pub parent_agent_run_id: Option<String>,
    pub role: String,
    pub objective: String,
    pub write_scope: String,
    pub worktree: PathBuf,
    pub artifact_dir: PathBuf,
    pub trace_path: PathBuf,
    pub context_manifest: PathBuf,
    pub memory: Option<MemoryLease>,
    pub child_memory: Vec<AgentMemoryLease>,
    pub repo: String,
    pub issue: String,
    pub issue_context: Issue,
    pub approval_policy: RuntimeApprovalPolicy,
    pub spawn_plan: Option<crate::SpawnPlan>,
}
impl AgentIssueRunRequest {
    pub fn child_memory_lease(&self, agent_run_id: &str) -> Option<MemoryLease> {
        self.child_memory
            .iter()
            .find(|lease| lease.agent_run_id == agent_run_id)
            .map(|lease| lease.lease.clone())
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentMemoryLease {
    pub agent_run_id: String,
    pub lease: MemoryLease,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RuntimeApprovalPolicy {
    #[default]
    Never,
    OnRequest,
}
#[derive(Clone, Debug, Default)]
pub struct AgentRunReport {
    pub stdout_jsonl: PathBuf,
    pub stderr_log: PathBuf,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MemoryGroupId(pub String);
impl MemoryGroupId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MemoryPolicyRef(pub String);
impl MemoryPolicyRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryLease {
    pub group_id: MemoryGroupId,
    pub policy: MemoryPolicyRef,
}
#[derive(Clone, Debug)]
pub enum CancelReason {
    User,
    Timeout,
    MemoryPressure,
    Policy,
}
#[derive(Clone, Debug, Default)]
pub struct MemoryGroupRequest {
    pub group_id: Option<MemoryGroupId>,
    pub path: Option<PathBuf>,
}
#[derive(Clone, Debug, Default)]
pub struct MemoryGroup {
    pub group_id: Option<MemoryGroupId>,
    pub path: Option<PathBuf>,
}
#[derive(Clone, Debug, Default)]
pub struct MemorySample {
    pub payload_json: String,
}
#[derive(Clone, Debug, Default)]
pub struct MemoryPolicy;
#[derive(Clone, Debug, Default)]
pub struct MemoryDecision;
#[derive(Clone, Debug, Default)]
pub struct TraceEvent;
#[derive(Clone, Debug)]
pub enum HumanInterventionMode {
    FailClosed,
    Interactive,
    ReviewRequired,
    AutoPolicy,
}
#[derive(Clone, Debug, Default)]
pub struct InterventionRequest;
#[derive(Clone, Debug, Default)]
pub struct InterventionDecision;
#[derive(Clone, Debug, Default)]
pub struct VcsCapabilities;
#[derive(Clone, Debug, Default)]
pub struct WorktreeRequest {
    pub run_id: String,
    pub repo: String,
    pub issue: String,
    pub base_ref: String,
    pub worktree_root: PathBuf,
    pub branch_template: String,
    pub fail_on_dirty_source_checkout: bool,
    pub copy_runtime_config_to_worktree: bool,
}
#[derive(Clone, Debug, Default)]
pub struct WorktreeRef {
    pub path: PathBuf,
    pub base_commit: String,
    pub run_id: String,
}
#[derive(Clone, Debug, Default)]
pub struct WorkspaceDiff {
    pub run_id: String,
    pub worktree: PathBuf,
    pub base_commit: String,
    pub current_commit: String,
    pub patch: String,
    pub touched_files: Vec<String>,
    pub untracked_files: Vec<String>,
    pub is_empty: bool,
}
#[derive(Clone, Debug, Default)]
pub struct CommitRequest;
#[derive(Clone, Debug, Default)]
pub struct CommitRef;
#[derive(Clone, Debug, Default)]
pub struct MergePlanRequest {
    pub worktree: WorktreeRef,
    pub base_ref: String,
    pub merge_mode: String,
    pub workspace_diff_artifact: Option<PathBuf>,
}
#[derive(Clone, Debug, Default)]
pub struct MergePlan {
    pub run_id: String,
    pub worktree: PathBuf,
    pub base_ref: String,
    pub base_commit: String,
    pub current_commit: String,
    pub base_ref_current_commit: String,
    pub base_ref_drifted: bool,
    pub head_contains_base_ref: bool,
    pub merge_mode: String,
    pub merge_enabled: bool,
    pub workspace_diff_artifact: Option<PathBuf>,
    pub workspace_diff_exists: bool,
    pub touched_files: Vec<String>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    pub recommendation: String,
}
#[derive(Clone, Debug, Default)]
pub struct TechnologyStack;
#[derive(Clone, Debug, Default)]
pub struct PreCommitPlanRequest;
#[derive(Clone, Debug, Default)]
pub struct PreCommitPlan;
#[derive(Clone, Debug, Default)]
pub struct PreCommitReport;
