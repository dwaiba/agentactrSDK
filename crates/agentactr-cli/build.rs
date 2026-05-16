use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");

    let git_sha = command_output("git", &["rev-parse", "--verify", "HEAD"])
        .and_then(|sha| sha.get(..12).map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    let rustc_version =
        command_output("rustc", &["--version"]).unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=AGENTACTR_BUILD_GIT_SHA={git_sha}");
    println!("cargo:rustc-env=AGENTACTR_BUILD_RUSTC_VERSION={rustc_version}");
}

fn command_output(command: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let value = stdout.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}
