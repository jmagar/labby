# ADR 0008: Execute Extraction with Isolated Lanes and Integration Ownership

Date: 2026-05-26

Status: Accepted

## Context

Crate and package extraction touches shared workspace manifests, global
registries, global routers, global app state, serve orchestration, frontend
package manifests, and product-specific runtime files. Parallel work can reduce
elapsed time, but uncontrolled parallel edits will move conflict resolution into
the critical path.

## Decision

Execute extraction with one branch/worktree per lane and a dedicated integration
lane for shared wiring.

Product and shared-platform lanes own their local crate/package APIs, tests, and
surface fragments. The integration lane owns shared choke points such as:

- workspace/root `Cargo.toml`
- `crates/lab/Cargo.toml`
- `crates/lab/src/lib.rs`
- `crates/lab/src/main.rs`
- `crates/lab/src/registry.rs`
- `crates/lab/src/api/router.rs`
- `crates/lab/src/api/state.rs`
- `crates/lab/src/cli.rs`
- `crates/lab/src/cli/serve.rs`
- frontend root/package-manager lockfiles, if introduced
- CI workflow files

Merge completed lanes one at a time. Prefer shared platform lanes first,
smaller product lanes before highly coupled product lanes, frontend lanes after
the first product REST/OpenAPI contract is stable, and standalone binaries last.

## Consequences

- Worktrees avoid live file contention but do not remove merge contention.
- Lane write scopes must remain explicit.
- Product lanes should not wire themselves into the global binary/router unless
  assigned integration ownership for that wave.
- Integration verification is the authoritative signal after merges.

## References

- `docs/crate-extract/execution-strategy.md`
- `docs/crate-extract/inventory.md`
- `docs/crate-extract/migration-roadmap.md`
