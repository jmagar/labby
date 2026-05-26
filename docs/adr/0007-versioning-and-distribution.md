# ADR 0007: Use Semver with Workspace-First Extraction and Git Tags

Date: 2026-05-26

Status: Accepted

## Context

Lab extraction needs package boundaries before it needs public package
distribution. Publishing too early would freeze unstable APIs, but future
products still need versioned dependencies once a boundary is reusable.

Research also found an npm caveat: direct git dependencies must resolve to a
real package root. npm does not install subpackages buried inside a git
workspace as independent packages.

## Decision

Start extraction as internal workspace crates and packages. Use semver for all
extracted Rust crates and TypeScript packages.

During active extraction, path dependencies are allowed inside development
workspaces. External consumers should eventually use versioned git tags or
published packages.

Publishing to `crates.io` or npm is optional and not required for first reuse.
If frontend packages are consumed through git dependencies, each dependency must
resolve to a repository/package root with its own `package.json`, or the project
must use a workspace-aware release flow that publishes/packages each dependency
explicitly.

REST APIs stay under explicit versions such as `/v1`. OpenAPI documents and MCP
action-contract manifests carry versions because they are separate surfaces.

## Consequences

- Boundary stability is proven before repository splits or public publishing.
- Breaking changes to REST routes, response shapes, auth requirements, MCP
  action params, package exports, or dependency direction require a major
  version bump or compatibility alias.
- The externalization decision is deferred until in-repo boundaries pass tests
  and have consumer fixtures.

## References

- `docs/crate-extract/spec.md`
- `docs/crate-extract/contract.md`
- `docs/crate-extract/research.md`
- `docs/crate-extract/open-questions.md`
- `docs/crate-extract/migration-roadmap.md`
