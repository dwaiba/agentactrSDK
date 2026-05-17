# Release Readiness

Use this checklist before announcing `agentactrSDK` publicly.

## Required Before Public Announcement

- Remote `main` is protected by a branch protection rule or ruleset.
- Required checks are configured for:
  - `Architecture / SOLID boundaries`
  - `Build / Rust workspace build`
  - `Build / Dockerfile checks`
  - `CI / Rust quality gate`
  - `Security / CodeQL`
  - `Security / RustSec audit`
  - `Security / Supply-chain metadata`
- Pull requests are required before merge.
- Merge queue is enabled when the repository owner wants serialized merges.
- First-time contributor workflow approval is enabled.
- `pull_request_target` is not used.
- Repository secrets are not available to untrusted PR workflows.
- Tag creation for `v*.*.*` is limited to maintainers.
- Release workflow is run only from trusted tag push or trusted manual dispatch.
- Depot repository/action variable `DEPOT_PROJECT_ID` is configured with the Depot project ID for trusted Docker builds.
- Depot action secret `DEPOT_TOKEN` is configured with a project-scoped Depot token.
- Native CLI binary and macOS `.pkg` release assets are intentionally disabled until the signing, notarization, provenance, and install verification path is re-enabled as a separate release slice.
- If native macOS distribution is re-enabled later, the self-hosted Apple Silicon macOS runner must have Xcode command-line tools available for `codesign`, `notarytool`, `pkgbuild`, `productsign`, `stapler`, and `spctl`.
- If native macOS distribution is re-enabled later, macOS release signing must use either an imported Developer ID certificate bundle through `MACOS_CERTIFICATE_P12_BASE64`, `MACOS_CERTIFICATE_PASSWORD`, and `MACOS_KEYCHAIN_PASSWORD`, or preinstalled Developer ID Application and Developer ID Installer identities available to the runner keychain.
- If native macOS distribution is re-enabled later, macOS notary credentials must be configured through `MACOS_NOTARY_APPLE_ID`, `MACOS_NOTARY_PASSWORD`, and `MACOS_NOTARY_TEAM_ID`, or an existing notarytool keychain profile must be present on the runner.
- Spending or budget controls are set at the account or organization level.

## Runner Cost Controls

- Remote build services such as Depot are trusted and preferred over local Docker builds for expensive or release-sensitive image work when the workflow context is trusted.
- PR and merge-queue workflows run only validation and Dockerfile checks on GitHub-hosted runners.
- Push-to-main Dockerfile checks run through Depot `call: check` without image publish.
- Full Docker image builds and pushes are limited to `nightly` and `release`.
- Scheduled `nightly` and `security` cron triggers are currently commented out; run `nightly` manually with `workflow_dispatch` when needed.
- All workflow jobs have `timeout-minutes`.
- PR workflows use `concurrency` with `cancel-in-progress: true`.
- External GitHub Actions are pinned to full 40-character commit SHAs; version tags are comments only, and `scripts/check-github-workflow-gates.sh` rejects mutable tags, branches, missing refs, and short SHAs [4].
- Workflow changes require maintainer review before workflow approval.
- Trusted `nightly` and `release` image builds use Depot, while Depot credentials stay out of untrusted PR and merge-queue workflows.

## First Release Rehearsal

Run from a fresh clone:

```bash
cargo fmt --all -- --check
scripts/check-architecture-boundaries.sh
scripts/check-github-workflow-gates.sh
scripts/check-docker-release-metadata.sh
cargo check --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --doc --workspace --all-features
cargo build --release --workspace --all-features
target/release/agentactr --version
target/release/agentactr --help
target/release/agentactr doctor
target/release/agentactr commands --json
```

## Release Decision

Do not tag a release until the exact release commit is green on remote required checks. The current release workflow publishes Depot-built runtime/static image metadata after a trusted `v*.*.*` tag push or trusted manual dispatch; native CLI binary archives and macOS `.pkg` assets are not attached. Operators should build the CLI locally with `cargo build --release --bin agentactr`. If native binary release distribution is re-enabled later, the Apple Silicon macOS path must fail closed unless the CLI binary is Developer-ID signed, notarized, Gatekeeper-assessed, and accompanied by a signed, notarized, and stapled `.pkg` installer [7].

## References

[1] GitHub, "About protected branches," GitHub Docs. [Online]. Available: https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches. [Accessed: May 15, 2026].

[2] GitHub, "Approving workflow runs from forks," GitHub Docs. [Online]. Available: https://docs.github.com/en/actions/managing-workflow-runs-and-deployments/managing-workflow-runs/approving-workflow-runs-from-private-forks. [Accessed: May 15, 2026].

[3] GitHub, "Billing and usage," GitHub Docs. [Online]. Available: https://docs.github.com/actions/reference/usage-limits-billing-and-administration. [Accessed: May 15, 2026].

[4] GitHub, "Secure use reference," GitHub Docs. [Online]. Available: https://docs.github.com/en/actions/reference/security/secure-use. [Accessed: May 17, 2026].

[5] Depot, "Container builds in GitHub Actions," Depot Documentation. [Online]. Available: https://depot.dev/docs/container-builds/integrations/github-actions. [Accessed: May 16, 2026].

[6] Docker, "Validating build configuration with GitHub Actions," Docker Docs. [Online]. Available: https://docs.docker.com/build/ci/github-actions/checks/. [Accessed: May 16, 2026].

[7] Apple, "Notarizing macOS software before distribution," Apple Developer Documentation. [Online]. Available: https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution. [Accessed: May 16, 2026].
