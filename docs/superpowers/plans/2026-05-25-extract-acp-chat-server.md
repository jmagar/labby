# ACP Chat Server Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a fresh rmcp-template clone for ACP Chat with CLI, MCP, HTTP API, SSE/session runtime, permission approval, and an embedded chat web UI.

**Architecture:** Start from `/home/jmagar/workspace/rmcp-template`, rename it into an ACP Chat platform server, then port Lab's ACP runtime, dispatch, API routes, and chat UI. ACP Chat consumes Marketplace Registry provider data over HTTP/API or a static provider snapshot; it does not compile Marketplace Registry implementation details.

**Tech Stack:** Rust 2024, rmcp, axum, SSE, tokio, serde, agent-client-protocol with Lab's vendored patch initially preserved, Next.js/React/TypeScript, Vitest, Playwright/browser tests, cargo-nextest.

---

## Beads

- Epic: `lab-hjhnu`
- Child: `lab-hjhnu.3`

## Target Repo

- Create fresh clone: `/home/jmagar/workspace/lab-acp-chat`
- Proposed binary: `acp-chat`
- Proposed env prefix: `ACP_CHAT`
- Proposed default port: `40130`
- Proposed service scopes: `chat:read`, `chat:write`, `chat:admin`
- Appdata root: `~/.acp-chat`

## Source Inventory

Copy or adapt backend code from:

- `crates/lab/src/acp.rs`
- `crates/lab/src/acp/**`
- `crates/lab/src/dispatch/acp.rs`
- `crates/lab/src/dispatch/acp/**`
- `crates/lab/src/api/services/acp.rs`
- `crates/lab-apis/src/acp.rs`
- `crates/lab-apis/src/acp/**`
- `crates/vendor/agent-client-protocol/**`

Copy or adapt web code from:

- `apps/gateway-admin/app/(admin)/chat/page.tsx`
- `apps/gateway-admin/components/chat/**`
- `apps/gateway-admin/components/ai/**`
- `apps/gateway-admin/components/floating-chat-*.tsx`
- `apps/gateway-admin/lib/acp/**`
- `apps/gateway-admin/lib/chat/**`
- `apps/gateway-admin/components/page-context-sync.tsx`
- Minimal shared UI, auth, theme, and request helpers.

Copy or adapt docs from:

- `docs/acp/README.md`
- `docs/acp/design.md`
- `docs/acp/research-findings.md`
- `docs/acp/chat-session-persistence-investigation-2026-05-05.md`
- `docs/superpowers/specs/2026-04-23-acp-chat-service-architecture.md`
- `docs/superpowers/plans/2026-04-22-acp-chat-bridge-implementation.md`
- `docs/superpowers/plans/2026-04-23-acp-chat-ui-implementation.md`
- `docs/superpowers/plans/2026-05-24-chat-page-polish-sweep.md`

## Task 1: Clone And Rename rmcp-template

- [ ] Create the fresh clone:

```bash
cd /home/jmagar/workspace
git clone /home/jmagar/workspace/rmcp-template lab-acp-chat
cd lab-acp-chat
```

- [ ] Rename crate, binary, plugin, Docker, and service identifiers from `example` to `acp-chat`.
- [ ] Rename env vars from `EXAMPLE_*` to `ACP_CHAT_*`.
- [ ] Set the default HTTP port to `40130`.
- [ ] Replace example scopes with `chat:read`, `chat:write`, and `chat:admin`.
- [ ] Verify plugin manifests do not contain `version`.
- [ ] Run:

```bash
cargo fmt
cargo check
```

## Task 2: Port ACP Runtime And Persistence

- [ ] Copy Lab's vendored `agent-client-protocol` patch into the new repo and preserve the workspace `[patch.crates-io]` entry.
- [ ] Document the patch and the removal condition in the new repo's `CLAUDE.md` and docs.
- [ ] Create `src/acp/` and port provider, runtime, registry, persistence, and type modules.
- [ ] Create `src/chat/` or `src/app.rs` service methods for session lifecycle, provider health, model discovery, page context sync, permission decisions, and event streaming.
- [ ] Ensure all persistent state writes under `~/.acp-chat`.
- [ ] Add tests for session create/resume, provider health, permission decision persistence, and event replay.

## Task 3: Rebuild CLI, MCP, API, And SSE Surfaces

- [ ] Add CLI commands for provider list/health, session list/create/get/delete, message send, permission approve/deny, and page context update.
- [ ] Add one MCP tool named `acp_chat` with flat action dispatch and `help`/`schema` actions.
- [ ] Add HTTP routes equivalent to Lab's ACP service routes.
- [ ] Add authenticated SSE endpoint for session events.
- [ ] Add session ownership checks for all session reads and writes.
- [ ] Add structured error tests for unknown provider, missing session, denied permission, stale SSE ticket, and malformed page context.
- [ ] Add parity tests proving core actions are reachable from CLI, MCP, and API.

## Task 4: Consume Marketplace Registry Provider Catalog

- [ ] Add config for `ACP_CHAT_REGISTRY_URL`.
- [ ] Add an HTTP client for Marketplace Registry provider catalog.
- [ ] Support a local static provider snapshot for development and offline use.
- [ ] Keep provider install/curation out of ACP Chat; link to Marketplace Registry for those actions.
- [ ] Add tests for remote catalog success, remote catalog unavailable, and static fallback.

## Task 5: Extract Product Web UI

- [ ] Replace the template web app with an ACP Chat-focused app.
- [ ] Copy chat page, chat components, AI artifact components, floating chat components, ACP clients, chat hooks, page-context sync, and minimal shared UI/theme/auth helpers.
- [ ] Replace Lab navigation with Sessions, Providers, Permissions, Activity, and Settings.
- [ ] Ensure permission approval controls are first-class and never hidden behind raw event inspectors.
- [ ] Wire the web UI to standalone ACP Chat API and SSE paths.
- [ ] Add browser tests for session start, message send, permission prompt, session resume, mobile layout, and disconnected-provider state.
- [ ] Run:

```bash
pnpm --dir apps/web test
pnpm --dir apps/web build
```

## Task 6: Import Existing Lab ACP Chat State

- [ ] Add `acp-chat import lab --lab-home ~/.labby --dry-run`.
- [ ] Import ACP sessions, provider settings, persisted events, permission decisions, and page-context records into `~/.acp-chat`.
- [ ] Make import idempotent and safe to run repeatedly.
- [ ] Verify no post-import code path writes to `~/.labby`.

## Task 7: Standalone Runtime Smoke

- [ ] Start the standalone ACP Chat service with loopback no-auth mode.
- [ ] Verify `/health` responds from the standalone service.
- [ ] Call provider list, provider health, session create, and session read actions through CLI, MCP, API, SSE, and the web UI where applicable.
- [ ] Confirm the standalone service reads and writes only `~/.acp-chat` and `ACP_CHAT_*` configuration.

## Verification

- [ ] ACP Chat repo:

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
ACP_CHAT_MCP_NO_AUTH=true ACP_CHAT_MCP_HOST=127.0.0.1 just dev
curl -sf http://127.0.0.1:40130/health
```

## Acceptance Criteria

- Fresh `lab-acp-chat` clone builds and serves an embedded ACP Chat web UI.
- CLI, MCP, API, SSE, and web UI expose equivalent ACP Chat workflows.
- Session ownership, permission approval, SSE auth, and process boundaries are verified.
- ACP Chat uses Marketplace Registry provider data over HTTP/API or static snapshot, not compile-time coupling.
- Service uses separate `ACP_CHAT_*` env vars and `~/.acp-chat` state.
- Lab remains unchanged; the new repo contains only the ACP Chat code copied for standalone operation.
