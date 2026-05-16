# agentactr Workflow

Default mode is unattended and fail-closed.

Required operator setup:

```bash
codex login
export GITHUB_TOKEN=...
agentactr doctor --fix-codex-config
```

For API-key based Codex auth:

```bash
export CODEX_API_KEY=...
agentactr init --yes --repo OWNER/REPO --codex-auth api-key
```

Strict defaults:

- Codex approval policy: never
- human intervention: fail_closed
- Git worktree isolation
- pre-commit required
- local commit only after quality gates
- no push or merge by default
- remote GitHub MCP write tools disabled

GitHub workflow defaults:

- PR and merge-queue workflows remain secret-free and do not publish images.
- Expensive Docker image build work is delegated to trusted remote build services such as Depot when the trigger context is trusted.
- Push-to-main Dockerfile checks use Depot `call: check` without image publish.
- `nightly` and `security` schedules are currently commented out; `nightly` remains manually runnable through `workflow_dispatch`.
- `release` publishes Depot-built images only from a trusted version tag or trusted manual dispatch.

## References

[1] Depot, "Container builds in GitHub Actions," Depot Documentation. [Online]. Available: https://depot.dev/docs/container-builds/integrations/github-actions. [Accessed: May 16, 2026].

[2] Docker, "Validating build configuration with GitHub Actions," Docker Docs. [Online]. Available: https://docs.docker.com/build/ci/github-actions/checks/. [Accessed: May 16, 2026].
