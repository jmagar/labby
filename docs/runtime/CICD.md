# CI/CD

Last updated: 2026-05-01

This document is the authoritative contract for CI, release, and artifact delivery in `lab`. All pipeline implementations must conform to this spec.

## CI Checks

Every push and pull request must pass all of the following:

| Check | Command |
|-------|---------|
| Workflow lint | `actionlint` over `.github/workflows/` |
| Frontend build | `./.github/actions/build-gateway-admin` (`pnpm install --frozen-lockfile && pnpm build` in `apps/gateway-admin`) |
| Compile | `cargo check --workspace --all-features` |
| Generated docs freshness | `just docs-check` |
| Format | `cargo fmt --all -- --check` |
| Lint | `cargo clippy --workspace --all-features -- -D warnings` |
| Deny | `cargo deny check` |
| Tests | `cargo nextest run --workspace --all-features --profile ci` |
| Tests (Windows) | same nextest run on the self-hosted `agent-os-lab` Windows runner; skipped on PRs (fork code must not reach a self-hosted runner) — runs on pushes to main, the weekly schedule, and manual dispatch |
| Release smoke | `cargo build --workspace --all-features --release` — Linux on every run; Windows only on pushes to main, the weekly schedule, and manual dispatch (skipped on PRs: 20-25 min runner time, and Linux cross-checking is blocked by aws-lc-sys needing a Windows C toolchain) |
| Container smoke | Docker build using `config/Dockerfile` |

Clippy runs with `-D warnings` — zero warnings are permitted. This is enforced at the workspace lint layer.

The frontend build is required because the Rust binary embeds the exported
Labby assets. It is a production build gate, not a TypeScript strictness gate:
`apps/gateway-admin/next.config.mjs` currently sets
`typescript.ignoreBuildErrors = true`. Run `pnpm test` in
`apps/gateway-admin` for the frontend unit/ACP test contract.

## CI Platform

- **Provider:** GitHub Actions
- **Manual runs:** `CI` and `Release` both support `workflow_dispatch`
- **Scheduled runs:** `CI` runs weekly on Monday at 09:23 UTC to keep
  dependency/advisory visibility fresh even when no PR is active
- **Job split:**
  - Frontend assets build once, then Rust compile/lint/test jobs download the exported `apps/gateway-admin/out` artifact
  - Fast checks (actionlint, frontend-assets, check, fmt, clippy, deny, test, release-smoke, container) on every push and PR
  - Release builds on `vX.Y.Z` tags only
  - Container image publishing and GitHub Release publishing after successful tag builds

## Build Matrix

| Platform | Target |
|----------|--------|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

Windows is a supported platform. Official Windows release artifacts are built
on native GitHub-hosted Windows runners using the MSVC target. Linux-to-Windows
GNU cross-compilation may be useful experimentally, but it is not the release
support contract.

## Integration Tests

Live service integration tests are **excluded from CI**. They require real service instances and are run locally only.

```bash
# Local only — never runs in CI
just test-integration
```

Integration tests must be marked `#[ignore]` so `cargo nextest run` skips them without explicit opt-in.

## Release Process

1. Bump version with `cargo-release` (single workspace version)
2. `cargo-release` tags the commit `vX.Y.Z` and pushes
3. The `vX.Y.Z` tag triggers the release CI job
4. Release job builds frontend assets once and reuses them for each target build
5. Release job builds the container image from `config/Dockerfile` and pushes it to GHCR
6. GitHub generates release notes from the tag diff
7. Binary archives and checksum files are published to GitHub Releases

**Tag format:** `vX.Y.Z` — no other formats are accepted.

**Version policy:** single version across the entire workspace. `lab` and `lab-apis` always share the same version number.

## Artifact Distribution

- **Surface:** GitHub Releases
- **Container surface:** GitHub Container Registry (`ghcr.io/jmagar/lab`)
- **Artifacts per release:** one binary archive per supported target (Linux x86_64, Windows x86_64; aarch64 dropped deliberately — rquickjs-sys does not cross-compile and no fleet host is ARM)
- **Checksums:** every binary archive has a SHA-256 checksum file
- **No package registry publishing** (crates.io, npm, etc.) unless explicitly decided

## Test Reports

CI uses the `ci` nextest profile in `.config/nextest.toml`. The test job
uploads `target/nextest/ci/junit.xml` as the `nextest-junit` artifact with
short retention so failed runs can be inspected without scraping logs.

## Cargo Deny Advisories

`deny.toml` keeps unmaintained advisory checks enabled. Any ignored advisory
must include a dependency-path comment and should be removed once the upstream
dependency path is gone. The weekly scheduled CI run keeps those exceptions
visible even if no pull request touches dependency policy.

## Size Policy

Binary size is tracked but not hard-gated in CI unless repo tooling enforces a monolith size limit. If a size gate is added, it runs in the fast check job.

## Frontend Tests

Gateway-admin tests are local/developer verification today. They are not part
of `ci.yml`.

```bash
cd apps/gateway-admin
pnpm test
pnpm test:acp
pnpm test:browser
```

## Non-Goals

- no telemetry pipeline
- no background analytics
- no phone-home behavior in any CI or release step
