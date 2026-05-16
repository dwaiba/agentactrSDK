use agentactr_sdk::{ExecutionConfig, LinuxMemoryConfig};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionBackend {
    NativeLinuxCgroupV2,
    DockerLinuxVm,
    NativeMacosObserveOnly,
    ObserveOnly,
}

impl ExecutionBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NativeLinuxCgroupV2 => "native_linux_cgroup_v2",
            Self::DockerLinuxVm => "docker_linux_vm",
            Self::NativeMacosObserveOnly => "native_macos_observe_only",
            Self::ObserveOnly => "observe_only",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionBackendDecision {
    pub configured: String,
    pub effective: ExecutionBackend,
    pub strict_memory_required: bool,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct ProcessCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub worktree: PathBuf,
    pub artifact_dir: PathBuf,
    pub trace_path: PathBuf,
    pub run_id: String,
    pub agent_run_id: String,
}

impl ProcessCommandSpec {
    pub fn host_command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .envs(self.env.iter().map(|(key, value)| (key, value)));
        command
    }
}

pub fn resolve_execution_backend(
    config: &ExecutionConfig,
) -> Result<ExecutionBackendDecision, String> {
    let configured = config.backend.trim();
    let effective = match configured {
        "" | "auto" => {
            if cfg!(target_os = "linux") {
                ExecutionBackend::NativeLinuxCgroupV2
            } else if cfg!(target_os = "macos") {
                ExecutionBackend::DockerLinuxVm
            } else {
                ExecutionBackend::ObserveOnly
            }
        }
        "native_linux_cgroup_v2" | "linux_cgroup_v2" | "linux" => {
            ExecutionBackend::NativeLinuxCgroupV2
        }
        "docker_linux_vm" | "docker" | "docker_desktop" => ExecutionBackend::DockerLinuxVm,
        "native_macos_observe_only" | "macos_observe_only" => {
            ExecutionBackend::NativeMacosObserveOnly
        }
        "observe_only" | "observe" | "disabled" | "off" => ExecutionBackend::ObserveOnly,
        other => {
            return Err(format!(
                "unsupported execution.backend: {other}; expected auto, native_linux_cgroup_v2, docker_linux_vm, native_macos_observe_only, or observe_only"
            ));
        }
    };
    let reason = match (&effective, configured) {
        (ExecutionBackend::DockerLinuxVm, "auto" | "") => {
            "auto selected Docker Linux backend on macOS for strict cgroup memory semantics"
        }
        (ExecutionBackend::NativeLinuxCgroupV2, "auto" | "") => {
            "auto selected native Linux cgroup v2 backend"
        }
        (ExecutionBackend::ObserveOnly, "auto" | "") => {
            "auto selected observe-only backend on an unsupported non-Linux/non-macOS host"
        }
        _ => "explicit execution backend configured",
    };
    Ok(ExecutionBackendDecision {
        configured: if configured.is_empty() {
            "auto".to_string()
        } else {
            configured.to_string()
        },
        effective,
        strict_memory_required: config.strict_memory_required,
        reason: reason.to_string(),
    })
}

pub fn docker_command(
    config: &ExecutionConfig,
    memory: &LinuxMemoryConfig,
    spec: &ProcessCommandSpec,
) -> Result<Command, String> {
    let docker = &config.docker;
    validate_docker_mount_mode("workspace_mount", &docker.workspace_mount)?;
    validate_docker_mount_mode("artifact_mount", &docker.artifact_mount)?;
    let mut command = Command::new(&docker.command);
    command.arg("run");
    if docker.remove_containers {
        command.arg("--rm");
    }
    command
        .arg("--name")
        .arg(container_name(
            &docker.container_prefix,
            &spec.run_id,
            &spec.agent_run_id,
        )?)
        .arg("--network")
        .arg(&docker.network)
        .arg("--memory")
        .arg(&memory.per_agent_memory_max)
        .arg("--memory-reservation")
        .arg(&memory.per_agent_memory_high)
        .arg("--oom-score-adj")
        .arg(memory.oom_score_adj.to_string())
        .arg("-v")
        .arg(volume_arg(
            &spec.worktree,
            &spec.worktree,
            &docker.workspace_mount,
        )?)
        .arg("-v")
        .arg(volume_arg(
            &spec.artifact_dir,
            &spec.artifact_dir,
            &docker.artifact_mount,
        )?)
        .arg("-v")
        .arg(volume_arg(
            &trace_mount_root(&spec.trace_path),
            &trace_mount_root(&spec.trace_path),
            "rw",
        )?)
        .arg("-w")
        .arg(&spec.cwd);
    for (key, value) in &spec.env {
        command.arg("-e").arg(format!("{key}={value}"));
    }
    command
        .arg(&docker.image)
        .arg(&spec.program)
        .args(&spec.args);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    Ok(command)
}

pub fn should_pull_image(config: &ExecutionConfig) -> bool {
    matches!(
        config.docker.pull_policy.as_str(),
        "always" | "if_missing" | "if-not-present" | "if_not_present"
    )
}

fn validate_docker_mount_mode(name: &str, value: &str) -> Result<(), String> {
    match value {
        "ro" | "rw" => Ok(()),
        other => Err(format!("{name} must be ro or rw, got {other}")),
    }
}

fn volume_arg(host: &Path, container: &Path, mode: &str) -> Result<String, String> {
    if host.as_os_str().is_empty() || container.as_os_str().is_empty() {
        return Err("Docker volume paths must be non-empty".to_string());
    }
    Ok(format!("{}:{}:{mode}", host.display(), container.display()))
}

fn trace_mount_root(path: &Path) -> PathBuf {
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn container_name(prefix: &str, run_id: &str, agent_run_id: &str) -> Result<String, String> {
    let prefix = safe_container_part(prefix);
    let run = safe_container_part(run_id);
    let agent = safe_container_part(agent_run_id);
    let name = format!("{prefix}-{run}-{agent}");
    let trimmed = name.trim_matches('-');
    if trimmed.is_empty() {
        Err("Docker container name would be empty".to_string())
    } else {
        Ok(trimmed.chars().take(120).collect())
    }
}

fn safe_container_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentactr_sdk::AgentactrConfig;
    use std::path::PathBuf;

    #[test]
    fn explicit_docker_backend_resolves() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO").execution;
        config.backend = "docker_linux_vm".to_string();

        let decision = resolve_execution_backend(&config).unwrap();

        assert_eq!(decision.effective, ExecutionBackend::DockerLinuxVm);
        assert!(decision.strict_memory_required);
    }

    #[test]
    fn docker_command_wraps_process_with_memory_and_mounts() {
        let config = AgentactrConfig::strict_defaults("OWNER/REPO");
        let spec = ProcessCommandSpec {
            program: "codex".to_string(),
            args: vec!["exec".to_string(), "--json".to_string()],
            cwd: PathBuf::from("/repo/worktree"),
            env: vec![("AGENTACTR_RUN_ID".to_string(), "run-1".to_string())],
            worktree: PathBuf::from("/repo/worktree"),
            artifact_dir: PathBuf::from("/repo/artifacts/run-1"),
            trace_path: PathBuf::from("/repo/runs/events.jsonl"),
            run_id: "run-1".to_string(),
            agent_run_id: "agent-1".to_string(),
        };

        let command = docker_command(&config.execution, &config.linux_memory, &spec).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(command.get_program().to_string_lossy(), "docker");
        assert!(args.contains(&"--memory".to_string()));
        assert!(args.contains(&"4G".to_string()));
        assert!(args.contains(&"--memory-reservation".to_string()));
        assert!(args.contains(&"3G".to_string()));
        assert!(args.contains(&"ghcr.io/dwaiba/agentactr-runtime:0.1.0-linux-arm64".to_string()));
        assert!(args.contains(&"--network".to_string()));
        assert!(args.contains(&"bridge".to_string()));
        assert!(args.ends_with(&[
            "codex".to_string(),
            "exec".to_string(),
            "--json".to_string()
        ]));
    }

    #[test]
    fn docker_command_preserves_piped_stdout_and_stderr_for_runtime_streaming() {
        let mut config = AgentactrConfig::strict_defaults("OWNER/REPO");
        config.execution.docker.command = "true".to_string();
        let spec = ProcessCommandSpec {
            program: "codex".to_string(),
            args: vec!["exec".to_string(), "--json".to_string()],
            cwd: PathBuf::from("/repo/worktree"),
            env: vec![("AGENTACTR_RUN_ID".to_string(), "run-1".to_string())],
            worktree: PathBuf::from("/repo/worktree"),
            artifact_dir: PathBuf::from("/repo/artifacts/run-1"),
            trace_path: PathBuf::from("/repo/runs/events.jsonl"),
            run_id: "run-1".to_string(),
            agent_run_id: "agent-1".to_string(),
        };

        let mut command = docker_command(&config.execution, &config.linux_memory, &spec).unwrap();
        let mut child = command.spawn().unwrap();

        assert!(child.stdout.take().is_some());
        assert!(child.stderr.take().is_some());
        let _ = child.wait();
    }
}
