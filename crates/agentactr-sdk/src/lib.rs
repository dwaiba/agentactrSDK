pub mod discovery;
pub mod domains;
pub mod issue_submission;
pub mod render;
pub mod resource_governor;
pub mod use_cases;

pub use agentactr_core::*;
pub use discovery::{
    apply_declared_stack_to_inspection, apply_declared_stack_to_inspection_with_config,
    discover_repository, discover_repository_with_config, quality_plan_for_repository,
    quality_plan_for_stack, QualityCommand, RepoInspection, StackKind,
};
pub use domains::{
    build_domain_graph, build_domain_graph_with_config, detect_domain_profiles,
    detect_domain_profiles_with_config, domain_findings, domain_findings_to_json,
    domain_graph_to_json, domain_profiles_to_json, domain_quality_plan,
    domain_quality_plan_to_json, domain_quality_plan_with_config, protobuf_profile, rpc_profile,
    DOMAIN_GRAPH_ARTIFACT_FORMAT_VERSION, DOMAIN_GRAPH_SCHEMA_VERSION,
};
pub use issue_submission::{
    draft_issue_proposals, draft_issue_proposals_from_structured_json, issue_submission_key,
    parent_issue_key, plan_issue_submission, plan_issue_submission_begin,
    prepare_issue_submission_proposal, validate_issue_submission_policy,
    DeterministicIssueDraftPlanner, IssueSubmissionBeginDecision, IssueSubmissionDecision,
    PreparedIssueSubmissionProposal,
};
pub use render::{
    annotate_agentactr_toml_possible_values, is_generated_agents_md, is_generated_project_spec_md,
    project_spec_filename, refresh_project_spec_md, render_agentactr_toml, render_agents_md,
    render_codex_config_toml, render_gitignore_additions, render_project_spec_md,
    render_workflow_md, DetectedCredentials,
};
pub use resource_governor::{
    select_helper_victim, HelperMemoryCandidate, HelperVictimSelection, MemoryGovernorPolicy,
    RunResourceGovernor, SkippedHelperCandidate,
};
pub use use_cases::{
    apply_issue_lifecycle_with_tracker, default_spawn_plan, finalize_recorded_run_with_tracker,
    finalize_run_with_tracker, load_recorded_quality_summary, quality_status_path,
    read_recorded_finalization_status, recorded_run_lifecycle_summary, AgentActr, AgentActrBuilder,
    AgentActrUseCases, DefaultSpawnPlanRequest, FinalizeDecision, FinalizeRunRequest,
    FsRunFinalizationArtifacts, IssueLifecycleMode, IssueLifecycleReport, IssueLifecycleRequest,
    LifecycleLabels, RecordedRunFinalizationReport, RecordedRunFinalizationRequest,
    RunFinalizationArtifactSource, RunIssueHooks, RunIssuePostRuntimeContext, RunIssueReport,
    RunIssueRequest, RunIssueRuntimeContext,
};
