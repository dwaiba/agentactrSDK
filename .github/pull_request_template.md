## Summary

Describe the change and the boundary it touches.

## Boundary Checklist

- [ ] Core remains provider-neutral; no transport, GitHub, Docker, Codex, SQLite, process, or cgroup details leaked into `agentactr-core`.
- [ ] SDK owns use-case policy/composition; concrete adapters stay outside `agentactr-sdk`.
- [ ] CLI changes are wiring, HCI, local artifact/reporting, or current default adapter packaging.
- [ ] `specs_agentactrSDK.md` is updated for any contract or architectural change.
- [ ] README/docs/diagrams are synchronized when behavior or architecture changes.
- [ ] No secrets, tokens, private prompts, or private local paths are added.

## Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `scripts/check-architecture-boundaries.sh`
- [ ] `scripts/check-github-workflow-gates.sh`
- [ ] `cargo check --workspace --all-features`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace --all-features`
- [ ] `cargo test --doc --workspace --all-features`

## Release Risk

- [ ] This does not increase untrusted PR runner cost.
- [ ] This does not add a new secret to PR workflows.
- [ ] Docker image publishing remains restricted to trusted release/nightly paths.
