Follow strict SOLID principles across the agentactr core, agentactr SDK, and all Rust CLI implementations.

The @specs_agentactrSDK.md specification is the architectural source of truth and must be updated whenever architectural corrections or contract changes are introduced.

README.md is the living operator document for the present repository state. Whenever source code, command behavior, architecture boundaries, memory behavior, tracker behavior, or spec diagrams change, README.md must stay synchronized with the code as source of truth and must embed the exact relevant diagrams from internal_readme/. Spec-affecting changes must also update and embed the exact relevant diagrams under internal_specs_agentactrSDK/svgs/. Diagrams must be updated or added with the documentation change rather than left stale.

Enforce:

- Hexagonal/Clean Architecture

- Dependency Inversion

- Interface segregation

- Explicit domain boundaries

- Transport isolation

- Async-safe Rust patterns

- Strong typing over dynamic behavior

- Deterministic error handling

- Structured observability

- Repository/service separation

- Configuration-driven composition

- Testability-first abstractions

- Secure-by-default implementations

Default implementations must remain compatible with:

- Codex

- App Server

- Codex SDK

- GitHub integrations

Before introducing architectural decisions, refactors, dependency changes, or protocol changes:

- Verify through MCP

- Verify through SKILLs

- Verify through authoritative Web Search where required

Prioritize:

- Maintainability

- Extensibility

- Explicit contracts

- Enterprise production readiness

Rust workspace hardening rules:

- Pin the Rust toolchain and declared MSRV intentionally.

- Prefer workspace-owned dependency versions over per-crate drift.

- Keep clippy, rustdoc, formatting, test, nextest, cargo-deny, and cargo-machete gates synchronized across local hooks, CI, README.md, and specs_agentactrSDK.md.

- Keep cargo-vet, cargo-audit, cargo-deny, and dependency-license/advisory policy synchronized across CI, local validation docs, and release readiness. New dependencies, new default features, yanked replacements, advisory ignores, license exceptions, and unaudited transitive risk must be reviewed intentionally and documented with the narrowest possible justification.

- Security-sensitive Rust surfaces must have an explicit advanced-test posture. Add or maintain fuzz targets for parsers, protocol handlers, artifact readers, redaction, path validation, and ledger/state replay logic when those surfaces change. Run Miri for unsafe or aliasing-sensitive code where feasible. Use sanitizer-backed jobs for FFI, process supervision, filesystem boundary handling, and memory-sensitive code paths where platform support exists.

- Unsafe Rust must be minimal, justified with local safety comments, and guarded by lint policy.

- Public SDK/core contracts must move toward typed errors instead of stringly error surfaces.

- Large CLI or adapter files should be split only along existing domain boundaries and SDK/adapter ownership seams.

Supply-chain and CI hardening rules:

- GitHub Actions and reusable workflows must be pinned to immutable commit SHAs. Version tags may be included only as comments or metadata for readability; execution must not depend on mutable tags, branches, or floating major versions.

- Workflow permissions must be least-privilege by job. Untrusted pull-request and merge-queue workflows must not receive repository secrets, cloud credentials, write tokens, privileged OIDC trust, release credentials, package-publish credentials, or remote-builder secrets.

- Dependabot version updates and security updates must remain enabled for Cargo dependencies and GitHub Actions. Dependency-review gates must block vulnerable, disallowed-license, unexpected-source, or unpinned-action changes unless an explicit reviewed exception is recorded.

- Release artifacts must have SBOM and provenance expectations. Container images, binaries, and package artifacts must publish or retain machine-readable SBOMs and tamper-evident provenance. Release-sensitive workflows should move toward SLSA-aligned build provenance, isolated builders, pinned inputs, and reproducible build metadata.

- Third-party actions, containers, install scripts, and binary downloads must be treated as supply-chain inputs. Prefer official sources, immutable digests or SHAs, checksum verification, signature/provenance verification where available, and narrowly scoped credentials.

Generated AGENTS.md rules:

- `agentactr doctor --fix-agents` and AGENTS template rendering must reflect the selected/configured repository stack, not only filesystem detection.

- Blank or new projects with `repository.declared_primary_stack` set must render that declared stack in generated AGENTS.md.

- Existing AGENTS.md wins: never overwrite it by default; write reviewable artifacts unless the operator explicitly requests replacement.

Remote build services such as depot.dev are trusted and preferred over local machine Docker builds for expensive or release-sensitive image build work when the workflow context is trusted. Keep repository/action variables and secrets out of untrusted pull-request and merge-queue workflows; use remote builders only from trusted workflow contexts or with explicitly non-secret validation paths.

# Citation & Reference Policy

All external references MUST use IEEE citation format unless explicitly overridden.

## General Rules

- Prefer official vendor documentation over third-party sources.
- Prefer versioned documentation URLs whenever available.
- Always include:
  - organization/author,
  - exact document/page title,
  - publication/version date if available,
  - stable URL,
  - accessed date.

- Avoid:
  - shortened URLs,
  - homepage-only references,
  - unofficial blogs when official docs exist,
  - mutable "latest" links without version context.

---

# IEEE Citation Templates

## Official API Documentation

Format:

[1] Organization, "Document/Page Title," Product or Documentation Name. [Online]. Available: URL. [Accessed: Month Day, Year].

Example:

[1] OpenAI, "Responses API Documentation," OpenAI Platform Docs. [Online]. Available: https://platform.openai.com/docs/api-reference/responses. [Accessed: May 15, 2026].

---

## Product Documentation

Format:

[2] Vendor, "Document Title," Product Documentation, version X.Y. [Online]. Available: URL. [Accessed: Month Day, Year].

Example:

[2] PostgreSQL Global Development Group, "Row Security Policies," PostgreSQL 18 Documentation. [Online]. Available: https://www.postgresql.org/docs/18/ddl-rowsecurity.html. [Accessed: May 15, 2026].

---

## GitHub Repositories

Format:

[3] Organization/User, "Repository Name," GitHub repository. [Online]. Available: URL. [Accessed: Month Day, Year].

Preferred (Pinned Commit):

[3] OpenAI, "Codex CLI," GitHub repository, commit 3f8ab91. [Online]. Available: https://github.com/openai/codex/tree/3f8ab91. [Accessed: May 15, 2026].

---

## Standards / Specifications

Format:

[4] Organization, "Specification Title," version/revision. [Online]. Available: URL. [Accessed: Month Day, Year].

Example:

[4] Khronos Group, "Vulkan 1.4 Specification." [Online]. Available: https://registry.khronos.org/vulkan/specs/1.4/html/vkspec.html. [Accessed: May 15, 2026].

---

# Reference Quality Requirements

Priority order for sources:

1. Official specifications
2. Official vendor documentation
3. Official repositories
4. Standards bodies
5. Peer-reviewed papers
6. Engineering blogs
7. Community discussions

Reddit, StackOverflow, and blogs MUST NOT be used as authoritative references when official documentation exists.

---

# Versioning Requirements

Whenever possible:

- cite explicit documentation versions,
- pin GitHub references to tags or commits,
- avoid floating references such as:
  - /latest/
  - /main/
  - /master/

Preferred:

- tagged releases,
- semantic versions,
- immutable commit SHAs.

---

# Inline Citation Rules

Use IEEE inline numeric references:

- RLS policies are enforced at the database layer [4].
- The Responses API supports tool calling [1].

References MUST appear in a dedicated "References" section.

---

# Documentation Generation Rule

Any generated:

- RFC,
- ADR,
- architecture document,
- README,
- design proposal,
- research report,
- benchmark report,
- implementation guide

MUST include a "References" section if external material is used.

---

# Citation Freshness

For rapidly changing APIs and platforms:

- include accessed dates,
- verify links are live,
- prefer documentation updated within the last 12 months where feasible.
