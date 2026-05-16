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
