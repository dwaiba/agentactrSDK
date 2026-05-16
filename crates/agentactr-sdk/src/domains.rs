use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use agentactr_core::{
    AgentactrConfig, DomainEvidence, DomainFinding, DomainGraph, DomainGraphEdge, DomainGraphNode,
    DomainProfile, DomainQualityGate, GeneratedArtifactProfile, ProtobufSchemaProfile, RpcProfile,
};
use serde_json::{json, Value};

pub const DOMAIN_GRAPH_SCHEMA_VERSION: &str = "0.1";
pub const DOMAIN_GRAPH_ARTIFACT_FORMAT_VERSION: &str = "0.1";

pub fn detect_domain_profiles(root: &Path) -> Vec<DomainProfile> {
    detect_auto_domain_profiles(root)
}

pub fn detect_domain_profiles_with_config(
    root: &Path,
    config: &AgentactrConfig,
) -> Vec<DomainProfile> {
    compose_domain_profiles(root, &config.architecture.domains)
}

fn detect_auto_domain_profiles(root: &Path) -> Vec<DomainProfile> {
    let files = collect_repo_files(root);
    let mut evidence_by_domain: BTreeMap<(&'static str, &'static str), Vec<DomainEvidence>> =
        BTreeMap::new();

    for file in &files {
        let name = file_name(file);
        let lower = file.to_ascii_lowercase();
        let signals = domain_signals(file, name.as_deref(), &lower);
        for (id, kind, signal, weight) in signals {
            evidence_by_domain
                .entry((id, kind))
                .or_default()
                .push(DomainEvidence {
                    path: file.clone(),
                    signal: signal.to_string(),
                    weight,
                });
        }
    }

    evidence_by_domain
        .into_iter()
        .map(|((id, kind), mut evidence)| {
            evidence.sort_by(|a, b| a.path.cmp(&b.path).then(a.signal.cmp(&b.signal)));
            evidence.dedup_by(|a, b| a.path == b.path && a.signal == b.signal);
            let score = evidence
                .iter()
                .map(|item| item.weight)
                .sum::<u16>()
                .min(100) as u8;
            DomainProfile {
                id: id.to_string(),
                kind: kind.to_string(),
                confidence: score.max(50),
                evidence,
            }
        })
        .collect()
}

pub fn domain_quality_plan(root: &Path) -> Vec<DomainQualityGate> {
    domain_quality_plan_for_domains(root, &["auto".to_string()])
}

pub fn domain_quality_plan_with_config(
    root: &Path,
    config: &AgentactrConfig,
) -> Vec<DomainQualityGate> {
    domain_quality_plan_for_domains(root, &config.quality.domains)
}

fn domain_quality_plan_for_domains(root: &Path, domains: &[String]) -> Vec<DomainQualityGate> {
    let files = collect_repo_files(root);
    let profiles = compose_domain_profiles(root, domains);
    let mut gates = Vec::new();
    for profile in profiles {
        gates.extend(domain_gates_for(&profile, &files));
    }
    gates.sort_by(|a, b| a.domain.cmp(&b.domain).then(a.name.cmp(&b.name)));
    gates.dedup_by(|a, b| a.domain == b.domain && a.name == b.name);
    gates
}

pub fn build_domain_graph(root: &Path, repo: impl Into<String>) -> DomainGraph {
    build_domain_graph_from_parts(
        root,
        repo.into(),
        detect_domain_profiles(root),
        domain_quality_plan(root),
    )
}

pub fn build_domain_graph_with_config(
    root: &Path,
    repo: impl Into<String>,
    config: &AgentactrConfig,
) -> DomainGraph {
    build_domain_graph_from_parts(
        root,
        repo.into(),
        detect_domain_profiles_with_config(root, config),
        domain_quality_plan_with_config(root, config),
    )
}

fn build_domain_graph_from_parts(
    root: &Path,
    repo: String,
    profiles: Vec<DomainProfile>,
    gates: Vec<DomainQualityGate>,
) -> DomainGraph {
    let files = collect_repo_files(root);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    nodes.push(DomainGraphNode {
        id: "repo:root".to_string(),
        kind: "repo".to_string(),
        label: repo.clone(),
        artifact_refs: Vec::new(),
    });

    add_repository_module_graph_nodes(root, &files, &mut nodes, &mut edges);

    for profile in &profiles {
        let node_id = format!("domain:{}", profile.id);
        nodes.push(DomainGraphNode {
            id: node_id.clone(),
            kind: profile.kind.clone(),
            label: profile.id.clone(),
            artifact_refs: profile
                .evidence
                .iter()
                .map(|item| item.path.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        });
        edges.push(DomainGraphEdge {
            from: "repo:root".to_string(),
            to: node_id,
            kind: "has_domain".to_string(),
        });
    }

    add_domain_specific_graph_nodes(root, &files, &profiles, &mut nodes, &mut edges);
    add_finding_graph_nodes(root, &mut nodes, &mut edges);

    for gate in &gates {
        let node_id = format!("quality_gate:{}:{}", gate.domain, gate.name);
        nodes.push(DomainGraphNode {
            id: node_id.clone(),
            kind: "quality_gate".to_string(),
            label: gate.name.clone(),
            artifact_refs: gate.artifact_paths.clone(),
        });
        edges.push(DomainGraphEdge {
            from: format!("domain:{}", gate.domain),
            to: node_id,
            kind: "covered_by_gate".to_string(),
        });
    }
    sort_graph(&mut nodes, &mut edges);

    DomainGraph {
        schema_version: DOMAIN_GRAPH_SCHEMA_VERSION.to_string(),
        artifact_format_version: DOMAIN_GRAPH_ARTIFACT_FORMAT_VERSION.to_string(),
        producer: "agentactr-sdk".to_string(),
        created_at: current_epoch_millis().to_string(),
        repo,
        nodes,
        edges,
    }
}

pub fn domain_graph_to_json(graph: &DomainGraph) -> Value {
    let detected_domains = graph
        .nodes
        .iter()
        .filter(|node| node.id.starts_with("domain:"))
        .map(|node| node.label.clone())
        .collect::<Vec<_>>();
    let evidence_references = graph
        .nodes
        .iter()
        .flat_map(|node| node.artifact_refs.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    json!({
        "schema_version": graph.schema_version,
        "artifact_format_version": graph.artifact_format_version,
        "producer": graph.producer,
        "created_at": graph.created_at,
        "repo": graph.repo,
        "detected_domains": detected_domains,
        "evidence_references": evidence_references,
        "nodes": graph.nodes.iter().map(|node| json!({
            "id": node.id,
            "kind": node.kind,
            "label": node.label,
            "artifact_refs": node.artifact_refs,
        })).collect::<Vec<_>>(),
        "edges": graph.edges.iter().map(|edge| json!({
            "from": edge.from,
            "to": edge.to,
            "kind": edge.kind,
        })).collect::<Vec<_>>(),
    })
}

pub fn domain_profiles_to_json(profiles: &[DomainProfile]) -> Value {
    json!(profiles
        .iter()
        .map(|profile| json!({
            "id": profile.id,
            "kind": profile.kind,
            "confidence": profile.confidence,
            "evidence": profile.evidence.iter().map(|evidence| json!({
                "path": evidence.path,
                "signal": evidence.signal,
                "weight": evidence.weight,
            })).collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>())
}

pub fn domain_findings(root: &Path) -> Vec<DomainFinding> {
    let mut findings = Vec::new();
    if let Some(profile) = protobuf_profile(root) {
        findings.extend(protobuf_findings(root, &profile));
    }
    if let Some(profile) = rpc_profile(root) {
        findings.extend(rpc_findings(root, &profile));
    }
    findings.sort_by(|a, b| a.domain.cmp(&b.domain).then(a.id.cmp(&b.id)));
    findings.dedup_by(|a, b| {
        a.domain == b.domain && a.id == b.id && a.evidence_paths == b.evidence_paths
    });
    findings
}

pub fn domain_findings_to_json(findings: &[DomainFinding]) -> Value {
    json!({
        "schema_version": DOMAIN_GRAPH_SCHEMA_VERSION,
        "artifact_format_version": DOMAIN_GRAPH_ARTIFACT_FORMAT_VERSION,
        "producer": "agentactr-sdk",
        "findings": findings.iter().map(|finding| json!({
            "id": finding.id,
            "domain": finding.domain,
            "severity": finding.severity,
            "title": finding.title,
            "message": finding.message,
            "evidence_paths": finding.evidence_paths,
            "remediation": finding.remediation,
        })).collect::<Vec<_>>(),
    })
}

fn protobuf_findings(root: &Path, profile: &ProtobufSchemaProfile) -> Vec<DomainFinding> {
    let mut findings = Vec::new();
    if !profile.buf_configured {
        findings.push(DomainFinding {
            id: "protobuf_governance_degraded".to_string(),
            domain: "api_contracts.protobuf".to_string(),
            severity: "warning".to_string(),
            title: "Protobuf governance is degraded without Buf".to_string(),
            message: "Schema files are present but buf.yaml/buf.gen.yaml/buf.lock were not detected; breaking-change and generation drift checks must be explicitly governed.".to_string(),
            evidence_paths: profile.files.clone(),
            remediation: vec![
                "add Buf configuration or pin protoc plus language plugins".to_string(),
                "record lint, breaking, format, generate, and generated drift gates".to_string(),
            ],
        });
    }
    if profile.buf_configured && !profile.buf_lock_present {
        findings.push(DomainFinding {
            id: "protobuf_buf_lock_missing".to_string(),
            domain: "api_contracts.protobuf".to_string(),
            severity: "warning".to_string(),
            title: "Buf dependency lock was not detected".to_string(),
            message: "Buf-governed protobuf repositories should commit buf.lock so dependency resolution and breaking-change inputs are replayable.".to_string(),
            evidence_paths: profile
                .files
                .iter()
                .cloned()
                .chain(profile.plugin_config_paths.iter().cloned())
                .collect(),
            remediation: vec![
                "run buf dep update when external proto dependencies are used".to_string(),
                "commit buf.lock or document why no remote proto dependencies are present".to_string(),
            ],
        });
    }
    if profile.plugin_config_paths.is_empty() {
        findings.push(DomainFinding {
            id: "protobuf_plugin_pinning_missing".to_string(),
            domain: "api_contracts.protobuf".to_string(),
            severity: "info".to_string(),
            title: "Protobuf plugin configuration was not detected".to_string(),
            message: "Generated-code drift checks require pinned generation plugin configuration such as buf.gen.yaml or an explicit protoc plugin config.".to_string(),
            evidence_paths: profile.files.clone(),
            remediation: vec![
                "add buf.gen.yaml with pinned remote/local plugins or record an equivalent protoc plugin config".to_string(),
            ],
        });
    } else if !profile.plugin_version_pins_present {
        findings.push(DomainFinding {
            id: "protobuf_plugin_version_pins_missing".to_string(),
            domain: "api_contracts.protobuf".to_string(),
            severity: "warning".to_string(),
            title: "Protobuf plugin version pins were not detected".to_string(),
            message: "Protobuf generation config was detected, but no explicit plugin version/revision pin evidence was found.".to_string(),
            evidence_paths: profile.plugin_config_paths.clone(),
            remediation: vec![
                "pin remote Buf plugins with explicit versions/revisions or pin local protoc-gen binaries in toolchain config".to_string(),
            ],
        });
    }
    for detail in proto_details(root) {
        if detail.has_message && !detail.has_reserved {
            findings.push(DomainFinding {
                id: format!("protobuf_reserved_fields_missing:{}", graph_id(&detail.file)),
                domain: "api_contracts.protobuf".to_string(),
                severity: "info".to_string(),
                title: "Proto schema has messages without reserved-field evidence".to_string(),
                message: "Deleted field numbers and names must be reserved during schema evolution; no reserved declaration was detected in this proto file.".to_string(),
                evidence_paths: vec![detail.file.clone()],
                remediation: vec![
                    "reserve deleted field numbers and names during schema evolution".to_string(),
                    "keep breaking changes behind explicit review".to_string(),
                ],
            });
        }
        for enum_value in detail.enum_zero_findings {
            findings.push(DomainFinding {
                id: format!("protobuf_enum_zero_value:{}", graph_id(&enum_value)),
                domain: "api_contracts.protobuf".to_string(),
                severity: "warning".to_string(),
                title: "Proto enum zero value is not unspecified-style".to_string(),
                message: format!(
                    "Enum zero value `{enum_value}` should use an UNSPECIFIED or UNKNOWN style name for forward-compatible defaults."
                ),
                evidence_paths: vec![detail.file.clone()],
                remediation: vec![
                    "rename zero enum values to *_UNSPECIFIED or *_UNKNOWN".to_string(),
                    "verify generated clients preserve the intended default semantics".to_string(),
                ],
            });
        }
    }
    for artifact in &profile.generated_artifacts {
        let path = artifact.path.to_ascii_lowercase();
        if path.contains("/src/")
            && !path.contains("generated")
            && !path.contains("/gen/")
            && !path.contains("/proto/")
        {
            findings.push(DomainFinding {
                id: format!("protobuf_generated_domain_mixing:{}", graph_id(&artifact.path)),
                domain: "api_contracts.protobuf".to_string(),
                severity: "warning".to_string(),
                title: "Generated protobuf artifact may be mixed with handwritten source".to_string(),
                message: "Generated transport code should live in generated-only packages/directories and be mapped at adapter boundaries.".to_string(),
                evidence_paths: vec![artifact.path.clone()],
                remediation: vec![
                    "move generated code under a generated/proto/gen package".to_string(),
                    "wrap generated DTOs and clients behind provider-neutral ports".to_string(),
                ],
            });
        }
    }
    findings
}

fn rpc_findings(root: &Path, profile: &RpcProfile) -> Vec<DomainFinding> {
    let evidence_paths = proto_details(root)
        .into_iter()
        .filter(|detail| !detail.services.is_empty())
        .map(|detail| detail.file)
        .collect::<Vec<_>>();
    let mut findings = vec![DomainFinding {
        id: "grpc_client_operational_contract".to_string(),
        domain: "rpc.grpc".to_string(),
        severity: "info".to_string(),
        title: "gRPC clients need explicit deadline, cancellation, retry, and status mapping policy"
            .to_string(),
        message: "gRPC service definitions were detected; client adapters should set deadlines, propagate cancellation, map status codes, and keep generated clients outside domain services.".to_string(),
        evidence_paths: evidence_paths.clone(),
        remediation: vec![
            "wrap generated clients behind ports/adapters".to_string(),
            "document deadline, retry/idempotency, metadata auth, and status-code mapping".to_string(),
        ],
    }];
    if !profile.health_or_reflection {
        findings.push(DomainFinding {
            id: "grpc_health_reflection_missing".to_string(),
            domain: "rpc.grpc".to_string(),
            severity: "info".to_string(),
            title: "gRPC health/reflection evidence was not detected".to_string(),
            message: "Service-facing gRPC APIs should intentionally decide health checks and reflection policy.".to_string(),
            evidence_paths: evidence_paths.clone(),
            remediation: vec![
                "add health/reflection where appropriate or record why they are intentionally disabled".to_string(),
            ],
        });
    }
    if !profile.connect_markers {
        findings.push(DomainFinding {
            id: "grpc_connect_policy_missing".to_string(),
            domain: "rpc.grpc".to_string(),
            severity: "info".to_string(),
            title: "Connect RPC compatibility policy was not detected".to_string(),
            message: "gRPC APIs that may be exposed through Connect should explicitly document whether Connect, Connect-Web, or Connect-ES generation is supported.".to_string(),
            evidence_paths: evidence_paths.clone(),
            remediation: vec![
                "add Connect generator/config evidence or record that Connect is intentionally unsupported".to_string(),
            ],
        });
    }
    if !profile.openapi_annotations && !profile.gateway_annotations {
        findings.push(DomainFinding {
            id: "grpc_openapi_contract_missing".to_string(),
            domain: "rpc.grpc".to_string(),
            severity: "info".to_string(),
            title: "OpenAPI/gateway annotation policy was not detected".to_string(),
            message: "Public RPC surfaces should intentionally decide grpc-gateway/OpenAPI annotation and documentation generation policy.".to_string(),
            evidence_paths: evidence_paths.clone(),
            remediation: vec![
                "add OpenAPI/gateway annotations or document why generated HTTP/OpenAPI contracts are intentionally absent".to_string(),
            ],
        });
    }
    if !profile.streaming_methods.is_empty() {
        findings.push(DomainFinding {
            id: "grpc_streaming_operational_contract".to_string(),
            domain: "rpc.grpc".to_string(),
            severity: "warning".to_string(),
            title: "Streaming RPCs need backpressure and replay/runbook guidance".to_string(),
            message: "Streaming RPC methods were detected; backpressure, cancellation, replay, and operational runbook behavior must be explicit.".to_string(),
            evidence_paths,
            remediation: vec![
                "document stream backpressure and cancellation behavior".to_string(),
                "define replay/idempotency and failure recovery semantics".to_string(),
            ],
        });
    }
    findings
}

pub fn domain_quality_plan_to_json(gates: &[DomainQualityGate]) -> Value {
    json!(gates
        .iter()
        .map(|gate| json!({
            "name": gate.name,
            "domain": gate.domain,
            "tool": gate.tool,
            "command": gate.command,
            "required": gate.required,
            "mutates": gate.mutates,
            "network_required": gate.network_required,
            "credential_required": gate.credential_required,
            "opt_in_required": gate.opt_in_required,
            "degraded_if_missing": gate.degraded_if_missing,
            "artifact_paths": gate.artifact_paths,
            "setup_guidance": gate.setup_guidance,
            "failure_policy": gate.failure_policy,
        }))
        .collect::<Vec<_>>())
}

pub fn protobuf_profile(root: &Path) -> Option<ProtobufSchemaProfile> {
    let files = collect_repo_files(root);
    let proto_files = files
        .iter()
        .filter(|file| file.ends_with(".proto"))
        .cloned()
        .collect::<Vec<_>>();
    if proto_files.is_empty() {
        return None;
    }
    let packages = proto_files
        .iter()
        .filter_map(|file| read_proto_package(&root.join(file)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let generated_artifacts = files
        .iter()
        .filter_map(|file| generated_artifact_profile(file))
        .collect::<Vec<_>>();
    let plugin_config_paths = protobuf_plugin_config_paths(root, &files);
    let plugin_version_pins_present =
        protobuf_plugin_version_pins_present(root, &plugin_config_paths);
    Some(ProtobufSchemaProfile {
        packages,
        files: proto_files,
        buf_configured: any_file_name(&files, &["buf.yaml", "buf.gen.yaml", "buf.lock"]),
        buf_lock_present: any_file_name(&files, &["buf.lock"]),
        plugin_config_paths,
        plugin_version_pins_present,
        generated_artifacts,
    })
}

pub fn rpc_profile(root: &Path) -> Option<RpcProfile> {
    let files = collect_repo_files(root);
    let proto_files = files
        .iter()
        .filter(|file| file.ends_with(".proto"))
        .collect::<Vec<_>>();
    let mut services = BTreeSet::new();
    let mut streaming_methods = BTreeSet::new();
    let mut gateway_annotations = false;
    let mut connect_markers = files.iter().any(|file| {
        let lower = file.to_ascii_lowercase();
        lower.contains("connectrpc")
            || lower.contains("connect-web")
            || lower.contains("connect-es")
            || lower.contains("connect-go")
    });
    let mut openapi_annotations = files.iter().any(|file| {
        let lower = file.to_ascii_lowercase();
        lower.contains("openapi") || lower.contains("openapiv2") || lower.contains("swagger")
    });
    let mut health_or_reflection = false;
    for file in proto_files {
        let Ok(content) = fs::read_to_string(root.join(file)) else {
            continue;
        };
        if content.contains("google.api.http") || content.contains("grpc.gateway") {
            gateway_annotations = true;
        }
        if content.contains("connectrpc")
            || content.contains("connect.")
            || content.contains("buf.build/connect")
        {
            connect_markers = true;
        }
        if content.contains("openapiv2")
            || content.contains("openapi")
            || content.contains("protoc-gen-openapiv2")
            || content.contains("grpc.gateway.protoc_gen_openapiv2")
        {
            openapi_annotations = true;
        }
        if content.contains("grpc.health") || content.contains("grpc.reflection") {
            health_or_reflection = true;
        }
        for line in content.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("service ") {
                let name = rest
                    .split(|ch: char| ch.is_whitespace() || ch == '{')
                    .next()
                    .unwrap_or_default();
                if !name.is_empty() {
                    services.insert(name.to_string());
                }
            }
            if let Some(rpc) = proto_rpc_declaration(line) {
                if rpc.contains("stream ") {
                    streaming_methods.insert(rpc);
                }
            }
        }
    }
    (!services.is_empty()).then(|| RpcProfile {
        kind: "grpc".to_string(),
        services: services.into_iter().collect(),
        streaming_methods: streaming_methods.into_iter().collect(),
        gateway_annotations,
        connect_markers,
        openapi_annotations,
        health_or_reflection,
    })
}

fn protobuf_plugin_config_paths(root: &Path, files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter(|file| protobuf_plugin_config_path(root, file))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn protobuf_plugin_config_path(root: &Path, file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    if lower.ends_with("buf.gen.yaml")
        || lower.ends_with("buf.gen.yml")
        || lower.ends_with("prototool.yaml")
        || lower.ends_with("prototool.yml")
    {
        return true;
    }
    if !(lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".json")
        || lower.ends_with(".toml")
        || lower.ends_with(".txt"))
    {
        return false;
    }
    fs::read_to_string(root.join(file))
        .map(|content| {
            let lower = content.to_ascii_lowercase();
            lower.contains("protoc-gen")
                || lower.contains("remote:")
                || (lower.contains("plugins:")
                    && (lower.contains("protobuf") || lower.contains("grpc")))
        })
        .unwrap_or(false)
}

fn protobuf_plugin_version_pins_present(root: &Path, plugin_config_paths: &[String]) -> bool {
    plugin_config_paths.iter().any(|file| {
        fs::read_to_string(root.join(file))
            .map(|content| protobuf_plugin_config_has_version_pin(&content))
            .unwrap_or(false)
    })
}

fn protobuf_plugin_config_has_version_pin(content: &str) -> bool {
    let mut in_plugin_entry = false;
    let mut plugin_reference_seen = false;
    content.lines().any(|raw_line| {
        let uncommented = strip_yaml_comment(raw_line);
        let line = uncommented.trim();
        if line.is_empty() || line.starts_with("version:") {
            return false;
        }
        if uncommented.trim_start().starts_with("- ") {
            in_plugin_entry = true;
            plugin_reference_seen = false;
        } else if uncommented
            .chars()
            .next()
            .is_some_and(|ch| !ch.is_whitespace())
        {
            in_plugin_entry = false;
            plugin_reference_seen = false;
        }
        if yaml_value_after_key(line, "revision").is_some() {
            return in_plugin_entry && plugin_reference_seen;
        }
        if let Some(value) =
            yaml_value_after_key(line, "remote").or_else(|| yaml_value_after_key(line, "plugin"))
        {
            plugin_reference_seen = true;
            return plugin_reference_has_version_pin(value);
        }
        if yaml_value_after_key(line, "path")
            .or_else(|| yaml_value_after_key(line, "local"))
            .map(local_plugin_path_has_version_pin)
            .unwrap_or(false)
        {
            return true;
        }
        false
    })
}

fn strip_yaml_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or_default()
}

fn yaml_value_after_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let trimmed = line.trim_start().strip_prefix("- ").unwrap_or(line).trim();
    let (candidate, value) = trimmed.split_once(':')?;
    (candidate.trim() == key).then(|| trim_yaml_scalar(value))
}

fn trim_yaml_scalar(value: &str) -> &str {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(',')
        .trim_end_matches(']')
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
}

fn plugin_reference_has_version_pin(value: &str) -> bool {
    let reference = trim_yaml_scalar(value)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(',');
    let Some(last_segment) = reference.rsplit('/').next() else {
        return false;
    };
    reference.contains('/')
        && last_segment.split_once(':').is_some_and(|(_, version)| {
            !version.trim().is_empty() && version.chars().any(|ch| ch.is_ascii_digit())
        })
}

fn local_plugin_path_has_version_pin(value: &str) -> bool {
    trim_yaml_scalar(value)
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '[' | ']'))
        .any(|part| {
            let token = part.trim_matches('"').trim_matches('\'');
            token.contains("@v")
                || token.contains("@=")
                || token.contains("==")
                || (token.contains("protoc-gen-") && token.contains('@'))
        })
}

fn compose_domain_profiles(root: &Path, configured_domains: &[String]) -> Vec<DomainProfile> {
    let auto_profiles = detect_auto_domain_profiles(root);
    let normalized = normalize_domain_selection(configured_domains);
    if normalized
        .iter()
        .any(|domain| domain == "disabled" || domain == "none")
    {
        return Vec::new();
    }
    let detected_only = normalized.iter().any(|domain| domain == "detected_only");
    let declared_only = normalized.iter().any(|domain| domain == "declared_only");
    let selectors = normalized
        .iter()
        .filter(|domain| {
            !matches!(
                domain.as_str(),
                "auto" | "detected_only" | "declared_only" | "disabled" | "none"
            )
        })
        .flat_map(|domain| expand_domain_selector(domain))
        .collect::<BTreeSet<_>>();

    if declared_only {
        let mut profiles = selectors
            .into_iter()
            .map(|domain| config_declared_domain_profile(&domain))
            .collect::<Vec<_>>();
        profiles.sort_by(|a, b| a.id.cmp(&b.id));
        return profiles;
    }

    if detected_only {
        return filter_profiles_by_selectors(auto_profiles, &selectors);
    }

    if normalized.is_empty() || normalized.iter().any(|domain| domain == "auto") {
        let mut profiles = auto_profiles;
        for domain in selectors {
            if !profiles.iter().any(|profile| profile.id == domain) {
                profiles.push(config_declared_domain_profile(&domain));
            }
        }
        profiles.sort_by(|a, b| a.id.cmp(&b.id));
        return profiles;
    }
    let mut profiles = Vec::new();
    for domain in selectors {
        if let Some(profile) = auto_profiles.iter().find(|profile| profile.id == domain) {
            profiles.push(profile.clone());
        } else {
            profiles.push(config_declared_domain_profile(&domain));
        }
    }
    profiles.sort_by(|a, b| a.id.cmp(&b.id));
    profiles
}

fn normalize_domain_selection(configured_domains: &[String]) -> Vec<String> {
    configured_domains
        .iter()
        .map(|domain| domain.trim().to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn filter_profiles_by_selectors(
    mut profiles: Vec<DomainProfile>,
    selectors: &BTreeSet<String>,
) -> Vec<DomainProfile> {
    if selectors.is_empty() {
        profiles.sort_by(|a, b| a.id.cmp(&b.id));
        return profiles;
    }
    profiles.retain(|profile| selectors.contains(&profile.id));
    profiles.sort_by(|a, b| a.id.cmp(&b.id));
    profiles
}

fn expand_domain_selector(selector: &str) -> Vec<String> {
    if let Some(domains) = category_domains(selector) {
        return domains.iter().map(|domain| (*domain).to_string()).collect();
    }
    vec![selector.to_string()]
}

pub fn domain_matches_selection(domain: &str, configured_domains: &[String]) -> bool {
    let normalized = normalize_domain_selection(configured_domains);
    if normalized
        .iter()
        .any(|item| item == "disabled" || item == "none")
    {
        return false;
    }
    if normalized.is_empty()
        || normalized
            .iter()
            .any(|item| item == "auto" || item == "detected_only")
    {
        return true;
    }
    normalized
        .iter()
        .filter(|item| item.as_str() != "declared_only")
        .flat_map(|item| expand_domain_selector(item))
        .any(|item| item == domain)
}

fn category_domains(selector: &str) -> Option<&'static [&'static str]> {
    match selector {
        "language" => Some(&[
            "language.rust",
            "language.golang",
            "language.python",
            "language.typescript",
        ]),
        "iac" => Some(&["iac.pulumi", "iac.terraform"]),
        "database" => Some(&[
            "database.postgres_migrations",
            "database.clickhouse_migrations",
        ]),
        "streaming" => Some(&["streaming.valkey", "streaming.kafka"]),
        "storage" => Some(&["storage.object"]),
        "communications" => Some(&["communications.email"]),
        "observability" => Some(&["observability.otel_prometheus"]),
        "security" => Some(&["security.auth_authz"]),
        "resilience" => Some(&["resilience.service_patterns"]),
        "tenancy" => Some(&["tenancy.multi_tenant"]),
        "service_patterns" => Some(&[
            "resilience.service_patterns",
            "identity.uuidv7",
            "errors.registry",
        ]),
        _ => None,
    }
}

fn config_declared_domain_profile(domain: &str) -> DomainProfile {
    DomainProfile {
        id: domain.to_string(),
        kind: domain_kind(domain).to_string(),
        confidence: 100,
        evidence: vec![DomainEvidence {
            path: "agentactr.toml".to_string(),
            signal: "config_declared_domain".to_string(),
            weight: 100,
        }],
    }
}

fn domain_kind(domain: &str) -> &str {
    domain.split('.').next().unwrap_or("domain")
}

fn add_repository_module_graph_nodes(
    root: &Path,
    files: &[String],
    nodes: &mut Vec<DomainGraphNode>,
    edges: &mut Vec<DomainGraphEdge>,
) {
    for (module_path, manifest_path) in repository_modules(root, files) {
        let node_id = format!("repo_module:{}", graph_id(&module_path));
        nodes.push(DomainGraphNode {
            id: node_id.clone(),
            kind: "repo_module".to_string(),
            label: module_path,
            artifact_refs: vec![manifest_path],
        });
        edges.push(DomainGraphEdge {
            from: "repo:root".to_string(),
            to: node_id,
            kind: "depends_on".to_string(),
        });
    }
}

fn repository_modules(root: &Path, files: &[String]) -> Vec<(String, String)> {
    let mut modules = BTreeMap::new();
    for file in files {
        if is_repo_module_manifest(file) {
            let module_path = file
                .rsplit_once('/')
                .map(|(dir, _)| dir.to_string())
                .unwrap_or_else(|| ".".to_string());
            if module_path == "." || ignored_module_path(&module_path) {
                continue;
            }
            modules.entry(module_path).or_insert_with(|| file.clone());
        }
    }

    for member in cargo_workspace_members(root) {
        let manifest = format!("{member}/Cargo.toml");
        modules.entry(member).or_insert(manifest);
    }

    modules.into_iter().collect()
}

fn is_repo_module_manifest(file: &str) -> bool {
    file.ends_with("/Cargo.toml")
        || file.ends_with("/package.json")
        || file.ends_with("/go.mod")
        || file.ends_with("/pyproject.toml")
}

fn ignored_module_path(path: &str) -> bool {
    path.contains("node_modules")
        || path.starts_with("target/")
        || path.starts_with(".agentactr/")
        || path.starts_with(".git/")
}

fn cargo_workspace_members(root: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(root.join("Cargo.toml")) else {
        return Vec::new();
    };
    parse_cargo_workspace_members(&content)
        .into_iter()
        .filter(|member| root.join(member).join("Cargo.toml").exists())
        .collect()
}

fn parse_cargo_workspace_members(content: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut in_workspace = false;
    let mut collecting_members = false;

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            in_workspace = line == "[workspace]";
            collecting_members = false;
            continue;
        }
        if !in_workspace {
            continue;
        }
        if collecting_members {
            if line.starts_with(']') {
                collecting_members = false;
                continue;
            }
            if let Some(member) = parse_quoted_toml_string(line.trim_end_matches(',')) {
                members.push(member);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("members") {
            let Some((_, value)) = rest.split_once('=') else {
                continue;
            };
            let value = value.trim();
            if value.starts_with('[') && !value.ends_with(']') {
                collecting_members = true;
                let after_bracket = value.trim_start_matches('[').trim();
                if let Some(member) = parse_quoted_toml_string(after_bracket.trim_end_matches(','))
                {
                    members.push(member);
                }
            } else if value.starts_with('[') {
                members.extend(
                    value
                        .trim_matches(&['[', ']'][..])
                        .split(',')
                        .filter_map(|item| parse_quoted_toml_string(item.trim())),
                );
            }
        }
    }
    members.sort();
    members.dedup();
    members
}

fn parse_quoted_toml_string(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches(',');
    let quote = trimmed.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = trimmed[1..].find(quote)?;
    Some(trimmed[1..1 + end].to_string())
}

fn add_finding_graph_nodes(
    root: &Path,
    nodes: &mut Vec<DomainGraphNode>,
    edges: &mut Vec<DomainGraphEdge>,
) {
    for finding in domain_findings(root) {
        let node_id = format!("domain_finding:{}", graph_id(&finding.id));
        nodes.push(DomainGraphNode {
            id: node_id.clone(),
            kind: "domain_finding".to_string(),
            label: finding.id,
            artifact_refs: finding.evidence_paths,
        });
        edges.push(DomainGraphEdge {
            from: format!("domain:{}", finding.domain),
            to: node_id,
            kind: "has_gap".to_string(),
        });
    }
}

fn add_domain_specific_graph_nodes(
    root: &Path,
    files: &[String],
    profiles: &[DomainProfile],
    nodes: &mut Vec<DomainGraphNode>,
    edges: &mut Vec<DomainGraphEdge>,
) {
    let domain_ids = profiles
        .iter()
        .map(|profile| profile.id.as_str())
        .collect::<BTreeSet<_>>();
    if domain_ids.contains("api_contracts.protobuf") || domain_ids.contains("rpc.grpc") {
        add_proto_graph_nodes(root, nodes, edges);
    }
    for file in files {
        let name = file_name(file);
        let lower = file.to_ascii_lowercase();
        if domain_ids.contains("database.postgres_migrations")
            && postgres_signal(file, name.as_deref(), &lower)
        {
            add_file_graph_node(
                nodes,
                edges,
                "database.postgres_migrations",
                file,
                postgres_graph_node_kind(file, name.as_deref(), &lower),
                "depends_on",
            );
        }
        if domain_ids.contains("database.clickhouse_migrations") && clickhouse_signal(file, &lower)
        {
            add_file_graph_node(
                nodes,
                edges,
                "database.clickhouse_migrations",
                file,
                clickhouse_graph_node_kind(file, &lower),
                "depends_on",
            );
        }
        if domain_ids.contains("streaming.valkey") && valkey_signal(file, &lower) {
            add_file_graph_node(
                nodes,
                edges,
                "streaming.valkey",
                file,
                valkey_graph_node_kind(file, &lower),
                valkey_graph_edge_kind(&lower),
            );
        }
        if domain_ids.contains("streaming.kafka") && kafka_signal(file, &lower) {
            add_file_graph_node(
                nodes,
                edges,
                "streaming.kafka",
                file,
                kafka_graph_node_kind(&lower),
                kafka_graph_edge_kind(&lower),
            );
        }
        if domain_ids.contains("storage.object") && storage_signal(file, &lower) {
            add_file_graph_node(
                nodes,
                edges,
                "storage.object",
                file,
                storage_graph_node_kind(file, &lower),
                "consumes",
            );
        }
        if domain_ids.contains("communications.email") && communications_signal(file, &lower) {
            add_file_graph_node(
                nodes,
                edges,
                "communications.email",
                file,
                communications_graph_node_kind(file, &lower),
                "serves",
            );
        }
        if domain_ids.contains("observability.otel_prometheus")
            && observability_signal(file, &lower)
        {
            add_file_graph_node(
                nodes,
                edges,
                "observability.otel_prometheus",
                file,
                observability_graph_node_kind(file, &lower),
                "observes",
            );
        }
        if domain_ids.contains("security.auth_authz") && security_signal(&lower) {
            add_file_graph_node(
                nodes,
                edges,
                "security.auth_authz",
                file,
                security_graph_node_kind(&lower),
                "validates",
            );
        }
        if domain_ids.contains("resilience.service_patterns") && resilience_signal(&lower) {
            add_file_graph_node(
                nodes,
                edges,
                "resilience.service_patterns",
                file,
                resilience_graph_node_kind(&lower),
                "depends_on",
            );
        }
        if file.contains("template") || file.ends_with("AGENTS.md") {
            nodes.push(DomainGraphNode {
                id: format!("template:{}", graph_id(file)),
                kind: "template".to_string(),
                label: file.clone(),
                artifact_refs: vec![file.clone()],
            });
            edges.push(DomainGraphEdge {
                from: "repo:root".to_string(),
                to: format!("template:{}", graph_id(file)),
                kind: "depends_on".to_string(),
            });
        }
    }
    add_issue_set_graph_nodes(root, nodes, edges);
}

fn add_file_graph_node(
    nodes: &mut Vec<DomainGraphNode>,
    edges: &mut Vec<DomainGraphEdge>,
    domain: &str,
    file: &str,
    kind: &str,
    edge_kind: &str,
) {
    let node_id = format!("{kind}:{}", graph_id(file));
    nodes.push(DomainGraphNode {
        id: node_id.clone(),
        kind: kind.to_string(),
        label: file.to_string(),
        artifact_refs: vec![file.to_string()],
    });
    edges.push(DomainGraphEdge {
        from: format!("domain:{domain}"),
        to: node_id,
        kind: edge_kind.to_string(),
    });
}

fn postgres_graph_node_kind(file: &str, _name: Option<&str>, lower: &str) -> &'static str {
    if lower.contains("backfill") {
        "db_backfill"
    } else if lower.contains("seed") {
        "db_seed"
    } else if lower.contains("migration") || file.ends_with(".sql") {
        "db_migration"
    } else {
        "db_schema"
    }
}

fn clickhouse_graph_node_kind(_file: &str, lower: &str) -> &'static str {
    if lower.contains("materialized_view") || lower.contains("materialized view") {
        "clickhouse_materialized_view"
    } else if lower.contains("dictionary") {
        "clickhouse_dictionary"
    } else if lower.contains("replicated") {
        "clickhouse_replicated_table"
    } else if lower.contains("ingestion") {
        "clickhouse_ingestion_schema"
    } else {
        "clickhouse_schema"
    }
}

fn valkey_graph_node_kind(file: &str, lower: &str) -> &'static str {
    if lower.contains("xreadgroup") || lower.contains("xadd") || lower.contains("stream") {
        "valkey_stream"
    } else if lower.contains("pubsub") || lower.contains("publish") || lower.contains("subscribe") {
        "valkey_pubsub"
    } else if lower.contains("lock") {
        "valkey_lock"
    } else if lower.contains("rate_limit") || lower.contains("ratelimit") {
        "valkey_rate_limit_counter"
    } else if lower.contains("queue") {
        "valkey_ephemeral_queue"
    } else if file.contains("cache") || lower.contains("cache") {
        "valkey_cache"
    } else {
        "valkey_surface"
    }
}

fn valkey_graph_edge_kind(lower: &str) -> &'static str {
    if lower.contains("xreadgroup") || lower.contains("subscribe") || lower.contains("queue") {
        "consumes"
    } else {
        "depends_on"
    }
}

fn kafka_graph_node_kind(lower: &str) -> &'static str {
    if lower.contains("dead_letter") || lower.contains("dead-letter") || lower.contains("dlq") {
        "kafka_dlq"
    } else if lower.contains("retry") {
        "kafka_retry_topic"
    } else if lower.contains("schema-registry") || lower.contains("schema_registry") {
        "schema_registry_subject"
    } else if lower.contains("outbox") {
        "outbox"
    } else if lower.contains("inbox") {
        "inbox"
    } else if lower.contains("projection") {
        "event_projection"
    } else if lower.contains("consumer_group") || lower.contains("consumer-group") {
        "consumer_group"
    } else {
        "kafka_topic"
    }
}

fn kafka_graph_edge_kind(lower: &str) -> &'static str {
    if lower.contains("producer") || lower.contains("outbox") {
        "serves"
    } else {
        "consumes"
    }
}

fn storage_graph_node_kind(file: &str, lower: &str) -> &'static str {
    if lower.contains("signed_url") || lower.contains("signed-url") || lower.contains("presign") {
        "object_storage_signed_url"
    } else if lower.contains("bucket") {
        "object_storage_bucket"
    } else if lower.contains("lifecycle") || lower.contains("retention") {
        "object_storage_lifecycle_policy"
    } else if file.contains("storage") {
        "object_storage_surface"
    } else {
        "object_storage_object"
    }
}

fn communications_graph_node_kind(file: &str, lower: &str) -> &'static str {
    if lower.contains("template") || file.contains("template") {
        "notification_template"
    } else if lower.contains("bounce") || lower.contains("suppression") {
        "notification_suppression"
    } else {
        "notification_channel"
    }
}

fn observability_graph_node_kind(file: &str, lower: &str) -> &'static str {
    if lower.contains("prometheus") || lower.contains("metrics") {
        "metric_signal"
    } else if lower.contains("trace") || lower.contains("tracing") || lower.contains("otel") {
        "trace_signal"
    } else if lower.contains("log") {
        "log_signal"
    } else if file.contains("observability") {
        "observability_signal"
    } else {
        "telemetry_signal"
    }
}

fn security_graph_node_kind(lower: &str) -> &'static str {
    if lower.contains("authz") || lower.contains("authorization") || lower.contains("permission") {
        "authorization_policy"
    } else if lower.contains("authn") || lower.contains("authentication") {
        "authentication_boundary"
    } else {
        "security_policy"
    }
}

fn resilience_graph_node_kind(lower: &str) -> &'static str {
    if lower.contains("circuit") {
        "circuit_breaker"
    } else if lower.contains("retry") {
        "retry_policy"
    } else if lower.contains("bulkhead") {
        "bulkhead_policy"
    } else if lower.contains("deadline") {
        "deadline_policy"
    } else {
        "middleware"
    }
}

fn add_proto_graph_nodes(
    root: &Path,
    nodes: &mut Vec<DomainGraphNode>,
    edges: &mut Vec<DomainGraphEdge>,
) {
    let details = proto_details(root);
    for detail in details {
        let file_node = format!("schema:{}", graph_id(&detail.file));
        nodes.push(DomainGraphNode {
            id: file_node.clone(),
            kind: "protobuf_schema".to_string(),
            label: detail.file.clone(),
            artifact_refs: vec![detail.file.clone()],
        });
        edges.push(DomainGraphEdge {
            from: "domain:api_contracts.protobuf".to_string(),
            to: file_node.clone(),
            kind: "depends_on".to_string(),
        });
        if let Some(package) = detail.package.as_ref() {
            let package_node = format!("proto_package:{package}");
            nodes.push(DomainGraphNode {
                id: package_node.clone(),
                kind: "proto_package".to_string(),
                label: package.clone(),
                artifact_refs: vec![detail.file.clone()],
            });
            edges.push(DomainGraphEdge {
                from: file_node.clone(),
                to: package_node.clone(),
                kind: "imports".to_string(),
            });
            for service in &detail.services {
                let service_node = format!("grpc_service:{package}.{}", service.name);
                nodes.push(DomainGraphNode {
                    id: service_node.clone(),
                    kind: "grpc_service".to_string(),
                    label: format!("{package}.{}", service.name),
                    artifact_refs: vec![detail.file.clone()],
                });
                edges.push(DomainGraphEdge {
                    from: package_node.clone(),
                    to: service_node.clone(),
                    kind: "serves".to_string(),
                });
                edges.push(DomainGraphEdge {
                    from: "domain:rpc.grpc".to_string(),
                    to: service_node.clone(),
                    kind: "serves".to_string(),
                });
                for rpc in &service.rpcs {
                    let rpc_node = format!("rpc:{package}.{}.{}", service.name, graph_id(rpc));
                    nodes.push(DomainGraphNode {
                        id: rpc_node.clone(),
                        kind: "rpc".to_string(),
                        label: rpc.clone(),
                        artifact_refs: vec![detail.file.clone()],
                    });
                    edges.push(DomainGraphEdge {
                        from: service_node.clone(),
                        to: rpc_node,
                        kind: "serves".to_string(),
                    });
                }
            }
        }
    }
    if let Some(profile) = protobuf_profile(root) {
        if profile.buf_lock_present {
            let node_id = "protobuf_lock:buf.lock".to_string();
            nodes.push(DomainGraphNode {
                id: node_id.clone(),
                kind: "protobuf_dependency_lock".to_string(),
                label: "buf.lock".to_string(),
                artifact_refs: vec!["buf.lock".to_string()],
            });
            edges.push(DomainGraphEdge {
                from: "domain:api_contracts.protobuf".to_string(),
                to: node_id,
                kind: "depends_on".to_string(),
            });
        }
        for path in &profile.plugin_config_paths {
            let node_id = format!("protobuf_plugin_config:{}", graph_id(path));
            nodes.push(DomainGraphNode {
                id: node_id.clone(),
                kind: "protobuf_plugin_config".to_string(),
                label: path.clone(),
                artifact_refs: vec![path.clone()],
            });
            edges.push(DomainGraphEdge {
                from: "domain:api_contracts.protobuf".to_string(),
                to: node_id,
                kind: "depends_on".to_string(),
            });
        }
        for artifact in profile.generated_artifacts {
            let node_id = format!("generated_artifact:{}", graph_id(&artifact.path));
            nodes.push(DomainGraphNode {
                id: node_id.clone(),
                kind: "generated_artifact".to_string(),
                label: artifact.path.clone(),
                artifact_refs: vec![artifact.path],
            });
            edges.push(DomainGraphEdge {
                from: "domain:api_contracts.protobuf".to_string(),
                to: node_id,
                kind: "generates".to_string(),
            });
        }
    }
}

fn add_issue_set_graph_nodes(
    root: &Path,
    nodes: &mut Vec<DomainGraphNode>,
    edges: &mut Vec<DomainGraphEdge>,
) {
    let issue_root = root.join(".agentactr").join("artifacts").join("issues");
    let Ok(entries) = fs::read_dir(issue_root) else {
        return;
    };
    for entry in entries.flatten() {
        let artifact_dir = entry.path();
        if !artifact_dir.is_dir() {
            continue;
        }
        let manifest_path = artifact_dir.join("issue_set_manifest.json");
        let Ok(manifest_text) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<Value>(&manifest_text) else {
            continue;
        };
        let Some(issue_set_id) = manifest.get("issue_set_id").and_then(Value::as_str) else {
            continue;
        };
        let issue_set_node = format!("issue_set:{}", graph_id(issue_set_id));
        nodes.push(DomainGraphNode {
            id: issue_set_node.clone(),
            kind: "issue_set".to_string(),
            label: issue_set_id.to_string(),
            artifact_refs: vec![repo_relative_path(root, &manifest_path)],
        });
        edges.push(DomainGraphEdge {
            from: "repo:root".to_string(),
            to: issue_set_node.clone(),
            kind: "maps_to_issue".to_string(),
        });
        if let Some(parent_issue) = manifest.get("parent_issue").and_then(Value::as_u64) {
            let tracker_node = format!("tracker_issue:{parent_issue}");
            nodes.push(DomainGraphNode {
                id: tracker_node.clone(),
                kind: "tracker_issue".to_string(),
                label: format!("#{parent_issue}"),
                artifact_refs: vec![repo_relative_path(root, &manifest_path)],
            });
            edges.push(DomainGraphEdge {
                from: issue_set_node.clone(),
                to: tracker_node,
                kind: "maps_to_issue".to_string(),
            });
        }
        add_issue_proposal_graph_nodes(root, &artifact_dir, &issue_set_node, nodes, edges);
    }
}

fn add_issue_proposal_graph_nodes(
    root: &Path,
    artifact_dir: &Path,
    issue_set_node: &str,
    nodes: &mut Vec<DomainGraphNode>,
    edges: &mut Vec<DomainGraphEdge>,
) {
    let proposals_path = artifact_dir.join("issue_proposals.json");
    let Ok(proposals_text) = fs::read_to_string(&proposals_path) else {
        return;
    };
    let Ok(proposals) = serde_json::from_str::<Value>(&proposals_text) else {
        return;
    };
    let Some(items) = proposals.as_array() else {
        return;
    };
    for proposal in items {
        let Some(proposal_id) = proposal.get("proposal_id").and_then(Value::as_str) else {
            continue;
        };
        let node_id = format!("issue_proposal:{}", graph_id(proposal_id));
        nodes.push(DomainGraphNode {
            id: node_id.clone(),
            kind: "issue_proposal".to_string(),
            label: proposal_id.to_string(),
            artifact_refs: vec![repo_relative_path(root, &proposals_path)],
        });
        edges.push(DomainGraphEdge {
            from: issue_set_node.to_string(),
            to: node_id,
            kind: "maps_to_issue".to_string(),
        });
    }
}

fn repo_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn sort_graph(nodes: &mut Vec<DomainGraphNode>, edges: &mut Vec<DomainGraphEdge>) {
    nodes.sort_by(|a, b| a.id.cmp(&b.id).then(a.kind.cmp(&b.kind)));
    nodes.dedup_by(|a, b| a.id == b.id && a.kind == b.kind);
    edges.sort_by(|a, b| {
        a.from
            .cmp(&b.from)
            .then(a.kind.cmp(&b.kind))
            .then(a.to.cmp(&b.to))
    });
    edges.dedup_by(|a, b| a.from == b.from && a.to == b.to && a.kind == b.kind);
}

fn graph_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                ':'
            }
        })
        .collect()
}

#[derive(Clone, Debug)]
struct ProtoDetail {
    file: String,
    package: Option<String>,
    services: Vec<ProtoService>,
    enum_zero_findings: Vec<String>,
    has_reserved: bool,
    has_message: bool,
}

#[derive(Clone, Debug)]
struct ProtoService {
    name: String,
    rpcs: Vec<String>,
}

fn proto_details(root: &Path) -> Vec<ProtoDetail> {
    collect_repo_files(root)
        .into_iter()
        .filter(|file| file.ends_with(".proto"))
        .filter_map(|file| parse_proto_detail(root, &file))
        .collect()
}

fn parse_proto_detail(root: &Path, file: &str) -> Option<ProtoDetail> {
    let content = fs::read_to_string(root.join(file)).ok()?;
    let package = content
        .lines()
        .map(str::trim)
        .find_map(proto_package_declaration);
    let mut services = Vec::new();
    let mut current_service: Option<ProtoService> = None;
    let mut current_enum: Option<String> = None;
    let mut enum_first_value_seen = false;
    let mut enum_zero_findings = Vec::new();
    let mut has_reserved = false;
    let mut has_message = false;

    for line in content.lines().map(str::trim) {
        if line.starts_with("message ") {
            has_message = true;
        }
        if line.starts_with("reserved ") {
            has_reserved = true;
        }
        if let Some(enum_name) = line.strip_prefix("enum ").and_then(|rest| {
            rest.split(|ch: char| ch.is_whitespace() || ch == '{')
                .next()
                .filter(|name| !name.is_empty())
        }) {
            current_enum = Some(enum_name.to_string());
            enum_first_value_seen = false;
        } else if current_enum.is_some() && line.starts_with('}') {
            current_enum = None;
            enum_first_value_seen = false;
        } else if let Some(enum_name) = current_enum.as_ref() {
            if !enum_first_value_seen && line.contains("= 0") {
                enum_first_value_seen = true;
                let value_name = line.split('=').next().unwrap_or_default().trim();
                if !value_name.contains("UNSPECIFIED") && !value_name.contains("UNKNOWN") {
                    enum_zero_findings.push(format!("{enum_name}.{value_name}"));
                }
            }
        }

        if let Some(service_name) = proto_service_declaration(line) {
            if let Some(service) = current_service.take() {
                services.push(service);
            }
            current_service = Some(ProtoService {
                name: service_name,
                rpcs: Vec::new(),
            });
        }
        if let Some(rpc) = proto_rpc_declaration(line) {
            if let Some(service) = current_service.as_mut() {
                service.rpcs.push(rpc);
            }
        }
        if current_service.is_some() && line.starts_with('}') {
            if let Some(service) = current_service.take() {
                services.push(service);
            }
        }
    }
    if let Some(service) = current_service {
        services.push(service);
    }

    Some(ProtoDetail {
        file: file.to_string(),
        package,
        services,
        enum_zero_findings,
        has_reserved,
        has_message,
    })
}

fn proto_package_declaration(line: &str) -> Option<String> {
    line.strip_prefix("package ")
        .map(|value| value.trim_end_matches(';').trim())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn proto_service_declaration(line: &str) -> Option<String> {
    line.find("service ").and_then(|start| {
        line[start + "service ".len()..]
            .split(|ch: char| ch.is_whitespace() || ch == '{')
            .next()
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
    })
}

fn proto_rpc_declaration(line: &str) -> Option<String> {
    let start = line.find("rpc ")?;
    let declaration = line[start..]
        .split('{')
        .next()
        .unwrap_or(&line[start..])
        .trim()
        .trim_end_matches(';')
        .trim();
    (!declaration.is_empty()).then(|| declaration.to_string())
}

fn domain_gates_for(profile: &DomainProfile, files: &[String]) -> Vec<DomainQualityGate> {
    match profile.id.as_str() {
        "language.rust" => language_gate("language.rust", "Rust"),
        "language.golang" => language_gate("language.golang", "Go"),
        "language.python" => language_gate("language.python", "Python"),
        "language.typescript" => language_gate("language.typescript", "TypeScript"),
        "api_contracts.protobuf" => protobuf_gates(files),
        "rpc.grpc" => grpc_gates(),
        "database.postgres_migrations" => postgres_gates(),
        "database.clickhouse_migrations" => clickhouse_gates(),
        "streaming.valkey" => valkey_gates(),
        "streaming.kafka" => kafka_gates(),
        "storage.object" => storage_gates(),
        "communications.email" => communications_gates(),
        "observability.otel_prometheus" => observability_gates(),
        "security.auth_authz" => security_gates(),
        "resilience.service_patterns" => resilience_gates(),
        "tenancy.multi_tenant" => tenancy_gates(),
        "identity.uuidv7" => uuidv7_gates(),
        "errors.registry" => error_registry_gates(),
        "iac.pulumi" => pulumi_gates(),
        "iac.terraform" => terraform_gates(),
        _ => Vec::new(),
    }
}

fn language_gate(domain: &str, label: &str) -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        format!("{}_stack_contract", domain.replace('.', "_")),
        domain,
        "agentactr",
        format!(
            "{label} stack quality is composed through the stack quality plan and must keep generated/domain/adapters separated"
        ),
    )]
}

fn protobuf_gates(files: &[String]) -> Vec<DomainQualityGate> {
    if any_file_name(files, &["buf.yaml", "buf.gen.yaml"]) {
        vec![
            DomainQualityGate::command_gate(
                "protobuf_format",
                "api_contracts.protobuf",
                "buf",
                "buf format --diff --exit-code",
            ),
            DomainQualityGate::command_gate(
                "protobuf_lint",
                "api_contracts.protobuf",
                "buf",
                "buf lint",
            ),
            DomainQualityGate::command_gate(
                "protobuf_breaking",
                "api_contracts.protobuf",
                "buf",
                "buf breaking",
            ),
            DomainQualityGate::command_gate(
                "protobuf_generate",
                "api_contracts.protobuf",
                "buf",
                "buf generate",
            ),
            DomainQualityGate::finding_gate(
                "protobuf_generated_drift",
                "api_contracts.protobuf",
                "agentactr",
                "verify generated artifacts are isolated and no handwritten domain code is mixed into generated files",
            ),
        ]
    } else {
        vec![DomainQualityGate {
            degraded_if_missing: true,
            setup_guidance: vec![
                "add buf.yaml/buf.gen.yaml or pin protoc and language plugins for protobuf governance"
                    .to_string(),
            ],
            ..DomainQualityGate::finding_gate(
                "protobuf_governance_degraded",
                "api_contracts.protobuf",
                "agentactr",
                "protobuf schemas are present without Buf; fallback protoc governance must be explicit",
            )
        }]
    }
}

fn grpc_gates() -> Vec<DomainQualityGate> {
    vec![
        DomainQualityGate::finding_gate(
            "grpc_deadlines",
            "rpc.grpc",
            "agentactr",
            "client RPCs require deadlines, cancellation propagation, retry/idempotency policy, and status-code mapping",
        ),
        DomainQualityGate::finding_gate(
            "grpc_boundary_mapping",
            "rpc.grpc",
            "agentactr",
            "raw generated clients and DTOs must be wrapped behind ports before reaching domain services",
        ),
    ]
}

fn postgres_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "postgres_migration_safety",
        "database.postgres_migrations",
        "agentactr",
        "check migration ordering, destructive changes, expand/contract sequencing, concurrent indexes, rollback notes, and backfill runbooks",
    )]
}

fn clickhouse_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "clickhouse_schema_evolution",
        "database.clickhouse_migrations",
        "agentactr",
        "map materialized-view dependencies, avoid mutation-heavy updates, and document ingestion-compatible backfills",
    )]
}

fn valkey_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "valkey_semantics",
        "streaming.valkey",
        "agentactr",
        "distinguish transient Pub/Sub from durable Streams and document TTL, replay, pending entries, retries, and idempotency",
    )]
}

fn kafka_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "kafka_operational_contract",
        "streaming.kafka",
        "agentactr",
        "define topic naming, partition key, consumer group, schema compatibility, idempotent producer, replay, DLQ, and lag metrics",
    )]
}

fn storage_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "object_storage_policy",
        "storage.object",
        "agentactr",
        "require identity/IAM access, public access prevention, encryption, lifecycle, signed URL policy, object ownership, and data classification",
    )]
}

fn communications_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "communications_idempotency",
        "communications.email",
        "agentactr",
        "email/notification providers require idempotency keys, verified senders, suppression/bounce handling, and redacted artifacts",
    )]
}

fn observability_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "observability_semantics",
        "observability.otel_prometheus",
        "agentactr",
        "check traces, metrics, logs, context propagation, tenant/run correlation, Prometheus naming, and high-cardinality labels",
    )]
}

fn security_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "security_boundary_policy",
        "security.auth_authz",
        "agentactr",
        "auth/authz checks, credentials, redaction, and tenant boundaries must remain behind explicit ports and middleware",
    )]
}

fn resilience_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "resilience_patterns",
        "resilience.service_patterns",
        "agentactr",
        "document deadlines, retries, circuit breakers, bulkheads, middleware, rate limits, and outbox/queue worker behavior",
    )]
}

fn tenancy_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "tenant_isolation",
        "tenancy.multi_tenant",
        "agentactr",
        "data access, logs, metrics, storage, migrations, and backfills must preserve tenant isolation",
    )]
}

fn uuidv7_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "uuidv7_policy",
        "identity.uuidv7",
        "agentactr",
        "sortable UUIDv7 identity policy must be explicit and compatible with database/client generators",
    )]
}

fn error_registry_gates() -> Vec<DomainQualityGate> {
    vec![DomainQualityGate::finding_gate(
        "error_registry",
        "errors.registry",
        "agentactr",
        "stable error codes require severity, retryability, operator action, redaction class, and protocol mapping",
    )]
}

fn pulumi_gates() -> Vec<DomainQualityGate> {
    vec![
        DomainQualityGate::finding_gate(
            "pulumi_component_architecture",
            "iac.pulumi",
            "agentactr",
            "prefer reusable components with typed inputs/outputs, secret outputs, provider propagation, and stack config validation",
        ),
        DomainQualityGate {
            opt_in_required: true,
            credential_required: true,
            network_required: true,
            setup_guidance: vec![
                "enable quality.iac.allow_preview before running pulumi preview in unattended gates"
                    .to_string(),
            ],
            ..DomainQualityGate::command_gate(
                "pulumi_preview",
                "iac.pulumi",
                "pulumi",
                "pulumi preview --non-interactive --diff",
            )
        },
    ]
}

fn terraform_gates() -> Vec<DomainQualityGate> {
    vec![
        DomainQualityGate::command_gate(
            "terraform_fmt",
            "iac.terraform",
            "terraform",
            "terraform fmt -check -recursive",
        ),
        DomainQualityGate {
            network_required: false,
            setup_guidance: vec![
                "run with backend disabled and lockfile readonly for local validation".to_string(),
            ],
            ..DomainQualityGate::command_gate(
                "terraform_validate",
                "iac.terraform",
                "terraform",
                "terraform init -backend=false -lockfile=readonly && terraform validate",
            )
        },
        DomainQualityGate::finding_gate(
            "terraform_module_architecture",
            "iac.terraform",
            "agentactr",
            "prefer registry modules, pinned providers/modules, examples, tests, and reusable module boundaries",
        ),
    ]
}

fn domain_signals(
    file: &str,
    name: Option<&str>,
    lower: &str,
) -> Vec<(&'static str, &'static str, &'static str, u16)> {
    let mut signals = Vec::new();
    match name {
        Some(
            "Cargo.toml" | "Cargo.lock" | "deny.toml" | "rust-toolchain" | "rust-toolchain.toml",
        ) => signals.push(("language.rust", "language", "rust_marker", 10)),
        Some("go.mod" | "go.sum" | "go.work" | ".golangci.yml" | ".golangci.yaml") => {
            signals.push(("language.golang", "language", "go_marker", 10));
        }
        Some(
            "pyproject.toml" | "uv.lock" | "poetry.lock" | "pdm.lock" | "Pipfile.lock" | "setup.py"
            | "setup.cfg" | "tox.ini" | "noxfile.py" | "pytest.ini" | "mypy.ini",
        ) => signals.push(("language.python", "language", "python_marker", 10)),
        Some(
            "package.json" | "tsconfig.json" | "bun.lockb" | "bun.lock" | "pnpm-lock.yaml"
            | "package-lock.json" | "yarn.lock" | "biome.json" | "biome.jsonc" | "deno.json"
            | "deno.jsonc" | "deno.lock",
        ) => signals.push(("language.typescript", "language", "typescript_marker", 10)),
        _ => {}
    }
    if file.ends_with(".rs") {
        signals.push(("language.rust", "language", "rust_source", 1));
    } else if file.ends_with(".go") {
        signals.push(("language.golang", "language", "go_source", 1));
    } else if file.ends_with(".py") {
        signals.push(("language.python", "language", "python_source", 1));
    } else if file.ends_with(".ts") || file.ends_with(".tsx") {
        signals.push(("language.typescript", "language", "typescript_source", 1));
    }
    if file.ends_with(".proto") {
        signals.push(("api_contracts.protobuf", "api_contract", "proto_file", 8));
        signals.push(("rpc.grpc", "rpc", "proto_file", 4));
    }
    if matches!(name, Some("buf.yaml" | "buf.gen.yaml" | "buf.lock")) {
        signals.push(("api_contracts.protobuf", "api_contract", "buf_config", 10));
    }
    if matches!(name, Some("Pulumi.yaml" | "Pulumi.yml"))
        || lower.contains("/pulumi.")
        || lower.contains("pulumi.")
    {
        signals.push(("iac.pulumi", "iac", "pulumi_config", 8));
    }
    if file.ends_with(".tf") || matches!(name, Some(".terraform.lock.hcl" | ".tflint.hcl")) {
        signals.push(("iac.terraform", "iac", "terraform_file", 8));
    }
    if postgres_signal(file, name, lower) {
        signals.push((
            "database.postgres_migrations",
            "database",
            "postgres_migration",
            7,
        ));
    }
    if clickhouse_signal(file, lower) {
        signals.push((
            "database.clickhouse_migrations",
            "database",
            "clickhouse_schema",
            7,
        ));
    }
    if valkey_signal(file, lower) {
        signals.push(("streaming.valkey", "streaming", "valkey_usage", 5));
    }
    if kafka_signal(file, lower) {
        signals.push(("streaming.kafka", "streaming", "kafka_usage", 5));
    }
    if storage_signal(file, lower) {
        signals.push(("storage.object", "storage", "object_storage_usage", 5));
    }
    if communications_signal(file, lower) {
        signals.push(("communications.email", "communications", "email_usage", 5));
    }
    if observability_signal(file, lower) {
        signals.push((
            "observability.otel_prometheus",
            "observability",
            "telemetry_usage",
            5,
        ));
    }
    if resilience_signal(lower) {
        signals.push((
            "resilience.service_patterns",
            "service_pattern",
            "resilience_usage",
            4,
        ));
    }
    if tenancy_signal(lower) {
        signals.push(("tenancy.multi_tenant", "tenancy", "tenant_usage", 4));
    }
    if security_signal(lower) {
        signals.push((
            "security.auth_authz",
            "security",
            "security_policy_usage",
            4,
        ));
    }
    if lower.contains("uuidv7") || lower.contains("uuid_v7") || lower.contains("uuid-v7") {
        signals.push(("identity.uuidv7", "identity", "uuidv7_usage", 4));
    }
    if lower.contains("error_registry")
        || lower.contains("errors/registry")
        || lower.contains("error-codes")
        || lower.contains("error_codes")
    {
        signals.push(("errors.registry", "errors", "error_registry", 4));
    }
    signals
}

fn postgres_signal(file: &str, name: Option<&str>, lower: &str) -> bool {
    file.ends_with(".sql")
        && (lower.contains("postgres")
            || lower.contains("pgsql")
            || lower.contains("migration")
            || lower.contains("backfill")
            || lower.contains("prisma")
            || lower.contains("drizzle"))
        || matches!(
            name,
            Some("schema.prisma" | "drizzle.config.ts" | "drizzle.config.js" | "sqlx-data.json")
        )
}

fn clickhouse_signal(file: &str, lower: &str) -> bool {
    lower.contains("clickhouse")
        || lower.contains("materialized_view")
        || (file.ends_with(".sql") && lower.contains("analytics"))
}

fn valkey_signal(file: &str, lower: &str) -> bool {
    lower.contains("valkey")
        || lower.contains("redis")
        || lower.contains("xreadgroup")
        || lower.contains("xadd")
        || lower.contains("pubsub")
        || lower.contains("rate_limit")
        || file.contains("cache")
}

fn kafka_signal(_file: &str, lower: &str) -> bool {
    lower.contains("kafka")
        || lower.contains("schema-registry")
        || lower.contains("schema_registry")
        || lower.contains("outbox")
}

fn storage_signal(file: &str, lower: &str) -> bool {
    lower.contains("s3")
        || lower.contains("gcs")
        || lower.contains("google.storage")
        || lower.contains("blob")
        || lower.contains("bucket")
        || file.contains("storage")
}

fn communications_signal(file: &str, lower: &str) -> bool {
    lower.contains("resend")
        || lower.contains("sendgrid")
        || lower.contains("mailgun")
        || lower.contains("smtp")
        || file.contains("email")
}

fn observability_signal(file: &str, lower: &str) -> bool {
    lower.contains("opentelemetry")
        || lower.contains("otel")
        || lower.contains("prometheus")
        || lower.contains("metrics")
        || lower.contains("tracing")
        || file.contains("observability")
}

fn resilience_signal(lower: &str) -> bool {
    lower.contains("circuit")
        || lower.contains("retry")
        || lower.contains("bulkhead")
        || lower.contains("deadline")
        || lower.contains("middleware")
}

fn tenancy_signal(lower: &str) -> bool {
    lower.contains("tenant")
        || lower.contains("multi_tenant")
        || lower.contains("multitenant")
        || lower.contains("row_level")
        || lower.contains("rls")
}

fn security_signal(lower: &str) -> bool {
    lower.contains("authz")
        || lower.contains("authn")
        || lower.contains("authorization")
        || lower.contains("authentication")
        || lower.contains("permission")
        || lower.contains("policy")
}

fn generated_artifact_profile(file: &str) -> Option<GeneratedArtifactProfile> {
    if file.ends_with(".pb.go") {
        Some(GeneratedArtifactProfile {
            path: file.to_string(),
            language: "go".to_string(),
            generator: "protoc-gen-go".to_string(),
        })
    } else if file.ends_with("_pb2.py") || file.ends_with("_pb2_grpc.py") {
        Some(GeneratedArtifactProfile {
            path: file.to_string(),
            language: "python".to_string(),
            generator: "grpcio-tools".to_string(),
        })
    } else if file.ends_with(".pb.ts") || file.ends_with("_pb.ts") {
        Some(GeneratedArtifactProfile {
            path: file.to_string(),
            language: "typescript".to_string(),
            generator: "typescript-protobuf".to_string(),
        })
    } else if file.ends_with(".rs") && file.contains("proto") && file.contains("generated") {
        Some(GeneratedArtifactProfile {
            path: file.to_string(),
            language: "rust".to_string(),
            generator: "prost-tonic".to_string(),
        })
    } else {
        None
    }
}

fn read_proto_package(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    content.lines().map(str::trim).find_map(|line| {
        line.strip_prefix("package ")
            .and_then(|value| value.strip_suffix(';').or(Some(value)))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn any_file_name(files: &[String], names: &[&str]) -> bool {
    files.iter().any(|file| {
        file_name(file)
            .as_deref()
            .is_some_and(|name| names.contains(&name))
    })
}

fn file_name(file: &str) -> Option<String> {
    Path::new(file)
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
}

fn collect_repo_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_repo_files_inner(root, root, &mut files);
    files.sort();
    files
}

fn collect_repo_files_inner(root: &Path, dir: &Path, files: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if ignored_entry(&name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_repo_files_inner(root, &path, files);
        } else if file_type.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rendered = rel.to_string_lossy().replace('\\', "/");
                if !ignored_file(&rendered) {
                    files.push(rendered);
                }
            }
        }
    }
}

fn ignored_entry(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".agentactr"
            | ".trunk"
            | ".gomodcache"
            | ".cache"
            | "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".next"
            | ".venv"
            | "vendor"
            | "__pycache__"
    )
}

fn ignored_file(name: &str) -> bool {
    matches!(name, ".gitignore" | "agentactr.toml" | "WORKFLOW.md")
        || name.starts_with("specs_")
        || name.starts_with("internal_specs_agentactrSDK/")
}

fn current_epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        let root = env::temp_dir().join(format!(
            "agentactr-domains-{name}-{}",
            current_epoch_millis()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn detects_protobuf_grpc_and_buf_quality_gates() {
        let root = temp_root("protobuf");
        fs::write(root.join("buf.yaml"), "version: v2\n").unwrap();
        fs::write(root.join("buf.lock"), "version: v2\n").unwrap();
        fs::write(
            root.join("buf.gen.yaml"),
            "version: v2\nplugins:\n  - remote: buf.build/protocolbuffers/go:v1.36.5\n    out: gen/go\n",
        )
        .unwrap();
        fs::write(
            root.join("service.proto"),
            "syntax = \"proto3\";\npackage app.v1;\nimport \"google/api/annotations.proto\";\nimport \"protoc-gen-openapiv2/options/annotations.proto\";\n// connectrpc.com/connect marker\nservice Users { rpc Watch(stream Req) returns (stream Res); }\nmessage Req {}\nmessage Res {}\n",
        )
        .unwrap();

        let profiles = detect_domain_profiles(&root);
        assert!(profiles
            .iter()
            .any(|profile| profile.id == "api_contracts.protobuf"));
        assert!(profiles.iter().any(|profile| profile.id == "rpc.grpc"));

        let gates = domain_quality_plan(&root);
        assert!(gates.iter().any(|gate| gate.name == "protobuf_breaking"));
        assert!(gates
            .iter()
            .any(|gate| gate.command.is_none() && gate.name == "grpc_deadlines"));

        let proto = protobuf_profile(&root).unwrap();
        assert_eq!(proto.packages, vec!["app.v1"]);
        assert!(proto.buf_configured);
        assert!(proto.buf_lock_present);
        assert_eq!(proto.plugin_config_paths, vec!["buf.gen.yaml"]);
        assert!(proto.plugin_version_pins_present);
        let rpc = rpc_profile(&root).unwrap();
        assert_eq!(rpc.services, vec!["Users"]);
        assert_eq!(rpc.streaming_methods.len(), 1);
        assert!(rpc.connect_markers);
        assert!(rpc.openapi_annotations);

        let graph = build_domain_graph(&root, "OWNER/REPO");
        assert!(graph
            .nodes
            .iter()
            .any(|node| node.kind == "proto_package" && node.label == "app.v1"));
        assert!(graph
            .nodes
            .iter()
            .any(|node| node.kind == "grpc_service" && node.label == "app.v1.Users"));
        assert!(graph.edges.iter().any(|edge| edge.kind == "serves"));

        let findings = domain_findings(&root);
        assert!(findings
            .iter()
            .any(|finding| finding.id == "grpc_streaming_operational_contract"));
        assert!(!findings
            .iter()
            .any(|finding| finding.id == "protobuf_buf_lock_missing"));
    }

    #[test]
    fn protobuf_plugin_pin_detection_ignores_schema_version_and_bare_local_plugins() {
        let root = temp_root("protobuf-unpinned-plugin");
        fs::write(root.join("buf.yaml"), "version: v2\n").unwrap();
        fs::write(root.join("buf.lock"), "version: v2\n").unwrap();
        fs::write(
            root.join("buf.gen.yaml"),
            "version: v2\nplugins:\n  - local: protoc-gen-go\n    out: gen/go\n  - path: protoc-gen-connect-go\n    out: gen/connect\n",
        )
        .unwrap();
        fs::write(
            root.join("service.proto"),
            "syntax = \"proto3\";\npackage app.v1;\nmessage Req {}\n",
        )
        .unwrap();

        let proto = protobuf_profile(&root).unwrap();
        assert_eq!(proto.plugin_config_paths, vec!["buf.gen.yaml"]);
        assert!(!proto.plugin_version_pins_present);

        let findings = domain_findings(&root);
        assert!(findings
            .iter()
            .any(|finding| finding.id == "protobuf_plugin_version_pins_missing"));
    }

    #[test]
    fn protobuf_plugin_pin_detection_accepts_plugin_scoped_pins() {
        assert!(protobuf_plugin_config_has_version_pin(
            "version: v2\nplugins:\n  - remote: buf.build/protocolbuffers/go:v1.36.5\n    out: gen/go\n"
        ));
        assert!(protobuf_plugin_config_has_version_pin(
            "version: v2\nplugins:\n  - remote: buf.build/protocolbuffers/go\n    revision: 2\n    out: gen/go\n"
        ));
        assert!(protobuf_plugin_config_has_version_pin(
            "plugins:\n  - path: ./bin/protoc-gen-go@v1.36.5\n    out: gen/go\n"
        ));
        assert!(!protobuf_plugin_config_has_version_pin(
            "version: v2\nplugins:\n  - remote: buf.build/protocolbuffers/go\n    out: gen/go\n  - local: protoc-gen-go\n    out: gen/local\n"
        ));
        assert!(!protobuf_plugin_config_has_version_pin(
            "version: v2\nrevision: 2\nplugins:\n  - remote: buf.build/protocolbuffers/go\n    out: gen/go\n"
        ));
    }

    #[test]
    fn detects_language_domains_and_stack_nodes() {
        let root = temp_root("language");
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
        fs::create_dir_all(root.join("crates/demo")).unwrap();
        fs::write(
            root.join("crates/demo/Cargo.toml"),
            "[package]\nname = \"demo_module\"\n",
        )
        .unwrap();

        let profiles = detect_domain_profiles(&root);
        assert!(profiles.iter().any(|profile| {
            profile.id == "language.rust" && profile.kind == "language" && profile.confidence > 0
        }));

        let graph = build_domain_graph(&root, "OWNER/REPO");
        assert!(graph
            .nodes
            .iter()
            .any(|node| node.id == "domain:language.rust"));
        assert!(graph.edges.iter().any(|edge| edge.from == "repo:root"
            && edge.to == "domain:language.rust"
            && edge.kind == "has_domain"));
        assert!(graph
            .nodes
            .iter()
            .any(|node| node.kind == "repo_module" && node.label == "crates/demo"));
    }

    #[test]
    fn graph_connects_domain_findings_with_has_gap_edges() {
        let root = temp_root("findings");
        fs::write(
            root.join("service.proto"),
            "syntax = \"proto3\";\npackage app.v1;\nservice Users { rpc Get(Req) returns (Res); }\nmessage Req {}\nmessage Res {}\n",
        )
        .unwrap();

        let graph = build_domain_graph(&root, "OWNER/REPO");
        assert!(graph
            .nodes
            .iter()
            .any(|node| node.kind == "domain_finding"
                && node.label == "protobuf_governance_degraded"));
        assert!(graph
            .edges
            .iter()
            .any(|edge| edge.kind == "has_gap" && edge.from == "domain:api_contracts.protobuf"));
    }

    #[test]
    fn graph_is_versioned_and_references_artifacts_without_vendor_payloads() {
        let root = temp_root("graph");
        fs::create_dir_all(root.join("migrations")).unwrap();
        fs::write(
            root.join("migrations/0001_init.sql"),
            "create table users(id uuid);\n",
        )
        .unwrap();
        let issue_dir = root.join(".agentactr/artifacts/issues/draft-1");
        fs::create_dir_all(&issue_dir).unwrap();
        fs::write(
            issue_dir.join("issue_set_manifest.json"),
            r##"{"issue_set_id":"draft-1","parent_issue":42,"repo":"OWNER/REPO"}"##,
        )
        .unwrap();
        fs::write(
            issue_dir.join("issue_proposals.json"),
            r##"[{"proposal_id":"proposal-1","title":"redacted","body":"redacted"}]"##,
        )
        .unwrap();

        let graph = build_domain_graph(&root, "OWNER/REPO");
        assert_eq!(graph.schema_version, DOMAIN_GRAPH_SCHEMA_VERSION);
        assert!(graph.nodes.iter().any(|node| node.id == "repo:root"));
        assert!(graph
            .nodes
            .iter()
            .any(|node| node.id == "domain:database.postgres_migrations"));
        assert!(graph
            .nodes
            .iter()
            .any(|node| node.kind == "tracker_issue" && node.label == "#42"));
        assert!(graph
            .nodes
            .iter()
            .any(|node| node.kind == "issue_proposal" && node.label == "proposal-1"));

        let json = domain_graph_to_json(&graph);
        assert_eq!(json["schema_version"], DOMAIN_GRAPH_SCHEMA_VERSION);
        assert!(json["detected_domains"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("database.postgres_migrations")));
        assert!(json["evidence_references"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("migrations/0001_init.sql")));
        assert!(json.get("raw_vendor_payload").is_none());
    }

    #[test]
    fn platform_graph_uses_specific_resource_nodes() {
        let root = temp_root("platform");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/valkey_stream.ts"),
            "redis.xadd('events', '*')",
        )
        .unwrap();
        fs::write(
            root.join("src/outbox.ts"),
            "kafkaProducer.send({ topic: 'events.retry' })",
        )
        .unwrap();
        fs::write(
            root.join("src/storage_bucket.ts"),
            "const bucket = s3.bucket",
        )
        .unwrap();
        fs::write(root.join("src/metrics.ts"), "prometheus.metrics()").unwrap();

        let graph = build_domain_graph(&root, "OWNER/REPO");
        assert!(graph.nodes.iter().any(|node| node.kind == "valkey_stream"));
        assert!(graph.nodes.iter().any(|node| node.kind == "outbox"));
        assert!(graph
            .nodes
            .iter()
            .any(|node| node.kind == "object_storage_bucket"));
        assert!(graph.nodes.iter().any(|node| node.kind == "metric_signal"));
        assert!(graph.edges.iter().any(|edge| edge.kind == "consumes"));
        assert!(graph.edges.iter().any(|edge| edge.kind == "serves"));
    }

    #[test]
    fn configured_domain_selection_can_disable_or_declare_domains() {
        let root = temp_root("configured");
        fs::write(root.join("service.proto"), "syntax = \"proto3\";\n").unwrap();
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.architecture.domains = vec!["disabled".to_string()];
        config.quality.domains = vec!["disabled".to_string()];
        assert!(detect_domain_profiles_with_config(&root, &config).is_empty());
        assert!(domain_quality_plan_with_config(&root, &config).is_empty());

        config.architecture.domains = vec!["streaming.kafka".to_string()];
        config.quality.domains = vec!["streaming.kafka".to_string()];
        let profiles = detect_domain_profiles_with_config(&root, &config);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, "streaming.kafka");
        assert_eq!(profiles[0].evidence[0].signal, "config_declared_domain");
        let gates = domain_quality_plan_with_config(&root, &config);
        assert!(gates.iter().any(|gate| gate.domain == "streaming.kafka"));
    }

    #[test]
    fn category_selectors_filter_or_declare_canonical_domains() {
        let root = temp_root("category");
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        fs::write(root.join("buf.yaml"), "version: v2\n").unwrap();
        fs::write(
            root.join("service.proto"),
            "syntax = \"proto3\";\npackage app.v1;\n",
        )
        .unwrap();

        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.architecture.domains = vec!["detected_only".to_string(), "language".to_string()];
        let profiles = detect_domain_profiles_with_config(&root, &config);
        assert!(profiles.iter().any(|profile| profile.id == "language.rust"));
        assert!(!profiles
            .iter()
            .any(|profile| profile.id == "api_contracts.protobuf"));
        assert!(!profiles.iter().any(|profile| profile.id == "language"));

        config.quality.domains = vec!["declared_only".to_string(), "iac".to_string()];
        let gates = domain_quality_plan_with_config(&root, &config);
        assert!(gates.iter().any(|gate| gate.domain == "iac.pulumi"));
        assert!(gates.iter().any(|gate| gate.domain == "iac.terraform"));
        assert!(!gates.iter().any(|gate| gate.domain == "iac"));
    }
}
