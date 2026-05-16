use crate::MemoryGroupId;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(RunId);
string_id!(AgentRunId);
string_id!(RuntimeKind);
string_id!(RuntimeTransportKind);
string_id!(ContainerRef);
string_id!(VmRef);
string_id!(RuntimeIdentityRef);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProcessId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProcessGroupId(pub i64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeProcessModel {
    OneShotProcess,
    PerRunServer,
    PerAgentServer,
    SharedServer,
    RemoteSession,
}

impl RuntimeProcessModel {
    pub fn is_shared(self) -> bool {
        matches!(self, Self::SharedServer)
    }

    pub fn is_remote(self) -> bool {
        matches!(self, Self::RemoteSession)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeProcessAttribution {
    pub run_id: RunId,
    pub agent_run_id: AgentRunId,
    pub parent_agent_run_id: Option<AgentRunId>,
    pub runtime_kind: RuntimeKind,
    pub transport_kind: RuntimeTransportKind,
    pub process_model: RuntimeProcessModel,
    pub root_pid: Option<ProcessId>,
    pub child_pids: Vec<ProcessId>,
    pub process_group_id: Option<ProcessGroupId>,
    pub container_ref: Option<ContainerRef>,
    pub vm_ref: Option<VmRef>,
    pub memory_group_id: Option<MemoryGroupId>,
    pub identity_ref: Option<RuntimeIdentityRef>,
}

impl RuntimeProcessAttribution {
    pub fn new(
        run_id: RunId,
        agent_run_id: AgentRunId,
        runtime_kind: RuntimeKind,
        transport_kind: RuntimeTransportKind,
        process_model: RuntimeProcessModel,
    ) -> Self {
        Self {
            run_id,
            agent_run_id,
            parent_agent_run_id: None,
            runtime_kind,
            transport_kind,
            process_model,
            root_pid: None,
            child_pids: Vec::new(),
            process_group_id: None,
            container_ref: None,
            vm_ref: None,
            memory_group_id: None,
            identity_ref: None,
        }
    }

    pub fn with_parent_agent_run_id(mut self, parent_agent_run_id: AgentRunId) -> Self {
        self.parent_agent_run_id = Some(parent_agent_run_id);
        self
    }

    pub fn with_root_pid(mut self, pid: ProcessId) -> Self {
        self.root_pid = Some(pid);
        self
    }

    pub fn with_child_pid(mut self, pid: ProcessId) -> Self {
        if !self.child_pids.contains(&pid) {
            self.child_pids.push(pid);
        }
        self
    }

    pub fn with_process_group_id(mut self, process_group_id: ProcessGroupId) -> Self {
        self.process_group_id = Some(process_group_id);
        self
    }

    pub fn with_container_ref(mut self, container_ref: ContainerRef) -> Self {
        self.container_ref = Some(container_ref);
        self
    }

    pub fn with_vm_ref(mut self, vm_ref: VmRef) -> Self {
        self.vm_ref = Some(vm_ref);
        self
    }

    pub fn with_memory_group_id(mut self, memory_group_id: MemoryGroupId) -> Self {
        self.memory_group_id = Some(memory_group_id);
        self
    }

    pub fn with_identity_ref(mut self, identity_ref: RuntimeIdentityRef) -> Self {
        self.identity_ref = Some(identity_ref);
        self
    }

    pub fn is_shared_server(&self) -> bool {
        self.process_model.is_shared()
    }

    pub fn is_remote_session(&self) -> bool {
        self.process_model.is_remote()
    }

    pub fn has_local_process_anchor(&self) -> bool {
        self.root_pid.is_some() || self.process_group_id.is_some() || !self.child_pids.is_empty()
    }

    pub fn has_backend_anchor(&self) -> bool {
        self.has_local_process_anchor() || self.container_ref.is_some() || self.vm_ref.is_some()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeProcessEventKind {
    Started,
    Attributed,
    ChildDiscovered,
    Terminated,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeProcessEvent {
    pub run_id: RunId,
    pub agent_run_id: AgentRunId,
    pub kind: RuntimeProcessEventKind,
    pub attribution: RuntimeProcessAttribution,
}

impl RuntimeProcessEvent {
    pub fn new(kind: RuntimeProcessEventKind, attribution: RuntimeProcessAttribution) -> Self {
        Self {
            run_id: attribution.run_id.clone(),
            agent_run_id: attribution.agent_run_id.clone(),
            kind,
            attribution,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribution(model: RuntimeProcessModel) -> RuntimeProcessAttribution {
        RuntimeProcessAttribution::new(
            RunId::new("run-1"),
            AgentRunId::new("agent-1"),
            RuntimeKind::new("codex"),
            RuntimeTransportKind::new("exec_json"),
            model,
        )
    }

    #[test]
    fn records_process_and_memory_anchors() {
        let attr = attribution(RuntimeProcessModel::OneShotProcess)
            .with_root_pid(ProcessId(123))
            .with_child_pid(ProcessId(456))
            .with_child_pid(ProcessId(456))
            .with_process_group_id(ProcessGroupId(123))
            .with_memory_group_id(MemoryGroupId::new("cg-run-1"));

        assert!(attr.has_local_process_anchor());
        assert!(attr.has_backend_anchor());
        assert_eq!(attr.child_pids, vec![ProcessId(456)]);
        assert_eq!(
            attr.memory_group_id.as_ref().map(MemoryGroupId::as_str),
            Some("cg-run-1")
        );
    }

    #[test]
    fn distinguishes_shared_and_remote_process_models() {
        assert!(attribution(RuntimeProcessModel::SharedServer).is_shared_server());
        assert!(attribution(RuntimeProcessModel::RemoteSession).is_remote_session());
        assert!(!attribution(RuntimeProcessModel::PerAgentServer).is_shared_server());
    }

    #[test]
    fn process_event_copies_correlation_ids() {
        let attr = attribution(RuntimeProcessModel::PerRunServer)
            .with_parent_agent_run_id(AgentRunId::new("agent-parent"));
        let event = RuntimeProcessEvent::new(RuntimeProcessEventKind::Attributed, attr);

        assert_eq!(event.run_id.as_str(), "run-1");
        assert_eq!(event.agent_run_id.as_str(), "agent-1");
        assert_eq!(
            event
                .attribution
                .parent_agent_run_id
                .as_ref()
                .map(AgentRunId::as_str),
            Some("agent-parent")
        );
        assert_eq!(event.kind, RuntimeProcessEventKind::Attributed);
    }
}
