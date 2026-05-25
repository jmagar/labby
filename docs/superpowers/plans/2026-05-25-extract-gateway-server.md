# Gateway Server Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a fresh rmcp-template clone for Lab Gateway with CLI, MCP, HTTP API, protected-route OAuth/proxying, and an embedded product web UI.

**Architecture:** Start from `/home/jmagar/workspace/rmcp-template`, rename it into a Gateway platform server, then copy only the Lab Gateway dispatch layer, API route group, CLI shim, and Gateway UI code required for standalone operation. Lab remains intact; this plan does not remove Lab code, add Lab forwarding adapters, or rewire Lab runtime behavior.

**Tech Stack:** Rust 2024, rmcp, axum, tower-http, reqwest, serde, tokio, rusqlite if retained for runtime state, Next.js/React/TypeScript, Vitest, Playwright/browser tests, cargo-nextest.

---

## Beads

- Epic: `lab-hjhnu`
- Child: `lab-hjhnu.1`

## Target Repo

- Create fresh clone: `/home/jmagar/workspace/lab-gateway`
- Proposed binary: `lab-gateway`
- Proposed env prefix: `LAB_GATEWAY`
- Proposed default port: `40110`
- Proposed service scopes: `gateway:read`, `gateway:write`, `gateway:admin`
- Appdata root: `~/.lab-gateway`

## Source Inventory

Copy or adapt backend code from:

- `crates/lab/src/dispatch/gateway.rs`
- `crates/lab/src/dispatch/gateway/**`
- `crates/lab/src/api/services/gateway.rs`
- `crates/lab/src/cli/gateway.rs`
- Gateway action/resource registration from `crates/lab/src/mcp/registry.rs`
- Shared helpers only when required by Gateway behavior.

Copy or adapt web code from:

- `apps/gateway-admin/app/(admin)/gateway/page.tsx`
- `apps/gateway-admin/app/(admin)/gateways/page.tsx`
- `apps/gateway-admin/components/gateway/**`
- `apps/gateway-admin/components/upstream-oauth/**`
- `apps/gateway-admin/lib/api/gateway-*.ts`
- `apps/gateway-admin/lib/hooks/use-gateways.ts`
- `apps/gateway-admin/lib/gateway-protected-route.ts`
- `apps/gateway-admin/lib/server/gateway-*.ts`
- Minimal shared UI, auth, theme, and request helpers from `apps/gateway-admin/components/ui/**`, `components/aurora/**`, `components/auth/**`, and `lib/auth/**`.

Copy or adapt docs from:

- `docs/services/GATEWAY.md`
- `docs/specs/gateway-schema-resources.md`
- `docs/contracts/gateway-schema-resources.md`
- `docs/superpowers/plans/2026-05-09-portable-mcp-gateway.md`
- `docs/superpowers/plans/2026-05-11-gateway-oauth-autodetect.md`

## Task 1: Clone And Rename rmcp-template

- [ ] Create the fresh clone:

```bash
cd /home/jmagar/workspace
git clone /home/jmagar/workspace/rmcp-template lab-gateway
cd lab-gateway
```

- [ ] Rename crate, binary, plugin, Docker, and service identifiers from `example` to `lab-gateway`.
- [ ] Rename env vars from `EXAMPLE_*` to `LAB_GATEWAY_*`.
- [ ] Set the default HTTP port to `40110`.
- [ ] Replace example scopes with `gateway:read`, `gateway:write`, and `gateway:admin`.
- [ ] Verify plugin manifests in `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`, and `gemini-extension.json` do not contain `version`.
- [ ] Run:

```bash
cargo fmt
cargo check
```

## Task 2: Port Gateway Domain Logic

- [ ] Create `src/gateway/` and port the contents of `crates/lab/src/dispatch/gateway/**`.
- [ ] Keep Gateway business logic in `src/app.rs` or `src/gateway/service.rs`; keep `src/mcp/tools.rs`, `src/cli.rs`, and API handlers as shims.
- [ ] Preserve primary action names and UI labels for `scout` and `invoke`; add compatibility aliases only where Lab currently supports them.
- [ ] Port discovery adapters for Claude Code, Claude Desktop, Codex, Cursor, Gemini, OpenCode, VS Code, and Windsurf.
- [ ] Port runtime catalog/import/tombstone behavior and gateway schema resources.
- [ ] Port protected MCP route matching, OAuth metadata/challenge, upstream auth, and proxy behavior.
- [ ] Write focused unit tests for route matching, catalog import idempotency, tombstone behavior, and tool-search ranking.

## Task 3: Rebuild CLI, MCP, And API Surfaces

- [ ] Add Gateway CLI subcommands equivalent to `crates/lab/src/cli/gateway.rs`.
- [ ] Add one MCP tool named `gateway` with flat action dispatch and `help`/`schema` actions.
- [ ] Add HTTP routes equivalent to `crates/lab/src/api/services/gateway.rs`.
- [ ] Add parity tests proving `scout`, `invoke`, `status`, `list`, `get`, protected-route actions, OAuth actions, and cleanup actions are reachable from CLI, MCP, and API.
- [ ] Add structured error tests for missing params, unknown action, auth denial, and upstream failure.

## Task 4: Extract Product Web UI

- [ ] Replace the template web app with a Gateway-focused app.
- [ ] Copy Gateway pages, Gateway components, upstream OAuth components, protected route panels, Gateway clients, hooks, and minimal shared UI/theme/auth helpers.
- [ ] Remove unrelated Lab navigation for marketplace, chat, nodes, docs, setup, and generic service settings.
- [ ] Keep UI controls dense and operational: gateway table, detail view, protected routes, tool exposure, OAuth, cleanup, and test panels.
- [ ] Wire frontend API calls to the new standalone Gateway API paths.
- [ ] Run:

```bash
pnpm --dir apps/web test
pnpm --dir apps/web build
```

## Task 5: Import Existing Lab Gateway State

- [ ] Add `lab-gateway import lab --lab-home ~/.lab --dry-run`.
- [ ] Import Gateway config, runtime catalog entries, protected routes, tombstones, and OAuth metadata into `~/.lab-gateway`.
- [ ] Make import idempotent and safe to run repeatedly.
- [ ] Verify no post-import code path writes to `~/.lab`.

## Task 6: Standalone Runtime Smoke

- [ ] Start the standalone Gateway with loopback no-auth mode.
- [ ] Verify `/health` responds from the standalone service.
- [ ] Call one read-only Gateway action through CLI, MCP, API, and the web UI.
- [ ] Confirm the standalone service reads and writes only `~/.lab-gateway` and `LAB_GATEWAY_*` configuration.

## Verification

- [ ] Gateway repo:

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
LAB_GATEWAY_MCP_NO_AUTH=true LAB_GATEWAY_MCP_HOST=127.0.0.1 just dev
curl -sf http://127.0.0.1:40110/health
```

## Acceptance Criteria

- Fresh `lab-gateway` clone builds and serves an embedded Gateway web UI.
- CLI, MCP, API, and web UI expose equivalent Gateway actions.
- Gateway protected routes remain auth-required outside loopback.
- Gateway uses separate `LAB_GATEWAY_*` env vars and `~/.lab-gateway` state.
- Lab remains unchanged; the new repo contains only the Gateway code copied for standalone operation.
