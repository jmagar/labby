# ADR 0001: Extract Lab as Reusable Rust and TypeScript Packages

Date: 2026-05-26

Status: Accepted

## Context

Lab has grown beyond one CLI/MCP/HTTP binary and one admin app. Gateway, ACP,
Marketplace, Stash, OAuth, Fleet/Nodes, logs, setup, doctor, and the admin web
shell are useful as reusable platform capabilities.

Future products need to consume these capabilities through package dependencies
instead of copying Lab source or depending on the full `labby` application.

## Decision

Extract Lab capabilities into reusable Rust crates, TypeScript packages, and
thin standalone binaries while preserving the current full `labby` binary as a
composition of those same boundaries.

The extraction starts inside the current Lab repository. Moving packages to
separate repositories or publishing them is deferred until the boundaries have
stable APIs, tests, and at least one consumer fixture where appropriate.

## Consequences

- `labby` remains the full product binary during migration.
- New products depend on package APIs instead of vendored source.
- Extraction work must preserve accumulated behavior rather than rewrite
  product runtimes from scratch.
- Package boundaries become architectural contracts, not just folder names.

## References

- `docs/crate-extract/spec.md`
- `docs/crate-extract/contract.md`
- `docs/crate-extract/migration-roadmap.md`
