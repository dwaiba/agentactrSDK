# Contributing

Thank you for contributing to `agentactrSDK`. This repository is intentionally strict because it is an agent runtime and SDK boundary project.

## Ground Rules

- Follow `AGENTS.md`.
- Treat `specs_agentactrSDK.md` as the architectural source of truth.
- Keep `agentactr-core` provider-neutral.
- Keep SDK policy/use-case orchestration in `agentactr-sdk`.
- Keep concrete transports, REST payloads, Docker commands, Codex wiring, and GitHub details behind adapters or CLI wiring.
- Do not include secrets, tokens, raw prompts, private logs, or private local paths in issues, PRs, tests, fixtures, or artifacts.

## Local Validation

Before opening a PR, run:

```bash
cargo fmt --all -- --check
scripts/check-architecture-boundaries.sh
scripts/check-github-workflow-gates.sh
scripts/check-docker-release-metadata.sh
cargo check --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --doc --workspace --all-features
cargo run --bin agentactr -- docs cli-markdown | cmp -s docs/cli/reference.md -
```

Install hooks:

```bash
pre-commit install --hook-type pre-commit --hook-type pre-push
```

## Pull Requests

PRs should be small and boundary-aware. Contract changes must include:

- spec update,
- tests,
- README or generated CLI docs when operator behavior changes,
- architecture diagram updates when boundaries change.

Untrusted PR workflows must not receive repository secrets and must not publish packages or Docker images.

## Docker And Remote Builds

Remote build services such as Depot are trusted and preferred for expensive Docker image builds when the workflow context is trusted. In the present workflow surface:

- PR and merge-queue workflows run validation plus Dockerfile checks only on GitHub-hosted runners, with no Depot token and no image publish.
- Push-to-main Dockerfile checks use Depot `call: check` without publishing images.
- `nightly` is manual-only because its cron schedule is commented out; when run manually, it builds and smoke-tests images with Depot.
- `release` builds and pushes runtime/static CLI images with Depot only from trusted tag or trusted manual dispatch.
- `security` keeps its weekly cron schedule commented out and uses no Depot credentials.

Do not add repository secrets or Depot credentials to untrusted PR or merge-queue workflows. If a future workflow needs remote-build acceleration, first prove the trigger context is trusted or that the path is explicitly secret-free.

## References

[1] GitHub, "Approving workflow runs from forks," GitHub Docs. [Online]. Available: https://docs.github.com/en/actions/managing-workflow-runs-and-deployments/managing-workflow-runs/approving-workflow-runs-from-private-forks. [Accessed: May 15, 2026].

[2] GitHub, "Security hardening for GitHub Actions," GitHub Docs. [Online]. Available: https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions. [Accessed: May 15, 2026].

[3] Depot, "Container builds in GitHub Actions," Depot Documentation. [Online]. Available: https://depot.dev/docs/container-builds/integrations/github-actions. [Accessed: May 16, 2026].

[4] Docker, "Validating build configuration with GitHub Actions," Docker Docs. [Online]. Available: https://docs.docker.com/build/ci/github-actions/checks/. [Accessed: May 16, 2026].
