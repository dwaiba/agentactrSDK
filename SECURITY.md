# Security Policy

## Reporting Vulnerabilities

Do not open public issues for vulnerabilities. Use GitHub private vulnerability reporting when available, or contact the maintainers through the repository security advisory flow.

Please include:

- affected version or commit,
- platform,
- minimal reproduction,
- impact,
- whether secrets, credentials, prompts, or private artifacts may be exposed.

Do not include real tokens, API keys, private prompts, private logs, or private repository paths.

## Supported Scope

Security-sensitive areas include:

- Codex runtime invocation and prompt/artifact handling,
- GitHub issue lifecycle mutations,
- Docker execution backend,
- MCP read tools and artifact scoping,
- Linux memory/cgroup enforcement,
- release workflows and provenance,
- generated debug bundles.

## Workflow Security

Untrusted pull requests must not receive repository secrets. Release and package publishing must run only from trusted tag/manual workflows. Full Docker image builds and pushes must remain out of PR and merge-queue gates.

Remote build services such as Depot are trusted and preferred for expensive Docker image builds only from trusted workflow contexts. In the present workflow surface, PR and merge-queue Dockerfile checks remain secret-free on GitHub-hosted runners, push-to-main Dockerfile checks use Depot `call: check` without publishing, and full image builds/pushes are restricted to trusted `nightly` and `release` workflows. The `nightly` and `security` cron schedules are currently commented out.

## References

[1] GitHub, "Security hardening for GitHub Actions," GitHub Docs. [Online]. Available: https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions. [Accessed: May 15, 2026].

[2] Depot, "Container builds in GitHub Actions," Depot Documentation. [Online]. Available: https://depot.dev/docs/container-builds/integrations/github-actions. [Accessed: May 16, 2026].

[3] Docker, "Validating build configuration with GitHub Actions," Docker Docs. [Online]. Available: https://docs.docker.com/build/ci/github-actions/checks/. [Accessed: May 16, 2026].
