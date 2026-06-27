# .github/ — CI/CD Workflows

This directory contains the GitHub Actions workflows for `lab`. The authoritative contract lives in `docs/runtime/CICD.md` — this file puts it next to the code.

## Workflows

| File | Trigger | Purpose |
|------|---------|---------|
| `workflows/ci.yml` | push/PR to `main`, weekly schedule, manual dispatch | Correctness, release-smoke, and container-smoke checks |
| `workflows/release.yml` | push of `v*.*.*` tag, manual dispatch | Release builds, container image, and GitHub Release |

## CI Path Routing

`ci.yml` starts with a `changes` job that runs `scripts/ci/changed_paths.py`.
It emits `docs`, `workflow`, `rust`, `web`, `docker`, `security`, and
`release` outputs. Scheduled and manual runs enable every category. Branch
protection should require the stable `ci-gate` job; individual heavyweight jobs
may be skipped when their category is false, and `ci-gate` treats
`success|skipped` as acceptable.

`secret-scan` remains always-on because secrets can be introduced in any path.

## CI Checks (ci.yml)

Every push and PR to `main` must pass `ci-gate`, which covers these jobs when
their category is enabled:

| Job | Category | Command |
|-----|----------|---------|
| secret-scan | always | `gitleaks/gitleaks-action@v3` — full-history secret scan (SAST) with existing historical findings baselined in `.gitleaksignore` |
| actionlint | `workflow` | `go run github.com/rhysd/actionlint/cmd/actionlint@latest` |
| frontend-assets | `rust`, `web`, `docker`, or `release` | `pnpm install --frozen-lockfile && pnpm build` in `apps/gateway-admin` |
| check | `rust` | `cargo check --workspace --all-features` |
| feature-slices | `rust` | `cargo check -p labby --no-default-features --features <slice>` per slice (`gateway`/`marketplace`/`fs`/`deploy`/`acp_registry`) — catches cross-slice coupling (a shared module unconditionally referencing a feature-gated one). Gates compilation only; overrides the global `-D warnings` because per-slice dead-code warnings are expected. |
| extracted-crate-slices | `rust` | crate-specific `cargo check` commands for extracted runtime crates |
| fmt | `rust` | `cargo fmt --all -- --check` |
| clippy | `rust` | `cargo clippy --workspace --all-features -- -D warnings` |
| deny | `security` | `cargo deny check` (via `EmbarkStudios/cargo-deny-action`) |
| docs-check (name: `Generated docs`) | `rust` | `just docs-check` — the generated-docs freshness gate; fails if `docs/generated/*` (action catalog, MCP help, CLI help) drift from the registry. This is the only freshness check; there is **no** standalone `doc-freshness.yml` or `code-conventions.yml` workflow — only `ci.yml` and `release.yml` exist. |
| test | `rust` | `cargo nextest run --workspace --all-features --profile ci` (`self-hosted` `linux-lab` runner for trusted events) |
| test-fork | `rust` | same `cargo nextest` command on `ubuntu-latest` for fork PRs only |
| test-windows | `rust` | same nextest run on the self-hosted `agent-os-lab` runner (label `windows-lab`); fork PRs never reach this runner |
| release-smoke | `release` | `cargo build --workspace --all-features --release` — Windows skipped on PRs (see below) |
| container | `docker` | Docker build with `config/Dockerfile` |

Most jobs run on `ubuntu-latest`. Linux test runs on `linux-lab` for trusted
events and `test-fork` uses `ubuntu-latest` for fork PRs. Windows is a
supported target; the release smoke matrix includes `windows-latest` to prove
the native MSVC release binary builds, but ONLY when the `release` category is
enabled and the event is not a PR. Windows release smoke is skipped on PRs
(20-25 min of runner time per PR, and a Linux cross-check is not viable because
aws-lc-sys requires a real Windows C toolchain even under `cargo check`).
Windows breakage therefore surfaces on the post-merge main run, not in the PR.

`RUSTFLAGS: -D warnings` is set globally — zero warnings permitted. The lone
exception is the `feature-slices` job, which overrides it to `""` because
per-slice dead-code warnings are an inherent, expected consequence of disabling
features; that job gates compilation, not warning-cleanliness.

## Release Build Matrix (release.yml)

Triggered by any tag matching `v*.*.*`, or manually through workflow dispatch.
Manual dispatch builds artifacts but GitHub Release publishing only runs for
matching tag refs.

| Target | Runner | Tool |
|--------|--------|------|
| `x86_64-unknown-linux-gnu` | ubuntu-latest | cargo |
| `x86_64-pc-windows-msvc` | windows-latest | cargo |

Windows uses the native GitHub-hosted Windows runner and the MSVC target.
Linux-to-Windows GNU cross-builds are not the official support contract.
aarch64 was removed from the matrix: rquickjs-sys (Code Mode QuickJS bindings)
fails to cross-compile in the `cross` container (missing target stdbool.h) and
no fleet host is aarch64 and the target was dropped deliberately (wontfix).

Release builds use `--all-features`.

## Release Process

1. Bump version with `cargo-release` (single workspace version — `lab` and `lab-apis` always match)
2. `cargo-release` creates and pushes the `vX.Y.Z` tag
3. Tag push triggers `release.yml`
4. Binary archives and SHA-256 checksum files are uploaded as GitHub Actions artifacts
5. The container image is built from `config/Dockerfile` and pushed to GHCR
6. Artifacts are attached to a GitHub Release via `softprops/action-gh-release`
7. Release notes auto-generated by GitHub (`generate_release_notes: true`)

**Tag format:** `vX.Y.Z` only. No `v*-rc`, no `v*-beta` unless the workflow is updated to handle them.

## Artifacts

| Platform | Archive |
|----------|---------|
| Linux x86_64 | `lab-x86_64-unknown-linux-gnu.tar.gz` |
| Windows x86_64 | `lab-x86_64-pc-windows-msvc.zip` |

Each archive is accompanied by a `.sha256` checksum file. Tag releases also
publish `ghcr.io/jmagar/lab:<tag>` and `ghcr.io/jmagar/lab:latest`.

## Integration Tests

Never run in CI. Marked `#[ignore]` in code. Run locally with `just test-integration`.

See `docs/runtime/CICD.md` for the full CI/CD contract.

## Gotchas

- `cargo-deny-action` reads `deny.toml` at repo root — keep it there
- The weekly scheduled CI run keeps cargo-deny advisory exceptions visible even when no PR is active
- `Swatinem/rust-cache` is on all compile jobs — do not remove it, cold builds are slow
- `fail-fast: false` on matrix jobs — all targets attempt even if one fails
- The release job (`Create Release`) depends on all `build` jobs completing — if any platform fails, no release is created
- The container image job uses `config/Dockerfile`; `config/Dockerfile.fast` is for local builds that already have `bin/lab`
- `permissions: contents: write` is required on `release.yml` for `softprops/action-gh-release` to create the release
- `permissions: packages: write` is required on `release.yml` for GHCR publishing
