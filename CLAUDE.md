# Labby Development Instructions

Labby is a Rust MCP gateway and operator control plane. One product exposes CLI, MCP, HTTP API, and the web UI over shared dispatch semantics. The canonical repository is `dinglebear-ai/labby`.

Use [docs/README.md](docs/README.md) as the product-documentation index. Generated catalogs under `docs/generated/` are the authoritative snapshots for registered services, actions, environment variables, API routes, MCP help, CLI help, and Cargo feature posture.

`docs/sessions/` and `docs/superpowers/` are tracked historical/work-product trees and are **hands-off during normal documentation audits and cleanup**. Do not edit, retire, relocate, link-audit, or otherwise tidy them unless the user explicitly asks for those trees. Pull requests that intentionally change either protected tree also require the maintainer-applied `protected-docs-approved` label. `docs/references/` is the intentionally untracked external-reference cache.

## Current Product Shape

The current registered services are `artifacts`, `bundles`, `doctor`, `fs`, `gateway`, `jobs`, `lab_admin`, `server_logs`, `setup`, `snippets`, `sources`, and `uploads`. On Linux, the current registry also exposes principal-scoped `stash`; unsupported platforms omit it because the required descriptor-relative filesystem primitives are unavailable. The direct stdio MCP proxy is a CLI product surface backed by the gateway runtime.

ACP chat, standalone Marketplace/MCP Registry products, Fleet/device runtime, Deploy-product, and the old Agent Artifact Manager named Stash are retired and deleted. The new File Stash does not restore components, revisions, workspaces, providers, deploy targets, Marketplace forks, or drift detection. Provider-backed Artifact discovery may project bounded ACP, Marketplace, and MCP Registry results through the `artifacts` control-plane service; that does not restore the retired standalone products.

The current CLI surface is generated in `docs/generated/cli-help.md`. Do not hand-maintain command inventories here.

## Workspace Boundaries

The workspace has 11 members:

| Crate | Responsibility |
| --- | --- |
| `labby-primitives` | dependency-leaf shared action/plugin/MCP/SSRF vocabulary |
| `labby-apis` | pure setup/doctor SDK contracts and shared HTTP primitives |
| `labby-auth` | inbound auth plus reusable upstream OAuth/JWT/session behavior |
| `labby-codemode` | host-neutral bounded Javy/QuickJS Code Mode runtime |
| `labby-gateway` | surface-neutral upstream MCP gateway runtime |
| `labby-openapi` | OpenAPI ingestion/projection helpers |
| `labby-runtime` | surface-neutral shared runtime contracts/helpers |
| `labby-web` | static web asset embedding/resolution/header helpers |
| `labby-winjob` | Windows process containment and verified filesystem primitives; sanctioned unsafe boundary |
| `labby` | product binary/library, dispatch, CLI, MCP, HTTP API, setup, local services |
| `xtask` | repository build/maintenance tasks |

External capabilities should normally be configured as upstream MCP servers, not added as new built-in Labby SDK modules. A new built-in service is appropriate only when Labby owns the local state or lifecycle. See [docs/dev/SERVICE_ONBOARDING.md](docs/dev/SERVICE_ONBOARDING.md).

## Feature Contract

`crates/labby/Cargo.toml` is the source of truth:

- `default = ["gateway-host"]`
- `gateway-host = ["gateway"]`
- `all = ["lab-admin", "api-docs", "gateway-host", "fs", "systemd", "skills"]`
- `proxy-testkit` is test-only support, not a product slice.

Retired products are deleted rather than hidden behind feature flags. Product slices must compile with `--no-default-features --features <slice> --all-targets` when the feature contract says they are standalone.

## Architecture Rules

1. **Shared semantics live below surfaces.** Product operation semantics belong in `crates/labby/src/dispatch/` or an extracted surface-neutral crate.
2. **CLI/MCP/API/web are adapters.** Do not duplicate validation, destructive classification, business rules, retries, or error mapping in surface handlers.
3. **Use the lowest correct crate.** Shared vocabulary goes in `labby-primitives`; gateway runtime in `labby-gateway`; Code Mode runtime in `labby-codemode`; reusable auth in `labby-auth`; product wiring in `labby`.
4. **Keep `labby-apis` pure.** It does not read ambient env/files and must not depend on product transports such as clap or rmcp.
5. **No `mod.rs`.** Use `foo.rs` plus sibling `foo/` modules.
6. **Native async traits only.** The workspace Clippy policy bans `#[async_trait]` in project code.
7. **Use bounded upstream listing helpers.** Do not call rmcp's unbounded `Peer::list_all_*` methods; the Clippy policy intentionally bans them.

Boundary-specific instructions live in nested `CLAUDE.md` files. Read the nearest one before editing that area.

## MCP And Gateway Rules

Labby exposes one MCP tool per registered product service using an `action` + `params` request shape. Shared action metadata drives discovery across surfaces.

The `lab://...` resource URI namespace and the `lab:read` / `lab` / `lab:admin` scope names are intentional protocol contracts. Do not rename them as part of the historical Lab → Labby product rename.

Gateway upstreams can be HTTP, Unix-socket, or stdio MCP servers. Code Mode collapses the live upstream catalog behind bounded `search`/`describe`/execution primitives. Never guess upstream tool schemas; discovery comes from the live catalog.

Stdio upstream configuration is admin-gated and protected by the spawn guard. It is not automatically classified as destructive. See [docs/services/GATEWAY.md](docs/services/GATEWAY.md) and [docs/services/UPSTREAM.md](docs/services/UPSTREAM.md).

## Destructive Actions

`requires_admin` and `destructive` are separate axes.

Mark `destructive: true` only for actions that can cause permanent or hard-to-recover loss. A state mutation is not automatically destructive. The canonical meaning is the doc comment on `labby_primitives::action::ActionSpec::destructive`.

Never invent per-surface destructive behavior. MCP elicitation, CLI confirmation, and HTTP policy must derive from shared metadata plus the surface's transport contract.

## Errors And Observability

Use the shared agent-facing error contract and stable kinds documented in:

- [docs/dev/ERRORS.md](docs/dev/ERRORS.md)
- [docs/contracts/agent-error-contract.md](docs/contracts/agent-error-contract.md)
- [docs/contracts/code-mode-tool-errors.md](docs/contracts/code-mode-tool-errors.md)

Do not stringify structured errors early. Preserve typed causes/recovery metadata through dispatch and map at the surface boundary.

Follow [docs/dev/OBSERVABILITY.md](docs/dev/OBSERVABILITY.md) for canonical surface names (`cli`, `mcp`, `api`), correlation, redaction, and required fields. Secrets, authorization values, OAuth material, and raw sensitive parameters must not enter logs or traces.

## Configuration And Auth

Configuration, env precedence, secrets, OAuth, remote-target authority, and deployment behavior are owned by the runtime docs:

- [docs/runtime/CONFIG.md](docs/runtime/CONFIG.md)
- [docs/runtime/ENV.md](docs/runtime/ENV.md)
- [docs/runtime/OAUTH.md](docs/runtime/OAUTH.md)
- [docs/design/REMOTE_GATEWAY_TARGET.md](docs/design/REMOTE_GATEWAY_TARGET.md)

Do not add a second configuration source or silently fall back from an explicitly configured remote target to local state.

## Adding A Built-In Service

Do not start by creating a `labby-apis/<service>` feature. First decide whether the capability should simply be an upstream MCP server.

For a genuine built-in Labby service:

1. define stable vocabulary in the lowest reusable crate that needs it;
2. implement shared semantics in an extracted runtime crate or `crates/labby/src/dispatch/<service>/`;
3. register only supported surfaces;
4. keep adapters thin;
5. add a product service doc under `docs/services/` when user/operator visible;
6. regenerate catalogs;
7. test feature slicing, action metadata, errors, authorization/destructive gates, and every surface.

See [docs/dev/SERVICE_ONBOARDING.md](docs/dev/SERVICE_ONBOARDING.md).

## Development Commands

The pinned `msrv` (1.97.1) is shared by Cargo, `rust-toolchain.toml`, CI, and container contracts. Keep those declarations synchronized.

Use the Justfile as the command source of truth:

```bash
just check
just test
just lint
just docs-generate
just docs-check
just rustdoc-check
just web-build
```

Useful direct checks:

```bash
cargo check --workspace --all-features
cargo nextest run --workspace --all-features
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo fmt --all -- --check
```

Do not declare success from compilation alone. Run focused tests for the behavior changed, then the repository gates appropriate to the affected slice.

## Web UI

`apps/gateway-admin` is the operator web UI and uses the Aurora design system. Reuse existing tokens/components/patterns before introducing new primitives. See its nested `CLAUDE.md` plus [docs/design/design-system-contract.md](docs/design/design-system-contract.md) and [docs/design/component-development.md](docs/design/component-development.md).

`apps/palette-tauri` has separate nested instructions for the launcher/Tauri boundary.

## Plugin Boundary

`plugins/labby` ships plugin metadata, MCP configuration, and skills. It does not ship the Labby binary and does not own host bootstrap. The binary owns setup/repair. The retired automatic Claude Code hooks must not be reintroduced. See [docs/PLUGINS.md](docs/PLUGINS.md).

## Documentation Discipline

`CLAUDE.md` is the instruction source of truth. Every directory that has a `CLAUDE.md` must expose sibling `AGENTS.md` and `GEMINI.md` symlinks pointing to `CLAUDE.md`.

Product documentation must describe current code, not old plans or session state. Use [docs/README.md](docs/README.md) for the canonical product-doc map. Regenerate `docs/generated/` instead of editing generated artifacts by hand.
