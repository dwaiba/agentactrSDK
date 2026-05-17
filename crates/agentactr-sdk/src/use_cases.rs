use agentactr_core::{
    AgentIssueRunRequest, AgentRole, AgentRunId, AgentRunReport, AgentRuntime, AgentactrConfig,
    ClaimRequest, ClaimResult, CommentRequest, Issue, IssueCommentKind, IssueId,
    IssueReleaseOutcome, IssueTracker, PreCommitRunner, QualityGateSummary, ReadScope,
    ReleaseRequest, ReleaseResult, RunBudgetSnapshot, RunId, RunOutcomeSummary, SpawnManager,
    SpawnPlan, SpawnPlanRequest, SpawnPolicy, SpawnRequest, VersionControl, WorktreeRef,
    WorktreeRequest,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
pub struct AgentActrBuilder {
    issue_tracker: Option<Arc<dyn IssueTracker>>,
    runtime: Option<Arc<dyn AgentRuntime>>,
    vcs: Option<Arc<dyn VersionControl>>,
    quality: Option<Arc<dyn PreCommitRunner>>,
}

impl AgentActrBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn issue_tracker(mut self, issue_tracker: Arc<dyn IssueTracker>) -> Self {
        self.issue_tracker = Some(issue_tracker);
        self
    }

    pub fn runtime(mut self, runtime: Arc<dyn AgentRuntime>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    pub fn version_control(mut self, vcs: Arc<dyn VersionControl>) -> Self {
        self.vcs = Some(vcs);
        self
    }

    pub fn quality(mut self, quality: Arc<dyn PreCommitRunner>) -> Self {
        self.quality = Some(quality);
        self
    }

    pub fn build(self) -> Result<AgentActr, String> {
        Ok(AgentActr {
            issue_tracker: self
                .issue_tracker
                .ok_or("AgentActrBuilder missing issue_tracker")?,
            runtime: self.runtime.ok_or("AgentActrBuilder missing runtime")?,
            vcs: self.vcs.ok_or("AgentActrBuilder missing version_control")?,
            quality: self.quality,
        })
    }
}

pub struct AgentActr {
    issue_tracker: Arc<dyn IssueTracker>,
    runtime: Arc<dyn AgentRuntime>,
    vcs: Arc<dyn VersionControl>,
    quality: Option<Arc<dyn PreCommitRunner>>,
}

impl AgentActr {
    pub fn quality_runner(&self) -> Option<&dyn PreCommitRunner> {
        self.quality.as_deref()
    }
}

pub trait AgentActrUseCases {
    fn prepare_workspace(&self, req: WorktreeRequest) -> Result<WorktreeRef, String>;
    fn plan_default_spawn(&self, req: DefaultSpawnPlanRequest<'_>) -> SpawnPlan;
    fn claim_issue(&self, req: ClaimRequest) -> Result<ClaimResult, String>;
    fn apply_issue_lifecycle(
        &self,
        req: IssueLifecycleRequest,
    ) -> Result<IssueLifecycleReport, String>;
    fn finalize_run(&self, req: FinalizeRunRequest) -> Result<IssueLifecycleReport, String>;
    fn run_issue(
        &self,
        req: RunIssueRequest,
        hooks: &mut dyn RunIssueHooks,
    ) -> Result<RunIssueReport, String>;
}

impl AgentActrUseCases for AgentActr {
    fn prepare_workspace(&self, req: WorktreeRequest) -> Result<WorktreeRef, String> {
        Ok(self.vcs.prepare_workspace(req)?)
    }

    fn plan_default_spawn(&self, req: DefaultSpawnPlanRequest<'_>) -> SpawnPlan {
        default_spawn_plan(req)
    }

    fn claim_issue(&self, req: ClaimRequest) -> Result<ClaimResult, String> {
        Ok(self.issue_tracker.claim(req)?)
    }

    fn apply_issue_lifecycle(
        &self,
        req: IssueLifecycleRequest,
    ) -> Result<IssueLifecycleReport, String> {
        apply_issue_lifecycle_with_tracker(&*self.issue_tracker, req)
    }

    fn finalize_run(&self, req: FinalizeRunRequest) -> Result<IssueLifecycleReport, String> {
        finalize_run_with_tracker(&*self.issue_tracker, req)
    }

    fn run_issue(
        &self,
        req: RunIssueRequest,
        hooks: &mut dyn RunIssueHooks,
    ) -> Result<RunIssueReport, String> {
        let worktree = run_phase(hooks, "worktree", || {
            Ok(self.vcs.prepare_workspace(req.worktree.clone())?)
        })?;
        let issue_context = run_phase(hooks, "github_fetch", || {
            let issues = self
                .issue_tracker
                .fetch_by_ids(std::slice::from_ref(&req.issue_id))?;
            issues
                .into_iter()
                .next()
                .ok_or("issue tracker returned no issue".to_string())
        })?;
        let runtime_req = run_hook_phase(hooks, "codex_preflight", |hooks| {
            hooks.before_runtime(RunIssueRuntimeContext {
                request: &req,
                worktree: &worktree,
                issue_context: &issue_context,
            })
        })?;
        let runtime = run_hook_phase(hooks, "codex_exec", |hooks| {
            let report = self.runtime.run_issue(runtime_req)?;
            hooks.after_runtime_success(RunIssuePostRuntimeContext {
                request: &req,
                worktree: &worktree,
                issue_context: &issue_context,
                runtime_report: &report,
            })?;
            Ok(report)
        })?;
        Ok(RunIssueReport {
            worktree,
            issue_context,
            runtime,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleLabels {
    pub claim_label: String,
    pub running_label: String,
    pub failed_label: String,
    pub done_label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IssueLifecycleMode {
    Disabled,
    RequireHumanReview,
    AutomaticAfterQualityGates,
    Failure { reason: String },
    Reject { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueLifecycleRequest {
    pub mode: IssueLifecycleMode,
    pub outcome: RunOutcomeSummary,
    pub labels: LifecycleLabels,
    pub summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FinalizeDecision {
    Approve,
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalizeRunRequest {
    pub decision: FinalizeDecision,
    pub reject_reason: Option<String>,
    pub outcome: RunOutcomeSummary,
    pub labels: LifecycleLabels,
    pub summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueLifecycleReport {
    pub status: String,
    pub release: Option<ReleaseResult>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedRunFinalizationRequest {
    pub run_id: String,
    pub repo: String,
    pub issue_number: u64,
    pub decision: FinalizeDecision,
    pub reject_reason: Option<String>,
    pub labels: LifecycleLabels,
    pub resume: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedRunFinalizationReport {
    pub lifecycle: IssueLifecycleReport,
    pub prior_status: Option<String>,
    pub outcome: RunOutcomeSummary,
    pub summary: String,
}

pub trait RunFinalizationArtifactSource {
    fn artifact_dir(&self) -> &Path;
    fn finalization_status(&self) -> Result<Option<String>, String>;
    fn quality_summary(&self, decision: FinalizeDecision) -> Result<QualityGateSummary, String>;

    fn run_summary(&self, run_id: &str) -> String {
        recorded_run_lifecycle_summary(run_id, self.artifact_dir())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FsRunFinalizationArtifacts {
    artifact_dir: PathBuf,
}

impl FsRunFinalizationArtifacts {
    pub fn new(artifact_dir: impl Into<PathBuf>) -> Self {
        Self {
            artifact_dir: artifact_dir.into(),
        }
    }

    pub fn quality_report_path(&self) -> PathBuf {
        self.artifact_dir.join("quality_report.txt")
    }

    pub fn quality_status_path(&self) -> PathBuf {
        quality_status_path(&self.quality_report_path())
    }
}

impl RunFinalizationArtifactSource for FsRunFinalizationArtifacts {
    fn artifact_dir(&self) -> &Path {
        &self.artifact_dir
    }

    fn finalization_status(&self) -> Result<Option<String>, String> {
        read_recorded_finalization_status(&self.artifact_dir)
    }

    fn quality_summary(&self, decision: FinalizeDecision) -> Result<QualityGateSummary, String> {
        load_recorded_quality_summary(&self.quality_report_path(), decision)
    }
}

pub fn finalize_recorded_run_with_tracker(
    tracker: &dyn IssueTracker,
    artifacts: &dyn RunFinalizationArtifactSource,
    req: RecordedRunFinalizationRequest,
) -> Result<RecordedRunFinalizationReport, String> {
    let prior_status = artifacts.finalization_status()?;
    validate_recorded_finalization_status(
        req.decision.clone(),
        req.resume,
        prior_status.as_deref(),
    )?;
    let quality = artifacts.quality_summary(req.decision.clone())?;
    let outcome = RunOutcomeSummary {
        run_id: req.run_id.clone(),
        repo: req.repo,
        issue_number: req.issue_number,
        runtime_success: true,
        quality,
        artifact_dir: artifacts.artifact_dir().to_path_buf(),
    };
    let summary = artifacts.run_summary(&req.run_id);
    let lifecycle = finalize_run_with_tracker(
        tracker,
        FinalizeRunRequest {
            decision: req.decision,
            reject_reason: req.reject_reason,
            outcome: outcome.clone(),
            labels: req.labels,
            summary: summary.clone(),
        },
    )?;
    Ok(RecordedRunFinalizationReport {
        lifecycle,
        prior_status,
        outcome,
        summary,
    })
}

fn validate_recorded_finalization_status(
    decision: FinalizeDecision,
    resume: bool,
    prior_status: Option<&str>,
) -> Result<(), String> {
    if !resume && matches!(prior_status, Some("finalized" | "review_rejected")) {
        return Err(format!(
            "run is already {}; pass --resume to re-verify idempotent finalization",
            prior_status.unwrap_or_default()
        ));
    }
    match decision {
        FinalizeDecision::Approve => match prior_status {
            Some("review_required") => Ok(()),
            Some("finalized") if resume => Ok(()),
            Some(other) => Err(format!(
                "finalize --approve requires prior review_required status; found {other}"
            )),
            None => {
                Err("finalize --approve requires recorded finalization_status.json".to_string())
            }
        },
        FinalizeDecision::Reject => match prior_status {
            Some("review_required" | "failed") => Ok(()),
            Some("review_rejected") if resume => Ok(()),
            Some(other) => Err(format!(
                "finalize --reject requires prior review_required or failed status; found {other}"
            )),
            None => Err("finalize --reject requires recorded finalization_status.json".to_string()),
        },
    }
}

pub fn finalize_run_with_tracker(
    tracker: &dyn IssueTracker,
    req: FinalizeRunRequest,
) -> Result<IssueLifecycleReport, String> {
    let mode = match req.decision {
        FinalizeDecision::Approve => IssueLifecycleMode::AutomaticAfterQualityGates,
        FinalizeDecision::Reject => IssueLifecycleMode::Reject {
            reason: req
                .reject_reason
                .clone()
                .ok_or("finalize reject requires reason")?,
        },
    };
    apply_issue_lifecycle_with_tracker(
        tracker,
        IssueLifecycleRequest {
            mode,
            outcome: req.outcome,
            labels: req.labels,
            summary: req.summary,
        },
    )
}

pub fn quality_gate_success(report_path: impl Into<std::path::PathBuf>) -> QualityGateSummary {
    QualityGateSummary {
        success: true,
        report_path: Some(report_path.into()),
        failed_reason: None,
    }
}

pub fn quality_gate_failure(
    report_path: impl Into<std::path::PathBuf>,
    reason: impl Into<String>,
) -> QualityGateSummary {
    QualityGateSummary {
        success: false,
        report_path: Some(report_path.into()),
        failed_reason: Some(reason.into()),
    }
}

pub fn quality_status_path(report_path: &Path) -> PathBuf {
    report_path.with_extension("status.json")
}

pub fn load_recorded_quality_summary(
    report_path: &Path,
    decision: FinalizeDecision,
) -> Result<QualityGateSummary, String> {
    let status_path = quality_status_path(report_path);
    if !status_path.exists() {
        if decision == FinalizeDecision::Approve {
            return Err(format!(
                "finalize --approve requires successful quality status at {}",
                status_path.display()
            ));
        }
        return Ok(QualityGateSummary {
            success: false,
            report_path: if report_path.exists() {
                Some(report_path.to_path_buf())
            } else {
                None
            },
            failed_reason: Some("quality status missing".to_string()),
        });
    }
    let text = std::fs::read_to_string(&status_path)
        .map_err(|e| format!("read quality status {}: {e}", status_path.display()))?;
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| format!("parse quality status {}: {e}", status_path.display()))?;
    let success = value
        .get("success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let failed_reason = value
        .get("failed_reason")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    if decision == FinalizeDecision::Approve && !success {
        return Err(format!(
            "finalize --approve requires successful quality status at {}; status is failed: {}",
            status_path.display(),
            failed_reason
                .clone()
                .unwrap_or_else(|| "unspecified quality failure".to_string())
        ));
    }
    Ok(QualityGateSummary {
        success,
        report_path: Some(report_path.to_path_buf()),
        failed_reason,
    })
}

pub fn read_recorded_finalization_status(artifact_dir: &Path) -> Result<Option<String>, String> {
    let path = artifact_dir.join("finalization_status.json");
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("read finalization status {}: {e}", path.display()))?;
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| format!("parse finalization status {}: {e}", path.display()))?;
    Ok(value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string))
}

pub fn recorded_run_lifecycle_summary(run_id: &str, artifact_dir: &Path) -> String {
    let quality_report_path = artifact_dir.join("quality_report.txt");
    format!(
        "agentactr run `{run_id}` completed. Review artifacts before terminal GitHub finalization.\n\nArtifacts: `{}`\nQuality report: `{}`",
        repo_relative_artifact_path(artifact_dir),
        repo_relative_artifact_path(&quality_report_path)
    )
}

pub fn apply_issue_lifecycle_with_tracker(
    tracker: &dyn IssueTracker,
    req: IssueLifecycleRequest,
) -> Result<IssueLifecycleReport, String> {
    match req.mode {
        IssueLifecycleMode::Disabled => Ok(IssueLifecycleReport {
            status: "disabled".to_string(),
            release: None,
        }),
        IssueLifecycleMode::RequireHumanReview => {
            let release = tracker.release(ReleaseRequest {
                repo: req.outcome.repo.clone(),
                issue_number: req.outcome.issue_number,
                run_id: req.outcome.run_id.clone(),
                outcome: IssueReleaseOutcome::ReviewRequired,
                add_labels: Vec::new(),
                remove_labels: vec![req.labels.running_label.clone()],
                close_state_reason: None,
                final_comment: Some(lifecycle_comment(
                    &req.outcome,
                    IssueCommentKind::ReviewRequired,
                    "review-required",
                    req.summary,
                )),
            })?;
            Ok(IssueLifecycleReport {
                status: "review_required".to_string(),
                release: Some(release),
            })
        }
        IssueLifecycleMode::AutomaticAfterQualityGates => {
            if !req.outcome.runtime_success || !req.outcome.quality.success {
                return Err(
                    "automatic finalization requires successful runtime and quality gates"
                        .to_string(),
                );
            }
            let release = tracker.release(ReleaseRequest {
                repo: req.outcome.repo.clone(),
                issue_number: req.outcome.issue_number,
                run_id: req.outcome.run_id.clone(),
                outcome: IssueReleaseOutcome::Finalized,
                add_labels: vec![req.labels.done_label.clone()],
                remove_labels: vec![
                    req.labels.claim_label.clone(),
                    req.labels.running_label.clone(),
                    req.labels.failed_label.clone(),
                ],
                close_state_reason: Some("completed".to_string()),
                final_comment: Some(lifecycle_comment(
                    &req.outcome,
                    IssueCommentKind::FinalSummary,
                    "final-summary",
                    req.summary,
                )),
            })?;
            Ok(IssueLifecycleReport {
                status: "finalized".to_string(),
                release: Some(release),
            })
        }
        IssueLifecycleMode::Failure { reason } => {
            let release = tracker.release(ReleaseRequest {
                repo: req.outcome.repo.clone(),
                issue_number: req.outcome.issue_number,
                run_id: req.outcome.run_id.clone(),
                outcome: IssueReleaseOutcome::Failure,
                add_labels: vec![req.labels.failed_label.clone()],
                remove_labels: vec![req.labels.running_label.clone()],
                close_state_reason: None,
                final_comment: Some(lifecycle_comment(
                    &req.outcome,
                    IssueCommentKind::FailureSummary,
                    "failure-summary",
                    format!("{}\n\nFailure reason: {reason}", req.summary),
                )),
            })?;
            Ok(IssueLifecycleReport {
                status: "failed".to_string(),
                release: Some(release),
            })
        }
        IssueLifecycleMode::Reject { reason } => {
            let release = tracker.release(ReleaseRequest {
                repo: req.outcome.repo.clone(),
                issue_number: req.outcome.issue_number,
                run_id: req.outcome.run_id.clone(),
                outcome: IssueReleaseOutcome::ReviewRejected,
                add_labels: Vec::new(),
                remove_labels: vec![req.labels.running_label.clone()],
                close_state_reason: None,
                final_comment: Some(lifecycle_comment(
                    &req.outcome,
                    IssueCommentKind::RejectionSummary,
                    "review-rejected",
                    format!("{}\n\nReview rejected. Reason: {reason}", req.summary),
                )),
            })?;
            Ok(IssueLifecycleReport {
                status: "review_rejected".to_string(),
                release: Some(release),
            })
        }
    }
}

fn lifecycle_comment(
    outcome: &RunOutcomeSummary,
    kind: IssueCommentKind,
    key: &str,
    body: String,
) -> CommentRequest {
    let body = sanitize_lifecycle_comment_body(outcome, &body);
    CommentRequest {
        repo: outcome.repo.clone(),
        issue_number: outcome.issue_number,
        run_id: outcome.run_id.clone(),
        kind,
        idempotency_key: format!("{}:{}:{key}", outcome.run_id, outcome.issue_number),
        body,
        update_existing: true,
    }
}

fn sanitize_lifecycle_comment_body(outcome: &RunOutcomeSummary, body: &str) -> String {
    let artifact_dir = outcome.artifact_dir.display().to_string();
    let artifact_ref = repo_relative_artifact_path(&outcome.artifact_dir);
    let quality_report = outcome.artifact_dir.join("quality_report.txt");
    let quality_report_abs = quality_report.display().to_string();
    let quality_report_ref = repo_relative_artifact_path(&quality_report);
    body.replace(&quality_report_abs, &quality_report_ref)
        .replace(&artifact_dir, &artifact_ref)
}

fn repo_relative_artifact_path(path: &Path) -> String {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if let Some(index) = components
        .iter()
        .position(|component| component == ".agentactr")
    {
        return components[index..].join("/");
    }
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn run_phase<T>(
    hooks: &mut dyn RunIssueHooks,
    phase: &'static str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    run_phase_inner(hooks, phase, |_| operation())
}

fn run_hook_phase<T>(
    hooks: &mut dyn RunIssueHooks,
    phase: &'static str,
    operation: impl FnOnce(&mut dyn RunIssueHooks) -> Result<T, String>,
) -> Result<T, String> {
    run_phase_inner(hooks, phase, operation)
}

fn run_phase_inner<T>(
    hooks: &mut dyn RunIssueHooks,
    phase: &'static str,
    operation: impl FnOnce(&mut dyn RunIssueHooks) -> Result<T, String>,
) -> Result<T, String> {
    hooks.phase_started(phase)?;
    match operation(hooks) {
        Ok(value) => {
            hooks.phase_completed(phase)?;
            Ok(value)
        }
        Err(err) => {
            hooks.phase_failed(phase, &err)?;
            Err(err)
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunIssueRequest {
    pub issue_id: IssueId,
    pub worktree: WorktreeRequest,
}

#[derive(Clone, Debug)]
pub struct DefaultSpawnPlanRequest<'a> {
    pub config: &'a AgentactrConfig,
    pub run_id: &'a str,
    pub writer_agent_run_id: &'a str,
    pub artifact_root: &'a std::path::Path,
}

pub fn default_spawn_plan(req: DefaultSpawnPlanRequest<'_>) -> SpawnPlan {
    let manager = SpawnManager::new(SpawnPolicy::from(&req.config.spawn));
    let candidates = vec![
        SpawnRequest::read_only_helper(
            AgentRole::RepoExplorer,
            "Map relevant repository boundaries, existing patterns, and likely files for the issue.",
            ReadScope::FullWorkspace,
        ),
        SpawnRequest::read_only_helper(
            AgentRole::Reproducer,
            "Inspect likely reproduction or verification paths and identify focused checks.",
            ReadScope::FullWorkspace,
        ),
        SpawnRequest::read_only_helper(
            AgentRole::Reviewer,
            "Review likely implementation risks, edge cases, and regression hazards before the writer edits.",
            ReadScope::FullWorkspace,
        ),
    ];
    manager.plan(SpawnPlanRequest {
        run_id: RunId::new(req.run_id),
        parent_agent_run_id: AgentRunId::new(req.writer_agent_run_id),
        writer_agent_run_id: AgentRunId::new(req.writer_agent_run_id),
        artifact_root: req.artifact_root.join("spawn"),
        candidates,
        budget: RunBudgetSnapshot::default(),
    })
}

#[derive(Clone, Debug)]
pub struct RunIssueReport {
    pub worktree: WorktreeRef,
    pub issue_context: Issue,
    pub runtime: AgentRunReport,
}

pub struct RunIssueRuntimeContext<'a> {
    pub request: &'a RunIssueRequest,
    pub worktree: &'a WorktreeRef,
    pub issue_context: &'a Issue,
}

pub struct RunIssuePostRuntimeContext<'a> {
    pub request: &'a RunIssueRequest,
    pub worktree: &'a WorktreeRef,
    pub issue_context: &'a Issue,
    pub runtime_report: &'a AgentRunReport,
}

pub trait RunIssueHooks {
    fn phase_started(&mut self, _phase: &str) -> Result<(), String> {
        Ok(())
    }

    fn phase_completed(&mut self, _phase: &str) -> Result<(), String> {
        Ok(())
    }

    fn phase_failed(&mut self, _phase: &str, _error: &str) -> Result<(), String> {
        Ok(())
    }

    fn before_runtime(
        &mut self,
        context: RunIssueRuntimeContext<'_>,
    ) -> Result<AgentIssueRunRequest, String>;

    fn after_runtime_success(
        &mut self,
        _context: RunIssuePostRuntimeContext<'_>,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentactr_core::{
        AdapterCapabilities, AgentRuntimeCapabilities, AgentSession, AgentStartRequest,
        AgentTurnRequest, AgentTurnStream, CancelReason, CandidateQuery, ClaimRequest, ClaimResult,
        CommentRef, CommentRequest, CommitRef, CommitRequest, MergePlan, MergePlanRequest,
        PortResult, ReleaseRequest, ReleaseResult, VcsCapabilities, WorkspaceDiff,
    };
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct Calls(Arc<Mutex<Vec<&'static str>>>);

    impl Calls {
        fn push(&self, value: &'static str) {
            self.0.lock().unwrap().push(value);
        }

        fn snapshot(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    struct FakeVcs {
        calls: Calls,
    }

    impl VersionControl for FakeVcs {
        fn capabilities(&self) -> AdapterCapabilities {
            AdapterCapabilities::unknown("vcs")
        }

        fn detect(&self, _root: &Path) -> PortResult<VcsCapabilities> {
            Ok(VcsCapabilities)
        }

        fn prepare_workspace(&self, req: WorktreeRequest) -> PortResult<WorktreeRef> {
            self.calls.push("worktree");
            Ok(WorktreeRef {
                path: req.worktree_root.join(&req.run_id),
                base_commit: "abc123".to_string(),
                run_id: req.run_id,
            })
        }

        fn diff(&self, _worktree: &WorktreeRef) -> PortResult<WorkspaceDiff> {
            Ok(WorkspaceDiff::default())
        }

        fn commit(&self, _req: CommitRequest) -> PortResult<CommitRef> {
            Ok(CommitRef)
        }

        fn merge_plan(&self, _req: MergePlanRequest) -> PortResult<MergePlan> {
            Ok(MergePlan::default())
        }
    }

    struct FakeTracker {
        calls: Calls,
    }

    impl IssueTracker for FakeTracker {
        fn capabilities(&self) -> AdapterCapabilities {
            AdapterCapabilities::unknown("tracker")
        }

        fn fetch_candidates(&self, _q: CandidateQuery) -> PortResult<Vec<Issue>> {
            Ok(Vec::new())
        }

        fn fetch_by_ids(&self, _ids: &[IssueId]) -> PortResult<Vec<Issue>> {
            self.calls.push("github_fetch");
            Ok(vec![Issue {
                id: "OWNER/REPO#42".to_string(),
                repo: "OWNER/REPO".to_string(),
                number: 42,
                title: "Test".to_string(),
                ..Issue::default()
            }])
        }

        fn claim(&self, _req: ClaimRequest) -> PortResult<ClaimResult> {
            Ok(ClaimResult::default())
        }

        fn release(&self, _req: ReleaseRequest) -> PortResult<ReleaseResult> {
            Ok(ReleaseResult::default())
        }

        fn comment(&self, _req: CommentRequest) -> PortResult<CommentRef> {
            Ok(CommentRef::default())
        }
    }

    #[derive(Clone, Default)]
    struct CapturingTracker {
        releases: Arc<Mutex<Vec<ReleaseRequest>>>,
    }

    impl CapturingTracker {
        fn releases(&self) -> Vec<ReleaseRequest> {
            self.releases.lock().unwrap().clone()
        }
    }

    impl IssueTracker for CapturingTracker {
        fn capabilities(&self) -> AdapterCapabilities {
            AdapterCapabilities::unknown("tracker")
        }

        fn fetch_candidates(&self, _q: CandidateQuery) -> PortResult<Vec<Issue>> {
            Ok(Vec::new())
        }

        fn fetch_by_ids(&self, _ids: &[IssueId]) -> PortResult<Vec<Issue>> {
            Ok(Vec::new())
        }

        fn claim(&self, _req: ClaimRequest) -> PortResult<ClaimResult> {
            Ok(ClaimResult::default())
        }

        fn release(&self, req: ReleaseRequest) -> PortResult<ReleaseResult> {
            self.releases.lock().unwrap().push(req.clone());
            Ok(ReleaseResult {
                applied_labels: req.add_labels,
                removed_labels: req.remove_labels,
                final_issue_state: if req.close_state_reason.is_some() {
                    "closed".to_string()
                } else {
                    "open".to_string()
                },
                state_reason: req.close_state_reason,
                comment_refs: Vec::new(),
                source_artifacts: Vec::new(),
                verification_status: "verified".to_string(),
                mismatch_details: Vec::new(),
            })
        }

        fn comment(&self, _req: CommentRequest) -> PortResult<CommentRef> {
            Ok(CommentRef::default())
        }
    }

    struct FakeRuntime {
        calls: Calls,
    }

    impl AgentRuntime for FakeRuntime {
        fn capabilities(&self) -> AgentRuntimeCapabilities {
            AgentRuntimeCapabilities {
                single_shot_issue_run: true,
                ..AgentRuntimeCapabilities::default()
            }
        }

        fn start(&self, _req: AgentStartRequest) -> PortResult<AgentSession> {
            Ok(AgentSession)
        }

        fn run_turn(&self, _req: AgentTurnRequest) -> PortResult<AgentTurnStream> {
            Ok(AgentTurnStream)
        }

        fn run_issue(&self, _req: AgentIssueRunRequest) -> PortResult<AgentRunReport> {
            self.calls.push("codex_exec");
            Ok(AgentRunReport::default())
        }

        fn cancel(&self, _session_id: &str, _reason: CancelReason) -> PortResult<()> {
            Ok(())
        }
    }

    struct FakeHooks {
        calls: Calls,
    }

    impl RunIssueHooks for FakeHooks {
        fn phase_started(&mut self, phase: &str) -> Result<(), String> {
            match phase {
                "worktree" => self.calls.push("phase:worktree"),
                "github_fetch" => self.calls.push("phase:github_fetch"),
                "codex_preflight" => self.calls.push("phase:codex_preflight"),
                "codex_exec" => self.calls.push("phase:codex_exec"),
                _ => {}
            }
            Ok(())
        }

        fn before_runtime(
            &mut self,
            context: RunIssueRuntimeContext<'_>,
        ) -> Result<AgentIssueRunRequest, String> {
            self.calls.push("before_runtime");
            Ok(AgentIssueRunRequest {
                worktree: context.worktree.path.clone(),
                issue_context: context.issue_context.clone(),
                ..AgentIssueRunRequest::default()
            })
        }

        fn after_runtime_success(
            &mut self,
            _context: RunIssuePostRuntimeContext<'_>,
        ) -> Result<(), String> {
            self.calls.push("after_runtime");
            Ok(())
        }
    }

    #[test]
    fn builder_requires_core_dependencies() {
        let err = AgentActrBuilder::new().build().err().unwrap();
        assert!(err.contains("issue_tracker"));
    }

    #[test]
    fn run_issue_orders_workspace_tracker_preflight_and_runtime() {
        let calls = Calls::default();
        let use_cases = AgentActrBuilder::new()
            .version_control(Arc::new(FakeVcs {
                calls: calls.clone(),
            }))
            .issue_tracker(Arc::new(FakeTracker {
                calls: calls.clone(),
            }))
            .runtime(Arc::new(FakeRuntime {
                calls: calls.clone(),
            }))
            .build()
            .unwrap();
        let mut hooks = FakeHooks {
            calls: calls.clone(),
        };

        let report = use_cases
            .run_issue(
                RunIssueRequest {
                    issue_id: IssueId("OWNER/REPO#42".to_string()),
                    worktree: WorktreeRequest {
                        run_id: "run-1".to_string(),
                        worktree_root: std::env::temp_dir(),
                        ..WorktreeRequest::default()
                    },
                },
                &mut hooks,
            )
            .unwrap();

        assert_eq!(report.issue_context.number, 42);
        assert_eq!(
            calls.snapshot(),
            vec![
                "phase:worktree",
                "worktree",
                "phase:github_fetch",
                "github_fetch",
                "phase:codex_preflight",
                "before_runtime",
                "phase:codex_exec",
                "codex_exec",
                "after_runtime",
            ]
        );
    }

    fn lifecycle_labels() -> LifecycleLabels {
        LifecycleLabels {
            claim_label: "agentactr:claimed".to_string(),
            running_label: "agentactr:running".to_string(),
            failed_label: "agentactr:failed".to_string(),
            done_label: "agentactr:done".to_string(),
        }
    }

    fn write_finalization_fixture(root: &Path, status: &str, quality_success: bool) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("quality_report.txt"), "quality report\n").unwrap();
        fs::write(
            quality_status_path(&root.join("quality_report.txt")),
            serde_json::json!({
                "schema_version": "0.1",
                "success": quality_success,
                "report_path": root.join("quality_report.txt").display().to_string(),
                "failed_reason": if quality_success { serde_json::Value::Null } else { serde_json::Value::String("failed gate".to_string()) },
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            root.join("finalization_status.json"),
            serde_json::json!({
                "schema_version": "0.1",
                "run_id": "run-1",
                "repo": "OWNER/REPO",
                "issue": "42",
                "status": status,
                "mode": "require_human_review",
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn recorded_finalization_approve_loads_artifacts_and_closes_via_sdk() {
        let root =
            std::env::temp_dir().join(format!("agentactr-sdk-finalize-{}", std::process::id()));
        write_finalization_fixture(&root, "review_required", true);
        let tracker = CapturingTracker::default();

        let report = finalize_recorded_run_with_tracker(
            &tracker,
            &FsRunFinalizationArtifacts::new(&root),
            RecordedRunFinalizationRequest {
                run_id: "run-1".to_string(),
                repo: "OWNER/REPO".to_string(),
                issue_number: 42,
                decision: FinalizeDecision::Approve,
                reject_reason: None,
                labels: lifecycle_labels(),
                resume: false,
            },
        )
        .unwrap();

        assert_eq!(report.lifecycle.status, "finalized");
        assert_eq!(report.prior_status.as_deref(), Some("review_required"));
        let releases = tracker.releases();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].close_state_reason.as_deref(), Some("completed"));
        assert!(releases[0]
            .add_labels
            .contains(&"agentactr:done".to_string()));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recorded_finalization_approve_rejects_missing_status_before_mutation() {
        let root = std::env::temp_dir().join(format!(
            "agentactr-sdk-finalize-missing-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("quality_report.txt"), "quality report\n").unwrap();
        fs::write(
            quality_status_path(&root.join("quality_report.txt")),
            serde_json::json!({
                "schema_version": "0.1",
                "success": true,
            })
            .to_string(),
        )
        .unwrap();
        let tracker = CapturingTracker::default();

        let err = finalize_recorded_run_with_tracker(
            &tracker,
            &FsRunFinalizationArtifacts::new(&root),
            RecordedRunFinalizationRequest {
                run_id: "run-1".to_string(),
                repo: "OWNER/REPO".to_string(),
                issue_number: 42,
                decision: FinalizeDecision::Approve,
                reject_reason: None,
                labels: lifecycle_labels(),
                resume: false,
            },
        )
        .unwrap_err();

        assert!(err.contains("recorded finalization_status.json"));
        assert!(tracker.releases().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn require_human_review_never_closes_or_applies_done_label() {
        let tracker = CapturingTracker::default();
        let outcome = RunOutcomeSummary {
            run_id: "run-1".to_string(),
            repo: "OWNER/REPO".to_string(),
            issue_number: 42,
            runtime_success: true,
            quality: QualityGateSummary {
                success: true,
                report_path: None,
                failed_reason: None,
            },
            artifact_dir: PathBuf::from("/tmp/artifacts"),
        };

        let report = apply_issue_lifecycle_with_tracker(
            &tracker,
            IssueLifecycleRequest {
                mode: IssueLifecycleMode::RequireHumanReview,
                outcome,
                labels: lifecycle_labels(),
                summary: "summary".to_string(),
            },
        )
        .unwrap();

        assert_eq!(report.status, "review_required");
        let release = tracker.releases().pop().unwrap();
        assert!(release.add_labels.is_empty());
        assert_eq!(release.close_state_reason, None);
        assert_eq!(release.remove_labels, vec!["agentactr:running".to_string()]);
    }

    #[test]
    fn lifecycle_comments_use_repo_relative_artifact_paths() {
        let artifact_dir = PathBuf::from(
            "/Users/chrisbanerjee/ghprojs/ielts-study-companion/.agentactr/artifacts/issue-1-1",
        );
        let outcome = RunOutcomeSummary {
            run_id: "issue-1-1".to_string(),
            repo: "OWNER/REPO".to_string(),
            issue_number: 1,
            runtime_success: false,
            quality: QualityGateSummary {
                success: false,
                report_path: Some(artifact_dir.join("quality_report.txt")),
                failed_reason: Some(format!(
                    "strict quality gate failed; report={}",
                    artifact_dir.join("quality_report.txt").display()
                )),
            },
            artifact_dir: artifact_dir.clone(),
        };
        let body = format!(
            "Artifacts: `{}`\nQuality report: `{}`\nFailure: report={}",
            artifact_dir.display(),
            artifact_dir.join("quality_report.txt").display(),
            artifact_dir.join("quality_report.txt").display()
        );

        let comment = lifecycle_comment(
            &outcome,
            IssueCommentKind::FailureSummary,
            "failure-summary",
            body,
        );

        assert!(!comment.body.contains("/Users/chrisbanerjee"));
        assert!(comment
            .body
            .contains(".agentactr/artifacts/issue-1-1/quality_report.txt"));
        assert!(recorded_run_lifecycle_summary("issue-1-1", &artifact_dir)
            .contains(".agentactr/artifacts/issue-1-1"));
    }
}
