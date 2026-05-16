#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainProfile {
    pub id: String,
    pub kind: String,
    pub confidence: u8,
    pub evidence: Vec<DomainEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainEvidence {
    pub path: String,
    pub signal: String,
    pub weight: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainQualityGate {
    pub name: String,
    pub domain: String,
    pub tool: String,
    pub command: Option<String>,
    pub required: bool,
    pub mutates: bool,
    pub network_required: bool,
    pub credential_required: bool,
    pub opt_in_required: bool,
    pub degraded_if_missing: bool,
    pub artifact_paths: Vec<String>,
    pub setup_guidance: Vec<String>,
    pub failure_policy: String,
}

impl DomainQualityGate {
    pub fn command_gate(
        name: impl Into<String>,
        domain: impl Into<String>,
        tool: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            domain: domain.into(),
            tool: tool.into(),
            command: Some(command.into()),
            required: true,
            mutates: false,
            network_required: false,
            credential_required: false,
            opt_in_required: false,
            degraded_if_missing: false,
            artifact_paths: Vec::new(),
            setup_guidance: Vec::new(),
            failure_policy: "fail_closed".to_string(),
        }
    }

    pub fn finding_gate(
        name: impl Into<String>,
        domain: impl Into<String>,
        tool: impl Into<String>,
        guidance: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            domain: domain.into(),
            tool: tool.into(),
            command: None,
            required: true,
            mutates: false,
            network_required: false,
            credential_required: false,
            opt_in_required: false,
            degraded_if_missing: false,
            artifact_paths: Vec::new(),
            setup_guidance: vec![guidance.into()],
            failure_policy: "finding_only".to_string(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainGraph {
    pub schema_version: String,
    pub artifact_format_version: String,
    pub producer: String,
    pub created_at: String,
    pub repo: String,
    pub nodes: Vec<DomainGraphNode>,
    pub edges: Vec<DomainGraphEdge>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainGraphNode {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub artifact_refs: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainGraphEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainFinding {
    pub id: String,
    pub domain: String,
    pub severity: String,
    pub title: String,
    pub message: String,
    pub evidence_paths: Vec<String>,
    pub remediation: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorRegistryProfile {
    pub domain: String,
    pub registry_path: Option<String>,
    pub required_fields: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiContractProfile {
    pub id: String,
    pub kind: String,
    pub packages: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtobufSchemaProfile {
    pub packages: Vec<String>,
    pub files: Vec<String>,
    pub buf_configured: bool,
    pub buf_lock_present: bool,
    pub plugin_config_paths: Vec<String>,
    pub plugin_version_pins_present: bool,
    pub generated_artifacts: Vec<GeneratedArtifactProfile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcProfile {
    pub kind: String,
    pub services: Vec<String>,
    pub streaming_methods: Vec<String>,
    pub gateway_annotations: bool,
    pub connect_markers: bool,
    pub openapi_annotations: bool,
    pub health_or_reflection: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedArtifactProfile {
    pub path: String,
    pub language: String,
    pub generator: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaEvolutionFinding {
    pub domain: String,
    pub severity: String,
    pub message: String,
    pub evidence_paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcSurfaceFinding {
    pub severity: String,
    pub service: String,
    pub message: String,
    pub evidence_paths: Vec<String>,
}
