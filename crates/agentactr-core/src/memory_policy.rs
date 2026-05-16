use crate::{AgentRunId, MemoryGroupId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryPressureState {
    Normal,
    PressureObserved,
    PressureSustained,
    Remediation,
    PressureCleared,
    SpawnPauseReleased,
    Terminal,
}

impl MemoryPressureState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::PressureObserved => "pressure_observed",
            Self::PressureSustained => "pressure_sustained",
            Self::Remediation => "remediation",
            Self::PressureCleared => "pressure_cleared",
            Self::SpawnPauseReleased => "spawn_pause_released",
            Self::Terminal => "terminal",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryPressureSnapshot {
    pub group_id: Option<MemoryGroupId>,
    pub agent_run_id: Option<AgentRunId>,
    pub memory_current_bytes: Option<u64>,
    pub memory_peak_bytes: Option<u64>,
    pub memory_events_high_delta: u64,
    pub memory_events_oom_delta: u64,
    pub memory_events_oom_kill_delta: u64,
    pub psi_some_total_delta_us: u64,
    pub psi_full_total_delta_us: u64,
    pub sampled_at_unix_ms: u64,
}

impl MemoryPressureSnapshot {
    pub fn has_pressure(&self) -> bool {
        self.memory_events_high_delta > 0
            || self.psi_some_total_delta_us > 0
            || self.psi_full_total_delta_us > 0
    }

    pub fn has_terminal_oom(&self) -> bool {
        self.memory_events_oom_delta > 0 || self.memory_events_oom_kill_delta > 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeMemoryMitigation {
    ContextCompactionRequested,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryAction {
    Observe,
    PauseReadOnlySpawns,
    ReleaseSpawnPause,
    RequestRuntimeMitigation(RuntimeMemoryMitigation),
    CancelReadOnlyHelper {
        agent_run_id: AgentRunId,
        reason: String,
    },
    Reclaim {
        group_id: MemoryGroupId,
        bytes: u64,
    },
    KillGroup {
        group_id: MemoryGroupId,
        terminal_cleanup: bool,
    },
    FailRun {
        reason: String,
    },
}

impl MemoryAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::PauseReadOnlySpawns => "pause_read_only_spawns",
            Self::ReleaseSpawnPause => "release_spawn_pause",
            Self::RequestRuntimeMitigation(RuntimeMemoryMitigation::ContextCompactionRequested) => {
                "request_context_compaction"
            }
            Self::CancelReadOnlyHelper { .. } => "cancel_read_only_helper",
            Self::Reclaim { .. } => "reclaim",
            Self::KillGroup { .. } => "kill_group",
            Self::FailRun { .. } => "fail_run",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryActionResult {
    pub action: MemoryAction,
    pub supported: bool,
    pub succeeded: bool,
    pub detail: String,
}

impl MemoryActionResult {
    pub fn succeeded(action: MemoryAction, detail: impl Into<String>) -> Self {
        Self {
            action,
            supported: true,
            succeeded: true,
            detail: detail.into(),
        }
    }

    pub fn degraded(action: MemoryAction, detail: impl Into<String>) -> Self {
        Self {
            action,
            supported: false,
            succeeded: false,
            detail: detail.into(),
        }
    }

    pub fn failed(action: MemoryAction, detail: impl Into<String>) -> Self {
        Self {
            action,
            supported: true,
            succeeded: false,
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryPressureTransition {
    pub previous: MemoryPressureState,
    pub next: MemoryPressureState,
    pub actions: Vec<MemoryAction>,
    pub reason: String,
}

impl MemoryPressureTransition {
    pub fn new(
        previous: MemoryPressureState,
        next: MemoryPressureState,
        actions: Vec<MemoryAction>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            previous,
            next,
            actions,
            reason: reason.into(),
        }
    }
}
