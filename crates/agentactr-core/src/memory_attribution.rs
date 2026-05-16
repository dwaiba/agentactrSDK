use crate::{RuntimeProcessAttribution, RuntimeProcessModel};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryBackend {
    Auto,
    NativeLinuxCgroupV2,
    NativeMacosObserve,
    DockerLinuxVm,
    ObserveOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryEnforcementClaim {
    Strict,
    DegradedObserveOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryAttributionFailure {
    SharedServerNotProven,
    MissingMemoryGroup,
    MissingProcessAnchor,
    MissingContainerOrVmAnchor,
    RemoteSessionCannotUseLocalCgroup,
    StrictUnavailableOnNativeMacos,
    StrictUnavailableOnObserveOnly,
}

impl MemoryAttributionFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SharedServerNotProven => {
                "shared server memory attribution is not proven for this run"
            }
            Self::MissingMemoryGroup => "strict memory attribution requires a memory group",
            Self::MissingProcessAnchor => "strict memory attribution requires a process anchor",
            Self::MissingContainerOrVmAnchor => {
                "Docker/Linux VM memory attribution requires a container or VM anchor"
            }
            Self::RemoteSessionCannotUseLocalCgroup => {
                "remote sessions cannot be enforced with local cgroups"
            }
            Self::StrictUnavailableOnNativeMacos => {
                "native macOS supports observation only, not strict cgroup enforcement"
            }
            Self::StrictUnavailableOnObserveOnly => {
                "observe-only memory backend cannot make a strict enforcement claim"
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryAttributionPolicy {
    pub backend: MemoryBackend,
    pub strict_required: bool,
    pub allow_shared_server: bool,
    pub require_process_anchor: bool,
}

impl Default for MemoryAttributionPolicy {
    fn default() -> Self {
        Self {
            backend: MemoryBackend::Auto,
            strict_required: false,
            allow_shared_server: false,
            require_process_anchor: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryAttributionDecision {
    pub claim: MemoryEnforcementClaim,
    pub backend: MemoryBackend,
    pub reason: String,
}

impl MemoryAttributionDecision {
    fn strict(backend: MemoryBackend, reason: impl Into<String>) -> Self {
        Self {
            claim: MemoryEnforcementClaim::Strict,
            backend,
            reason: reason.into(),
        }
    }

    fn observe(backend: MemoryBackend, reason: impl Into<String>) -> Self {
        Self {
            claim: MemoryEnforcementClaim::DegradedObserveOnly,
            backend,
            reason: reason.into(),
        }
    }
}

impl MemoryAttributionPolicy {
    pub fn validate(
        &self,
        attribution: &RuntimeProcessAttribution,
    ) -> Result<MemoryAttributionDecision, MemoryAttributionFailure> {
        if attribution.process_model == RuntimeProcessModel::SharedServer
            && !self.allow_shared_server
        {
            return Err(MemoryAttributionFailure::SharedServerNotProven);
        }

        match self.backend {
            MemoryBackend::NativeMacosObserve | MemoryBackend::ObserveOnly => {
                if self.strict_required {
                    return match self.backend {
                        MemoryBackend::NativeMacosObserve => {
                            Err(MemoryAttributionFailure::StrictUnavailableOnNativeMacos)
                        }
                        _ => Err(MemoryAttributionFailure::StrictUnavailableOnObserveOnly),
                    };
                }
                Ok(MemoryAttributionDecision::observe(
                    self.backend,
                    "backend is observe-only",
                ))
            }
            MemoryBackend::NativeLinuxCgroupV2 => self.validate_linux(attribution),
            MemoryBackend::DockerLinuxVm => self.validate_docker_linux_vm(attribution),
            MemoryBackend::Auto => self.validate_auto(attribution),
        }
    }

    fn validate_linux(
        &self,
        attribution: &RuntimeProcessAttribution,
    ) -> Result<MemoryAttributionDecision, MemoryAttributionFailure> {
        if attribution.process_model == RuntimeProcessModel::RemoteSession {
            return Err(MemoryAttributionFailure::RemoteSessionCannotUseLocalCgroup);
        }
        self.validate_group_and_process(attribution)?;
        Ok(MemoryAttributionDecision::strict(
            MemoryBackend::NativeLinuxCgroupV2,
            "memory group and process anchor are present",
        ))
    }

    fn validate_docker_linux_vm(
        &self,
        attribution: &RuntimeProcessAttribution,
    ) -> Result<MemoryAttributionDecision, MemoryAttributionFailure> {
        if attribution.container_ref.is_none() && attribution.vm_ref.is_none() {
            return Err(MemoryAttributionFailure::MissingContainerOrVmAnchor);
        }
        if attribution.memory_group_id.is_none() {
            return Err(MemoryAttributionFailure::MissingMemoryGroup);
        }
        if self.require_process_anchor && !attribution.has_backend_anchor() {
            return Err(MemoryAttributionFailure::MissingProcessAnchor);
        }
        Ok(MemoryAttributionDecision::strict(
            MemoryBackend::DockerLinuxVm,
            "container or VM anchor and memory group are present",
        ))
    }

    fn validate_auto(
        &self,
        attribution: &RuntimeProcessAttribution,
    ) -> Result<MemoryAttributionDecision, MemoryAttributionFailure> {
        let strict_possible = attribution.memory_group_id.is_some()
            && attribution.has_local_process_anchor()
            && !attribution.is_remote_session();

        if strict_possible {
            return Ok(MemoryAttributionDecision::strict(
                MemoryBackend::Auto,
                "auto backend can attribute a local process to a memory group",
            ));
        }

        if self.strict_required {
            if attribution.memory_group_id.is_none() {
                return Err(MemoryAttributionFailure::MissingMemoryGroup);
            }
            if self.require_process_anchor && !attribution.has_local_process_anchor() {
                return Err(MemoryAttributionFailure::MissingProcessAnchor);
            }
            if attribution.is_remote_session() {
                return Err(MemoryAttributionFailure::RemoteSessionCannotUseLocalCgroup);
            }
        }

        Ok(MemoryAttributionDecision::observe(
            MemoryBackend::Auto,
            "auto backend cannot prove strict memory attribution",
        ))
    }

    fn validate_group_and_process(
        &self,
        attribution: &RuntimeProcessAttribution,
    ) -> Result<(), MemoryAttributionFailure> {
        if attribution.memory_group_id.is_none() {
            return Err(MemoryAttributionFailure::MissingMemoryGroup);
        }
        if self.require_process_anchor && !attribution.has_local_process_anchor() {
            return Err(MemoryAttributionFailure::MissingProcessAnchor);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentRunId, ContainerRef, MemoryGroupId, ProcessId, RunId, RuntimeKind,
        RuntimeProcessAttribution, RuntimeProcessModel, RuntimeTransportKind,
    };

    fn attr(model: RuntimeProcessModel) -> RuntimeProcessAttribution {
        RuntimeProcessAttribution::new(
            RunId::new("run-1"),
            AgentRunId::new("agent-1"),
            RuntimeKind::new("codex"),
            RuntimeTransportKind::new("exec_json"),
            model,
        )
    }

    #[test]
    fn native_macos_observe_only_does_not_require_cgroup() {
        let policy = MemoryAttributionPolicy {
            backend: MemoryBackend::NativeMacosObserve,
            ..MemoryAttributionPolicy::default()
        };

        let decision = policy
            .validate(&attr(RuntimeProcessModel::OneShotProcess))
            .expect("macOS native observation should be allowed");

        assert_eq!(decision.claim, MemoryEnforcementClaim::DegradedObserveOnly);
    }

    #[test]
    fn observe_only_backend_rejects_strict_claims() {
        let policy = MemoryAttributionPolicy {
            backend: MemoryBackend::ObserveOnly,
            strict_required: true,
            ..MemoryAttributionPolicy::default()
        };

        let err = policy
            .validate(&attr(RuntimeProcessModel::OneShotProcess))
            .expect_err("observe-only backend cannot satisfy strict enforcement");

        assert_eq!(
            err,
            MemoryAttributionFailure::StrictUnavailableOnObserveOnly
        );
    }

    #[test]
    fn native_linux_strict_rejects_missing_memory_group() {
        let policy = MemoryAttributionPolicy {
            backend: MemoryBackend::NativeLinuxCgroupV2,
            strict_required: true,
            ..MemoryAttributionPolicy::default()
        };

        let err = policy
            .validate(&attr(RuntimeProcessModel::OneShotProcess).with_root_pid(ProcessId(12)))
            .expect_err("strict Linux attribution requires a memory group");

        assert_eq!(err, MemoryAttributionFailure::MissingMemoryGroup);
    }

    #[test]
    fn native_linux_strict_accepts_group_and_process_anchor() {
        let policy = MemoryAttributionPolicy {
            backend: MemoryBackend::NativeLinuxCgroupV2,
            strict_required: true,
            ..MemoryAttributionPolicy::default()
        };
        let attribution = attr(RuntimeProcessModel::PerAgentServer)
            .with_root_pid(ProcessId(12))
            .with_memory_group_id(MemoryGroupId::new("agentactr/run-1/agent-1"));

        let decision = policy
            .validate(&attribution)
            .expect("strict Linux attribution should be valid");

        assert_eq!(decision.claim, MemoryEnforcementClaim::Strict);
    }

    #[test]
    fn docker_linux_vm_requires_backend_anchor() {
        let policy = MemoryAttributionPolicy {
            backend: MemoryBackend::DockerLinuxVm,
            strict_required: true,
            ..MemoryAttributionPolicy::default()
        };

        let err = policy
            .validate(&attr(RuntimeProcessModel::PerRunServer))
            .expect_err("Docker/Linux VM attribution requires a backend anchor");

        assert_eq!(err, MemoryAttributionFailure::MissingContainerOrVmAnchor);
    }

    #[test]
    fn docker_linux_vm_accepts_container_and_memory_group() {
        let policy = MemoryAttributionPolicy {
            backend: MemoryBackend::DockerLinuxVm,
            strict_required: true,
            ..MemoryAttributionPolicy::default()
        };
        let attribution = attr(RuntimeProcessModel::PerRunServer)
            .with_container_ref(ContainerRef::new("docker://agentactr-run-1"))
            .with_memory_group_id(MemoryGroupId::new("docker-cgroup/run-1"));

        let decision = policy
            .validate(&attribution)
            .expect("container-backed attribution should be valid");

        assert_eq!(decision.claim, MemoryEnforcementClaim::Strict);
    }
}
