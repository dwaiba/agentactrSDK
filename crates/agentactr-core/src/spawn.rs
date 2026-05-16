use std::path::PathBuf;

use crate::{AgentRunId, MemoryPolicyRef, RunId, SpawnConfig};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentRole {
    Lead,
    RepoExplorer,
    Reproducer,
    Implementer,
    Reviewer,
    QualityAgent,
    Finalizer,
    VcsAgent,
    PolicyAgent,
    Custom(String),
}

impl AgentRole {
    pub fn is_default_writer(&self) -> bool {
        matches!(self, Self::Implementer)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Lead => "Lead",
            Self::RepoExplorer => "RepoExplorer",
            Self::Reproducer => "Reproducer",
            Self::Implementer => "Implementer",
            Self::Reviewer => "Reviewer",
            Self::QualityAgent => "QualityAgent",
            Self::Finalizer => "Finalizer",
            Self::VcsAgent => "VcsAgent",
            Self::PolicyAgent => "PolicyAgent",
            Self::Custom(value) => value.as_str(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadScope {
    FullWorkspace,
    Paths(Vec<String>),
    ArtifactsOnly,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteScope {
    None,
    ArtifactsOnly,
    Workspace(Vec<String>),
}

impl WriteScope {
    pub fn allows_workspace_write(&self) -> bool {
        matches!(self, Self::Workspace(_))
    }

    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::None)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolPolicy {
    pub allow_shell: bool,
    pub allow_network: bool,
    pub allow_write_tools: bool,
}

impl ToolPolicy {
    pub fn read_only() -> Self {
        Self {
            allow_shell: true,
            allow_network: false,
            allow_write_tools: false,
        }
    }

    pub fn writer() -> Self {
        Self {
            allow_shell: true,
            allow_network: false,
            allow_write_tools: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextBudget {
    pub max_uncached_input_tokens: u64,
    pub max_files: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputBudget {
    pub max_output_tokens: u64,
    pub max_artifact_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentNodeStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentNode {
    pub agent_run_id: AgentRunId,
    pub parent_agent_run_id: Option<AgentRunId>,
    pub role: AgentRole,
    pub objective: String,
    pub read_scope: ReadScope,
    pub write_scope: WriteScope,
    pub tool_policy: ToolPolicy,
    pub context_budget: ContextBudget,
    pub output_budget: OutputBudget,
    pub memory_policy: Option<MemoryPolicyRef>,
    pub artifact_dir: PathBuf,
    pub status: AgentNodeStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnPolicy {
    pub enabled: bool,
    pub max_child_agents_per_issue: u32,
    pub max_spawn_depth: u32,
    pub allow_parallel_read_only: bool,
    pub allow_parallel_writers: bool,
    pub max_total_uncached_input_tokens: u64,
    pub max_child_uncached_input_tokens: u64,
    pub max_child_output_tokens: u64,
    pub pause_on_memory_pressure: bool,
}

impl Default for SpawnPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_child_agents_per_issue: 4,
            max_spawn_depth: 1,
            allow_parallel_read_only: true,
            allow_parallel_writers: false,
            max_total_uncached_input_tokens: 250_000,
            max_child_uncached_input_tokens: 80_000,
            max_child_output_tokens: 12_000,
            pause_on_memory_pressure: true,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunBudgetSnapshot {
    pub child_agents_started: u32,
    pub active_child_agents: u32,
    pub current_depth: u32,
    pub total_uncached_input_tokens: u64,
    pub total_output_tokens: u64,
    pub memory_pressure_active: bool,
    pub writer_active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnRequest {
    pub parent_agent_run_id: Option<AgentRunId>,
    pub role: AgentRole,
    pub objective: String,
    pub read_scope: ReadScope,
    pub write_scope: WriteScope,
    pub tool_policy: ToolPolicy,
    pub memory_policy: Option<MemoryPolicyRef>,
}

impl SpawnRequest {
    pub fn read_only_helper(
        role: AgentRole,
        objective: impl Into<String>,
        read_scope: ReadScope,
    ) -> Self {
        Self {
            parent_agent_run_id: None,
            role,
            objective: objective.into(),
            read_scope,
            write_scope: WriteScope::None,
            tool_policy: ToolPolicy::read_only(),
            memory_policy: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnAction {
    Spawn,
    Skip,
    FailClosed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnReason {
    Allowed,
    PolicyDisabled,
    MaxChildrenReached,
    MaxDepthReached,
    MemoryPressure,
    BudgetExhausted,
    ParallelReadOnlyDisabled,
    ParallelWriterDisabled,
    WriterAlreadyActive,
    ScopeNotEnforceable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnDecision {
    pub action: SpawnAction,
    pub reason: SpawnReason,
    pub budget_after_spawn: RunBudgetSnapshot,
}

impl SpawnDecision {
    fn new(
        action: SpawnAction,
        reason: SpawnReason,
        budget_after_spawn: RunBudgetSnapshot,
    ) -> Self {
        Self {
            action,
            reason,
            budget_after_spawn,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnManager {
    pub policy: SpawnPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnPlanRequest {
    pub run_id: RunId,
    pub parent_agent_run_id: AgentRunId,
    pub writer_agent_run_id: AgentRunId,
    pub artifact_root: PathBuf,
    pub candidates: Vec<SpawnRequest>,
    pub budget: RunBudgetSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnDecisionRecord {
    pub role: AgentRole,
    pub objective: String,
    pub decision: SpawnDecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnPlan {
    pub run_id: RunId,
    pub parent_agent_run_id: AgentRunId,
    pub writer_agent_run_id: AgentRunId,
    pub child_nodes: Vec<AgentNode>,
    pub decisions: Vec<SpawnDecisionRecord>,
    pub handoffs: Vec<ArtifactHandoff>,
}

impl From<&SpawnConfig> for SpawnPolicy {
    fn from(config: &SpawnConfig) -> Self {
        let mut policy = Self {
            enabled: config.enabled,
            max_child_agents_per_issue: config.max_child_agents_per_issue.min(u32::MAX as u64)
                as u32,
            max_spawn_depth: config.max_spawn_depth.min(u32::MAX as u64) as u32,
            allow_parallel_read_only: config.allow_parallel_read_only,
            allow_parallel_writers: config.allow_parallel_writers,
            max_total_uncached_input_tokens: config.max_total_uncached_input_tokens,
            max_child_uncached_input_tokens: config.max_child_uncached_input_tokens,
            max_child_output_tokens: config.max_child_output_tokens,
            pause_on_memory_pressure: config.pause_on_memory_pressure,
        };
        if !policy.allow_parallel_writers {
            policy.max_spawn_depth = policy.max_spawn_depth.min(1);
        }
        policy
    }
}

impl SpawnManager {
    pub fn new(policy: SpawnPolicy) -> Self {
        Self { policy }
    }

    pub fn decide(&self, request: &SpawnRequest, budget: &RunBudgetSnapshot) -> SpawnDecision {
        if !self.policy.enabled {
            return self.skip(SpawnReason::PolicyDisabled, budget);
        }
        if budget.child_agents_started >= self.policy.max_child_agents_per_issue {
            return self.skip(SpawnReason::MaxChildrenReached, budget);
        }
        if budget.current_depth >= self.policy.max_spawn_depth {
            return self.skip(SpawnReason::MaxDepthReached, budget);
        }
        if self.policy.pause_on_memory_pressure
            && budget.memory_pressure_active
            && !request.role.is_default_writer()
        {
            return self.skip(SpawnReason::MemoryPressure, budget);
        }
        if request.write_scope.allows_workspace_write() && !request.role.is_default_writer() {
            return self.fail_closed(SpawnReason::ScopeNotEnforceable, budget);
        }
        if request.write_scope.allows_workspace_write()
            && !self.policy.allow_parallel_writers
            && budget.writer_active
        {
            return self.skip(SpawnReason::WriterAlreadyActive, budget);
        }
        if request.write_scope.is_read_only()
            && !self.policy.allow_parallel_read_only
            && budget.active_child_agents > 0
        {
            return self.skip(SpawnReason::ParallelReadOnlyDisabled, budget);
        }
        if self.would_exceed_uncached_budget(budget) {
            return self.skip(SpawnReason::BudgetExhausted, budget);
        }

        let mut budget_after_spawn = budget.clone();
        budget_after_spawn.child_agents_started =
            budget_after_spawn.child_agents_started.saturating_add(1);
        budget_after_spawn.active_child_agents =
            budget_after_spawn.active_child_agents.saturating_add(1);
        budget_after_spawn.total_uncached_input_tokens = budget_after_spawn
            .total_uncached_input_tokens
            .saturating_add(self.policy.max_child_uncached_input_tokens);
        budget_after_spawn.total_output_tokens = budget_after_spawn
            .total_output_tokens
            .saturating_add(self.policy.max_child_output_tokens);
        if request.write_scope.allows_workspace_write() {
            budget_after_spawn.writer_active = true;
        }

        SpawnDecision::new(SpawnAction::Spawn, SpawnReason::Allowed, budget_after_spawn)
    }

    pub fn build_node(
        &self,
        agent_run_id: AgentRunId,
        request: SpawnRequest,
        artifact_dir: PathBuf,
    ) -> AgentNode {
        AgentNode {
            agent_run_id,
            parent_agent_run_id: request.parent_agent_run_id,
            role: request.role,
            objective: request.objective,
            read_scope: request.read_scope,
            write_scope: request.write_scope,
            tool_policy: request.tool_policy,
            context_budget: ContextBudget {
                max_uncached_input_tokens: self.policy.max_child_uncached_input_tokens,
                max_files: 64,
            },
            output_budget: OutputBudget {
                max_output_tokens: self.policy.max_child_output_tokens,
                max_artifact_bytes: 1_048_576,
            },
            memory_policy: request.memory_policy,
            artifact_dir,
            status: AgentNodeStatus::Pending,
        }
    }

    pub fn plan(&self, request: SpawnPlanRequest) -> SpawnPlan {
        let mut budget = request.budget;
        let mut child_nodes = Vec::new();
        let mut decisions = Vec::new();
        let mut handoffs = Vec::new();

        for (index, mut candidate) in request.candidates.into_iter().enumerate() {
            candidate.parent_agent_run_id = Some(request.parent_agent_run_id.clone());
            let decision = self.decide(&candidate, &budget);
            decisions.push(SpawnDecisionRecord {
                role: candidate.role.clone(),
                objective: candidate.objective.clone(),
                decision: decision.clone(),
            });
            if decision.action != SpawnAction::Spawn {
                continue;
            }

            budget = decision.budget_after_spawn;
            let agent_run_id = AgentRunId::new(format!(
                "{}-child-{}-{}",
                request.run_id.as_str(),
                index + 1,
                candidate.role.as_str().to_ascii_lowercase()
            ));
            let artifact_dir = request.artifact_root.join(agent_run_id.as_str());
            let node = self.build_node(agent_run_id.clone(), candidate, artifact_dir.clone());
            handoffs.push(ArtifactHandoff {
                artifact: ArtifactRef {
                    path: artifact_dir.join("handoff.md"),
                    digest: "pending".to_string(),
                    producer_agent_run_id: agent_run_id,
                    redaction_state: RedactionState::NotRequired,
                },
                consumer_agent_run_id: request.writer_agent_run_id.clone(),
                visibility: ArtifactVisibility::ReferenceOnly,
            });
            child_nodes.push(node);
        }

        SpawnPlan {
            run_id: request.run_id,
            parent_agent_run_id: request.parent_agent_run_id,
            writer_agent_run_id: request.writer_agent_run_id,
            child_nodes,
            decisions,
            handoffs,
        }
    }

    fn would_exceed_uncached_budget(&self, budget: &RunBudgetSnapshot) -> bool {
        budget
            .total_uncached_input_tokens
            .saturating_add(self.policy.max_child_uncached_input_tokens)
            > self.policy.max_total_uncached_input_tokens
    }

    fn skip(&self, reason: SpawnReason, budget: &RunBudgetSnapshot) -> SpawnDecision {
        SpawnDecision::new(SpawnAction::Skip, reason, budget.clone())
    }

    fn fail_closed(&self, reason: SpawnReason, budget: &RunBudgetSnapshot) -> SpawnDecision {
        SpawnDecision::new(SpawnAction::FailClosed, reason, budget.clone())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactVisibility {
    ReferenceOnly,
    Summary,
    FullBody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RedactionState {
    Redacted,
    Unredacted,
    NotRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactRef {
    pub path: PathBuf,
    pub digest: String,
    pub producer_agent_run_id: AgentRunId,
    pub redaction_state: RedactionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactHandoff {
    pub artifact: ArtifactRef,
    pub consumer_agent_run_id: AgentRunId,
    pub visibility: ArtifactVisibility,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> SpawnManager {
        SpawnManager::new(SpawnPolicy::default())
    }

    #[test]
    fn allows_budgeted_read_only_helper() {
        let request = SpawnRequest::read_only_helper(
            AgentRole::RepoExplorer,
            "map repository boundaries",
            ReadScope::FullWorkspace,
        );
        let decision = manager().decide(&request, &RunBudgetSnapshot::default());

        assert_eq!(decision.action, SpawnAction::Spawn);
        assert_eq!(decision.reason, SpawnReason::Allowed);
        assert_eq!(decision.budget_after_spawn.child_agents_started, 1);
    }

    #[test]
    fn fails_closed_when_read_only_role_requests_workspace_write() {
        let request = SpawnRequest {
            parent_agent_run_id: None,
            role: AgentRole::Reviewer,
            objective: "review and patch".to_string(),
            read_scope: ReadScope::FullWorkspace,
            write_scope: WriteScope::Workspace(vec!["src".to_string()]),
            tool_policy: ToolPolicy::writer(),
            memory_policy: None,
        };
        let decision = manager().decide(&request, &RunBudgetSnapshot::default());

        assert_eq!(decision.action, SpawnAction::FailClosed);
        assert_eq!(decision.reason, SpawnReason::ScopeNotEnforceable);
    }

    #[test]
    fn keeps_one_writer_policy_by_default() {
        let request = SpawnRequest {
            parent_agent_run_id: None,
            role: AgentRole::Implementer,
            objective: "modify code".to_string(),
            read_scope: ReadScope::FullWorkspace,
            write_scope: WriteScope::Workspace(vec!["crates".to_string()]),
            tool_policy: ToolPolicy::writer(),
            memory_policy: None,
        };
        let budget = RunBudgetSnapshot {
            writer_active: true,
            ..RunBudgetSnapshot::default()
        };
        let decision = manager().decide(&request, &budget);

        assert_eq!(decision.action, SpawnAction::Skip);
        assert_eq!(decision.reason, SpawnReason::WriterAlreadyActive);
    }

    #[test]
    fn allows_first_writer_under_one_writer_policy() {
        let request = SpawnRequest {
            parent_agent_run_id: None,
            role: AgentRole::Implementer,
            objective: "modify code".to_string(),
            read_scope: ReadScope::FullWorkspace,
            write_scope: WriteScope::Workspace(vec!["crates".to_string()]),
            tool_policy: ToolPolicy::writer(),
            memory_policy: None,
        };
        let decision = manager().decide(&request, &RunBudgetSnapshot::default());

        assert_eq!(decision.action, SpawnAction::Spawn);
        assert!(decision.budget_after_spawn.writer_active);
    }

    #[test]
    fn pauses_helpers_under_memory_pressure() {
        let request = SpawnRequest::read_only_helper(
            AgentRole::Reproducer,
            "reproduce failure",
            ReadScope::FullWorkspace,
        );
        let budget = RunBudgetSnapshot {
            memory_pressure_active: true,
            ..RunBudgetSnapshot::default()
        };
        let decision = manager().decide(&request, &budget);

        assert_eq!(decision.action, SpawnAction::Skip);
        assert_eq!(decision.reason, SpawnReason::MemoryPressure);
    }

    #[test]
    fn enforces_uncached_input_budget() {
        let request = SpawnRequest::read_only_helper(
            AgentRole::RepoExplorer,
            "map repository boundaries",
            ReadScope::FullWorkspace,
        );
        let budget = RunBudgetSnapshot {
            total_uncached_input_tokens: 200_001,
            ..RunBudgetSnapshot::default()
        };
        let decision = manager().decide(&request, &budget);

        assert_eq!(decision.action, SpawnAction::Skip);
        assert_eq!(decision.reason, SpawnReason::BudgetExhausted);
    }

    #[test]
    fn spawn_policy_uses_operator_budget_config() {
        let mut config = crate::AgentactrConfig::strict_defaults("OWNER/REPO").spawn;
        config.max_total_uncached_input_tokens = 123_000;
        config.max_child_uncached_input_tokens = 12_000;
        config.max_child_output_tokens = 3_000;
        config.pause_on_memory_pressure = false;

        let policy = SpawnPolicy::from(&config);

        assert_eq!(policy.max_total_uncached_input_tokens, 123_000);
        assert_eq!(policy.max_child_uncached_input_tokens, 12_000);
        assert_eq!(policy.max_child_output_tokens, 3_000);
        assert!(!policy.pause_on_memory_pressure);
    }

    #[test]
    fn artifact_handoff_is_referenceable_between_agents() {
        let handoff = ArtifactHandoff {
            artifact: ArtifactRef {
                path: PathBuf::from("artifacts/explorer/repo-map.json"),
                digest: "sha256:abc123".to_string(),
                producer_agent_run_id: AgentRunId::new("agent-explorer"),
                redaction_state: RedactionState::Redacted,
            },
            consumer_agent_run_id: AgentRunId::new("agent-implementer"),
            visibility: ArtifactVisibility::ReferenceOnly,
        };

        assert_eq!(
            handoff.artifact.producer_agent_run_id.as_str(),
            "agent-explorer"
        );
        assert_eq!(handoff.consumer_agent_run_id.as_str(), "agent-implementer");
        assert_eq!(handoff.visibility, ArtifactVisibility::ReferenceOnly);
    }

    #[test]
    fn plan_builds_read_only_children_and_handoffs() {
        let plan = manager().plan(SpawnPlanRequest {
            run_id: RunId::new("run-1"),
            parent_agent_run_id: AgentRunId::new("agent-lead"),
            writer_agent_run_id: AgentRunId::new("agent-writer"),
            artifact_root: PathBuf::from("artifacts/spawn"),
            candidates: vec![
                SpawnRequest::read_only_helper(
                    AgentRole::RepoExplorer,
                    "map repository",
                    ReadScope::FullWorkspace,
                ),
                SpawnRequest::read_only_helper(
                    AgentRole::Reviewer,
                    "review likely risks",
                    ReadScope::FullWorkspace,
                ),
            ],
            budget: RunBudgetSnapshot::default(),
        });

        assert_eq!(plan.child_nodes.len(), 2);
        assert_eq!(plan.handoffs.len(), 2);
        assert!(plan
            .child_nodes
            .iter()
            .all(|node| node.write_scope == WriteScope::None));
        assert_eq!(
            plan.handoffs[0].consumer_agent_run_id.as_str(),
            "agent-writer"
        );
    }
}
