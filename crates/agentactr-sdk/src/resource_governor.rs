use agentactr_core::{
    AgentRunId, MemoryAction, MemoryPressureSnapshot, MemoryPressureState,
    MemoryPressureTransition, RuntimeMemoryMitigation,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryGovernorPolicy {
    pub sustained_pressure_samples: u32,
    pub clear_pressure_samples: u32,
    pub request_runtime_compaction: bool,
    pub pause_read_only_spawns: bool,
}

impl Default for MemoryGovernorPolicy {
    fn default() -> Self {
        Self {
            sustained_pressure_samples: 2,
            clear_pressure_samples: 2,
            request_runtime_compaction: true,
            pause_read_only_spawns: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelperMemoryCandidate {
    pub agent_run_id: AgentRunId,
    pub read_only: bool,
    pub priority: i32,
    pub memory_pressure_score: u64,
    pub started_at_unix_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelperVictimSelection {
    pub selected: Option<AgentRunId>,
    pub skipped: Vec<SkippedHelperCandidate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedHelperCandidate {
    pub agent_run_id: AgentRunId,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunResourceGovernor {
    policy: MemoryGovernorPolicy,
    state: MemoryPressureState,
    pressure_samples: u32,
    clear_samples: u32,
    spawn_paused: bool,
}

impl RunResourceGovernor {
    pub fn new(policy: MemoryGovernorPolicy) -> Self {
        Self {
            policy,
            state: MemoryPressureState::Normal,
            pressure_samples: 0,
            clear_samples: 0,
            spawn_paused: false,
        }
    }

    pub fn state(&self) -> MemoryPressureState {
        self.state
    }

    pub fn spawn_paused(&self) -> bool {
        self.spawn_paused
    }

    pub fn observe(
        &mut self,
        snapshot: &MemoryPressureSnapshot,
        helpers: &[HelperMemoryCandidate],
    ) -> MemoryPressureTransition {
        let previous = self.state;
        if snapshot.has_terminal_oom() {
            self.state = MemoryPressureState::Terminal;
            self.spawn_paused = true;
            let mut actions = Vec::new();
            if let Some(group_id) = snapshot.group_id.clone() {
                actions.push(MemoryAction::KillGroup {
                    group_id,
                    terminal_cleanup: true,
                });
            }
            actions.push(MemoryAction::FailRun {
                reason: "memory cgroup reported oom or oom_kill".to_string(),
            });
            return MemoryPressureTransition::new(
                previous,
                self.state,
                actions,
                "terminal memory event",
            );
        }

        if snapshot.has_pressure() {
            self.pressure_samples = self.pressure_samples.saturating_add(1);
            self.clear_samples = 0;
            if self.pressure_samples >= self.policy.sustained_pressure_samples {
                self.state = MemoryPressureState::PressureSustained;
                let mut actions = Vec::new();
                if self.policy.pause_read_only_spawns && !self.spawn_paused {
                    self.spawn_paused = true;
                    actions.push(MemoryAction::PauseReadOnlySpawns);
                }
                if self.policy.request_runtime_compaction {
                    actions.push(MemoryAction::RequestRuntimeMitigation(
                        RuntimeMemoryMitigation::ContextCompactionRequested,
                    ));
                }
                if let Some(victim) = select_helper_victim(helpers).selected {
                    actions.push(MemoryAction::CancelReadOnlyHelper {
                        agent_run_id: victim,
                        reason: "sustained memory pressure".to_string(),
                    });
                } else {
                    actions.push(MemoryAction::FailRun {
                        reason: "sustained memory pressure with no cancellable read-only helper"
                            .to_string(),
                    });
                }
                return MemoryPressureTransition::new(
                    previous,
                    self.state,
                    actions,
                    "sustained memory pressure",
                );
            }
            self.state = MemoryPressureState::PressureObserved;
            return MemoryPressureTransition::new(
                previous,
                self.state,
                vec![MemoryAction::Observe],
                "memory pressure observed",
            );
        }

        self.pressure_samples = 0;
        self.clear_samples = self.clear_samples.saturating_add(1);
        if self.spawn_paused && self.clear_samples >= self.policy.clear_pressure_samples {
            self.spawn_paused = false;
            self.state = MemoryPressureState::SpawnPauseReleased;
            return MemoryPressureTransition::new(
                previous,
                self.state,
                vec![MemoryAction::ReleaseSpawnPause],
                "memory pressure cleared and spawn pause released",
            );
        }
        if self.state != MemoryPressureState::Normal {
            self.state = MemoryPressureState::PressureCleared;
            return MemoryPressureTransition::new(
                previous,
                self.state,
                vec![MemoryAction::Observe],
                "memory pressure cleared",
            );
        }
        MemoryPressureTransition::new(previous, self.state, Vec::new(), "memory normal")
    }
}

pub fn select_helper_victim(helpers: &[HelperMemoryCandidate]) -> HelperVictimSelection {
    let mut skipped = Vec::new();
    let mut candidates = Vec::new();
    for helper in helpers {
        if helper.read_only {
            candidates.push(helper);
        } else {
            skipped.push(SkippedHelperCandidate {
                agent_run_id: helper.agent_run_id.clone(),
                reason: "writer or write-capable helper is not cancellable".to_string(),
            });
        }
    }
    candidates.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| right.memory_pressure_score.cmp(&left.memory_pressure_score))
            .then_with(|| right.started_at_unix_ms.cmp(&left.started_at_unix_ms))
            .then_with(|| left.agent_run_id.0.cmp(&right.agent_run_id.0))
    });
    HelperVictimSelection {
        selected: candidates.first().map(|helper| helper.agent_run_id.clone()),
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(high: u64, oom: u64) -> MemoryPressureSnapshot {
        MemoryPressureSnapshot {
            memory_events_high_delta: high,
            memory_events_oom_delta: oom,
            sampled_at_unix_ms: 1,
            ..MemoryPressureSnapshot::default()
        }
    }

    #[test]
    fn releases_spawn_pause_after_cooldown() {
        let mut governor = RunResourceGovernor::new(MemoryGovernorPolicy {
            sustained_pressure_samples: 1,
            clear_pressure_samples: 2,
            ..MemoryGovernorPolicy::default()
        });
        assert_eq!(
            governor.observe(&snapshot(1, 0), &[]).next,
            MemoryPressureState::PressureSustained
        );
        assert!(governor.spawn_paused());
        assert_eq!(
            governor.observe(&snapshot(0, 0), &[]).next,
            MemoryPressureState::PressureCleared
        );
        assert!(governor.spawn_paused());
        let transition = governor.observe(&snapshot(0, 0), &[]);
        assert_eq!(transition.next, MemoryPressureState::SpawnPauseReleased);
        assert_eq!(transition.actions, vec![MemoryAction::ReleaseSpawnPause]);
        assert!(!governor.spawn_paused());
    }

    #[test]
    fn terminal_oom_fails_run() {
        let mut governor = RunResourceGovernor::new(MemoryGovernorPolicy::default());
        let mut sample = snapshot(0, 1);
        sample.group_id = Some(agentactr_core::MemoryGroupId::new("run:run-1:agent:writer"));
        let transition = governor.observe(&sample, &[]);
        assert_eq!(transition.next, MemoryPressureState::Terminal);
        assert!(matches!(
            transition.actions.first(),
            Some(MemoryAction::KillGroup {
                terminal_cleanup: true,
                ..
            })
        ));
        assert!(matches!(
            transition.actions.get(1),
            Some(MemoryAction::FailRun { .. })
        ));
    }

    #[test]
    fn sustained_pressure_without_helper_fails_run() {
        let mut governor = RunResourceGovernor::new(MemoryGovernorPolicy {
            sustained_pressure_samples: 1,
            ..MemoryGovernorPolicy::default()
        });
        let transition = governor.observe(&snapshot(1, 0), &[]);
        assert_eq!(transition.next, MemoryPressureState::PressureSustained);
        assert!(transition
            .actions
            .iter()
            .any(|action| matches!(action, MemoryAction::FailRun { .. })));
    }

    #[test]
    fn selects_helper_victim_deterministically() {
        let helpers = vec![
            HelperMemoryCandidate {
                agent_run_id: AgentRunId::new("writer"),
                read_only: false,
                priority: 0,
                memory_pressure_score: 100,
                started_at_unix_ms: 3,
            },
            HelperMemoryCandidate {
                agent_run_id: AgentRunId::new("helper-a"),
                read_only: true,
                priority: 5,
                memory_pressure_score: 200,
                started_at_unix_ms: 1,
            },
            HelperMemoryCandidate {
                agent_run_id: AgentRunId::new("helper-b"),
                read_only: true,
                priority: 1,
                memory_pressure_score: 10,
                started_at_unix_ms: 2,
            },
        ];
        let selection = select_helper_victim(&helpers);
        assert_eq!(selection.selected, Some(AgentRunId::new("helper-b")));
        assert_eq!(selection.skipped.len(), 1);
    }
}
