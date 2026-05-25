# Marketplace Registry Server Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a fresh rmcp-template clone for a unified ACP/MCP Registry plus Claude/Codex Marketplace server with CLI, MCP, HTTP API, and embedded web UI.

**Architecture:** Start from `/home/jmagar/workspace/rmcp-template`, rename it into a Marketplace Registry platform server, then port Lab's marketplace, MCP Registry, and ACP Registry code into one product. Marketplace Registry owns provider/catalog curation; ACP Chat consumes catalog data by HTTP/API instead of compile-time coupling.

**Tech Stack:** Rust 2024, rmcp, axum, reqwest, serde, tokio, rusqlite/r2d2_sqlite, diffy-imara, Next.js/React/TypeScript, Vitest, cargo-nextest.

---

## Beads

- Epic: `lab-hjhnu`
- Child: `lab-hjhnu.2`

## Target Repo

- Create fresh clone: `/home/jmagar/workspace/lab-marketplace-registry`
- Proposed binary: `marketplace-registry`
- Proposed env prefix: `MARKETPLACE_REGISTRY`
- Proposed default port: `40120`
- Proposed service scopes: `marketplace:read`, `marketplace:write`, `registry:read`, `registry:write`
- Appdata root: `~/.marketplace-registry`

## Source Inventory

Copy or adapt backend code from:

- `crates/lab/src/dispatch/marketplace.rs`
- `crates/lab/src/dispatch/marketplace/**`
- `crates/lab-apis/src/marketplace.rs`
- `crates/lab-apis/src/marketplace/**`
- `crates/lab-apis/src/mcpregistry.rs`
- `crates/lab-apis/src/mcpregistry/**`
- `crates/lab-apis/src/acp_registry.rs`
- `crates/lab-apis/src/acp_registry/**`
- `crates/lab/src/api/services/marketplace.rs`
- `crates/lab/src/api/services/registry_v01.rs`
- `crates/lab/src/cli/marketplace.rs`
- `crates/lab/src/cli/marketplace/**`
- `crates/lab/src/cli/mcpregistry.rs`

Copy or adapt web code from:

- `apps/gateway-admin/app/(admin)/marketplace/**`
- `apps/gateway-admin/app/(admin)/registry/page.tsx`
- `apps/gateway-admin/components/marketplace/**`
- `apps/gateway-admin/components/registry/**`
- `apps/gateway-admin/lib/api/marketplace-client.ts`
- `apps/gateway-admin/lib/api/mcpregistry-client.ts`
- `apps/gateway-admin/lib/hooks/use-marketplace.ts`
- `apps/gateway-admin/lib/hooks/use-registry.ts`
- `apps/gateway-admin/lib/types/marketplace.ts`
- `apps/gateway-admin/lib/types/registry.ts`
- Minimal shared UI, auth, theme, and request helpers.

Copy or adapt docs from:

- `docs/services/MARKETPLACE.md`
- `docs/services/MCPREGISTRY_METADATA.md`
- `docs/features/marketplace-v2-design.md`
- `docs/coverage/mcpregistry.md`
- `docs/coverage/acp_registry.md`
- `docs/superpowers/plans/2026-05-07-marketplace-gateway.md`
- `docs/superpowers/plans/2026-04-21-mcp-registry-normalization.md`

## Task 1: Clone And Rename rmcp-template

- [ ] Create the fresh clone:

```bash
cd /home/jmagar/workspace
git clone /home/jmagar/workspace/rmcp-template lab-marketplace-registry
cd lab-marketplace-registry
```

- [ ] Rename crate, binary, plugin, Docker, and service identifiers from `example` to `marketplace-registry`.
- [ ] Rename env vars from `EXAMPLE_*` to `MARKETPLACE_REGISTRY_*`.
- [ ] Set the default HTTP port to `40120`.
- [ ] Replace example scopes with marketplace and registry scopes.
- [ ] Verify plugin manifests do not contain `version`.
- [ ] Run:

```bash
cargo fmt
cargo check
```

## Task 2: Port Registry And Marketplace Domain Logic

- [ ] Create `src/marketplace/` and port Lab's marketplace dispatch modules.
- [ ] Create `src/registry/mcp.rs` and `src/registry/acp.rs` from Lab MCP Registry and ACP Registry clients/types.
- [ ] Keep catalog storage, source import, package diff/patch/update, and backend adapters in service/domain modules.
- [ ] Keep CLI, MCP, and API handlers as thin shims over service/domain logic.
- [ ] Preserve stable action names for plugin marketplace, MCP Registry, ACP Registry, managed sources, fork/patch/update, and generated wrapper planning.
- [ ] Preserve path validation, source allowlisting, provenance, source trust, and destructive confirmation.
- [ ] Port store schema and add migrations under the new appdata root.

## Task 3: Rebuild CLI, MCP, And API Surfaces

- [ ] Add CLI subcommands equivalent to Lab marketplace and mcpregistry commands.
- [ ] Add one MCP tool named `marketplace_registry` with flat action dispatch and `help`/`schema` actions.
- [ ] Add HTTP routes for marketplace list/search/detail/install/fork/update and registry list/search/install.
- [ ] Add API route for ACP provider catalog export that ACP Chat can consume.
- [ ] Add parity tests for read actions, install preview, managed source writes, fork/patch/update actions, and registry searches.
- [ ] Add structured error tests for invalid source URL, unsafe path, unknown package, and destructive action without confirmation.

## Task 4: Extract Product Web UI

- [ ] Replace the template web app with a Marketplace Registry-focused app.
- [ ] Copy marketplace pages, plugin detail pages, registry page, marketplace components, registry components, clients, hooks, and minimal shared UI/theme/auth helpers.
- [ ] Replace Lab navigation with Marketplace, MCP Registry, ACP Registry, Sources, Managed Catalogs, and Settings.
- [ ] Ensure install/fork/update buttons use confirmation dialogs and show provenance before write actions.
- [ ] Wire the web UI to the standalone API paths.
- [ ] Run:

```bash
pnpm --dir apps/web test
pnpm --dir apps/web build
```

## Task 5: Import Existing Lab Marketplace State

- [ ] Add `marketplace-registry import lab --lab-home ~/.lab --dry-run`.
- [ ] Import marketplace sources, MCP Registry cache, ACP Registry cache, managed marketplace files, forks, patches, and update metadata into `~/.marketplace-registry`.
- [ ] Preserve raw source entries and version-source metadata.
- [ ] Make import idempotent and safe to run repeatedly.
- [ ] Verify no post-import code path writes to `~/.lab`.

## Task 6: Provide ACP Chat Catalog Contract

- [ ] Define a stable response for `GET /api/registry/acp/providers`.
- [ ] Include provider id, display name, command metadata, supported models if known, install state, and provenance.
- [ ] Add static export command for offline provider snapshot generation.
- [ ] Document ACP Chat consumption rules and fallback behavior.

## Task 7: Standalone Runtime Smoke

- [ ] Start the standalone Marketplace Registry with loopback no-auth mode.
- [ ] Verify `/health` responds from the standalone service.
- [ ] Call marketplace list, MCP Registry search, and ACP provider catalog actions through CLI, MCP, API, and the web UI.
- [ ] Confirm the standalone service reads and writes only `~/.marketplace-registry` and `MARKETPLACE_REGISTRY_*` configuration.

## Verification

- [ ] Marketplace Registry repo:

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo nextest run
cargo build --release
pnpm --dir apps/web test
pnpm --dir apps/web build
just build-web
just build-full
```

- [ ] Runtime smoke:

```bash
MARKETPLACE_REGISTRY_MCP_NO_AUTH=true MARKETPLACE_REGISTRY_MCP_HOST=127.0.0.1 just dev
curl -sf http://127.0.0.1:40120/health
```

## Acceptance Criteria

- Fresh `lab-marketplace-registry` clone builds and serves an embedded Marketplace Registry web UI.
- CLI, MCP, API, and web UI expose equivalent marketplace and registry actions.
- ACP Chat can consume provider catalog data over HTTP/API.
- Destructive install/update/fork actions retain confirmation and provenance.
- Service uses separate `MARKETPLACE_REGISTRY_*` env vars and `~/.marketplace-registry` state.
- Lab remains unchanged; the new repo contains only the marketplace and registry code copied for standalone operation.
