---
date: 2026-06-11 01:52:35 EST
repo: git@github.com:jmagar/lab.git
branch: claude/objective-ardinghelli-203310
head: 2f2bf442
working directory: /home/jmagar/workspace/lab/.claude/worktrees/objective-ardinghelli-203310
worktree: /home/jmagar/workspace/lab/.claude/worktrees/objective-ardinghelli-203310
beads: lab-2r5br, lab-sohnl
---

# Gateway usage dashboard + metrics backend

## User Request

Turn the gateway-admin overview (landing) page into a usage **dashboard** for the gateway — connected/offline servers, tool counts, agents/devices, tool-call totals/failures, top/least tool, tokens, most-active agent, fan-out, etc. Built frontend-first against mock data for layout approval, then wired the real backend (token instrumentation + aggregation endpoints).

## Session Overview

Shipped a full operator usage dashboard for `apps/gateway-admin` (frontend-first on mock data, drill-downs, analytics, error/loading states, squared Aurora styling) and the backend that feeds it: token logging on every dispatch, plus four `logs.*` aggregation endpoints (`logs.metrics`, `logs.tool_detail`, `logs.agent_detail`, `logs.calls`). Five commits total this session (one is the release commit from `quick-push`). The dashboard still defaults to mock data; the backend endpoints are implemented and unit-tested but not yet verified end-to-end against a live gateway.

## Sequence of Events

1. **Explored** the gateway-admin data layer and the Rust backend telemetry to map what was real vs. needed (gateways/fleet hooks exist; the SQLite log store had no aggregation; tokens were logged only for Code Mode).
2. **Scoping decisions** (via questions): full instrumentation, tokens on all calls, drop dependent-calls metric, frontend-first delivery.
3. **Built the dashboard** on mock data (8-up stats, charts, insight panels, drawers, `/usage` explorer) and iterated per feedback: dismissable warning, faceted Most Active (agent/device/IP), 1×8 layout, compact number+label tiles, typography/spacing/radii polish (retuned `--radius` tokens 6/8/10), then a hardening pass (light-mode toggles, loading skeletons, error/retry states, aggregation unit tests). Committed `727fbcd3`.
4. **Backend Phase 1** — token instrumentation on MCP + API dispatch-completion events; estimators relocated to shared `dispatch::helpers`. Committed `05fe1399`.
5. **Backend Phase 2a** — `logs.metrics` aggregation endpoint over the log store. Committed `d705adf1`.
6. **Backend Phase 2b** — `logs.tool_detail` / `logs.agent_detail` / `logs.calls` drill-down endpoints. Committed `2f2bf442`.
7. **Release pass** (`quick-push`) — minor version bump, CHANGELOG `[0.24.0]`, this session log.

## Key Findings

- **Two log stores.** `crates/lab/src/node/log_store.rs` (`node_logs`, fleet) vs the gateway's own `crates/lab/src/dispatch/logs/store.rs` (`log_events`, rich columns) — the latter is the real `logs.search`/aggregation target.
- **Field mapping** (`dispatch/logs/ingest.rs:552`): only `message/subsystem/surface/action/request_id/session_id/correlation_id/instance/auth_flow/outcome_kind/actor_key` are promoted to columns; everything else (`service`, `tool`, `input_tokens`, `output_tokens`, `elapsed_ms`, `kind`, `call_count`) lands in `fields_json`. `u128` (`elapsed_ms`) is recorded via `Debug` → stored as a string.
- **Completion-event discriminator**: a dispatch-completion log carries **both** `input_tokens` and `output_tokens` in `fields_json`; start events carry only `input_tokens`. Success/failure read from the message suffix (`… ok` / `… error`).
- **Convention gates**: lefthook `aurora-radius` blocks arbitrary `rounded-[…]` (must use `rounded-aurora-*` tokens); clippy `pedantic`+`nursery` on with `-D warnings` (allowed `cast_possible_truncation` but not `cast_precision_loss`).
- **`api -> mcp` is a forbidden dependency** — token estimators had to move from `mcp/result_format` to `dispatch/helpers`.

## Technical Decisions

- **Squaring via tokens, not per-component overrides** — the radius convention forbids arbitrary radii, so `--radius-1/2/3` were retuned to 6/8/10 (app-wide), with dashboard constants aliasing `rounded-aurora-*`.
- **Aggregate in Rust over fetched events** (not SQL `json_extract`) — mirrors the frontend mock exactly, handles percentiles/bucketing trivially; fetch capped at 10k events (acceptable for homelab; SQL is a follow-up).
- **One mock call-stream** drives every frontend metric so totals reconcile across tiles/charts/drawers/explorer; `lib/types/metrics.ts` is the canonical backend contract.
- **Tier-2 metrics return empty/zero honestly** (device/IP facets, truncation/artifacts, new-vs-returning) rather than faking data.

## Files Changed

| status | path | purpose | evidence |
|---|---|---|---|
| created | `apps/gateway-admin/components/dashboard/*` | dashboard UI (stat-tile, window-selector, charts, panels, drawers, metric-bars, error-notice, ui/drill tokens) | commit 727fbcd3 |
| created | `apps/gateway-admin/app/(admin)/usage/page.tsx` | `/usage` call explorer | commit 727fbcd3 |
| modified | `apps/gateway-admin/app/(admin)/page.tsx` | overview → dashboard | commit 727fbcd3 |
| modified | `apps/gateway-admin/app/globals.css` | `--radius` tokens 6/8/10 | commit 727fbcd3 |
| created | `apps/gateway-admin/lib/{types/metrics.ts,api/metrics-client.ts,hooks/*,dashboard/dashboard-metrics.ts}` (+tests) | contract, mock client, hooks, presenters | commit 727fbcd3 |
| modified | `crates/lab/src/dispatch/helpers.rs` | shared token estimators | commit 05fe1399 |
| modified | `crates/lab/src/mcp/{result_format.rs,call_tool.rs,call_tool_upstream.rs}` | log input/output tokens (MCP) | commit 05fe1399 |
| modified | `crates/lab/src/api/services/helpers.rs` | log input/output tokens (API) | commit 05fe1399 |
| modified | `docs/dev/OBSERVABILITY.md` | document token fields | commit 05fe1399 |
| created | `crates/lab/src/dispatch/logs/metrics.rs` (+`metrics/tests.rs`) | aggregation + drill-down types/fns | commits d705adf1, 2f2bf442 |
| modified | `crates/lab/src/dispatch/logs/{logs.rs,catalog.rs,params.rs,dispatch.rs,types.rs}` | wire `logs.metrics`/`tool_detail`/`agent_detail`/`calls` | commits d705adf1, 2f2bf442 |
| modified | `Cargo.toml`, `Cargo.lock`, `apps/gateway-admin/package.json` | version bump 0.24.0 / 0.23.0 | this push |
| modified | `CHANGELOG.md` | `[0.24.0]` section | this push |
| modified | `apps/gateway-admin/next-env.d.ts` | Next-generated drift | this push |
| created | `docs/sessions/2026-06-11-gateway-usage-dashboard-and-metrics-backend.md` | this log | this push |

## Beads Activity

| ID | Title | Action | Status | Why |
|---|---|---|---|---|
| lab-2r5br | Gateway overview → usage dashboard | created, claimed, notes updated | open | Frontend dashboard; committed 727fbcd3, pending live verification |
| lab-sohnl | Backend: usage metrics aggregation + token instrumentation | created, notes updated | open | Backend phases committed; pending live verification + tier-2 telemetry |

## Repository Maintenance

- **Plans**: not moved — `quick-push` constrains the session save to documentation only. `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/mcp-streamable-http-oauth-proxy.md` are unrelated to this session; left as-is.
- **Beads**: `lab-2r5br` and `lab-sohnl` updated with committed state + remaining work (notes). Both intentionally left **open** — work is verified at the unit level but not end-to-end.
- **Worktrees/branches**: inspected (`git worktree list`); other worktrees (`settings-page-revamp`, `protected-mcp-route-gateway-subsets`) are unrelated/dirty — left untouched.
- **Stale docs**: `OBSERVABILITY.md` updated for the new token fields (in-session). No other stale docs identified.

## Tools and Skills Used

- **Shell/file tools**: extensive Rust + TS edits, `cargo check/clippy/nextest/fmt`, `pnpm tsc/eslint/test`, `git`.
- **Skills**: `vibin:aurora-design-system` (UI tokens/conventions), `vibin:save-to-md` (this log), `vibin:quick-push` (release flow). `beads` for tracking.
- **Browser**: headless `google-chrome` + Playwright (cached chromium) for dashboard screenshots; diagnosed a `next dev` `allowedDevOrigins` cross-origin issue (fixed via `LAB_ALLOWED_DEV_ORIGINS`).
- No MCP tools or subagents were used for the implementation.

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo check --workspace --all-features` | clean | Finished, no errors | pass |
| `cargo nextest run --all-features -E 'test(logs::metrics) …'` | green | 11 passed (5 metrics + 3 drill-down + 3 existing) | pass |
| `cargo clippy --workspace --all-features -- -D warnings` | clean | Finished, no warnings | pass |
| `pnpm tsc --noEmit` / `pnpm exec eslint` (gateway-admin) | clean | exit 0 | pass |
| `pnpm test:unit` (gateway-admin) | only pre-existing fails | 436/439 (3 pre-existing, unrelated) | pass |

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| gateway-admin overview | static server-fleet summary | full usage dashboard (mock data default) |
| dispatch logs | tokens only on Code Mode | `input_tokens`/`output_tokens` on every MCP + API dispatch |
| logs dispatch | search/tail/stats only | + `logs.metrics`/`tool_detail`/`agent_detail`/`calls` |
| Aurora radii (gateway-admin) | 14/18/22 | 6/8/10 (app-wide) |

## Risks and Rollback

- **App-wide radius change** affects every gateway-admin page; other pages (marketplace/settings/chat) not visually re-verified. Rollback: restore `--radius-1/2/3` to 14/18/22 in `app/globals.css`.
- **Field-extraction assumptions** (`service`/`elapsed_ms`/token shapes) are unit-tested vs fixtures, not a live gateway — a mismatch would yield wrong/empty real numbers. Rollback: dashboard stays on mock until verified.

## Open Questions

- Do real dispatch events store `service`/`elapsed_ms` exactly where the extractor expects? (Needs a live gateway with traffic to confirm.)
- App-wide square radii on non-dashboard pages — acceptable, or scope back?

## Next Steps

1. **Live end-to-end verification (highest priority):** build/run the labby binary (or `just dev-debug` hot-swap into the container), generate real tool-call traffic, flip `NEXT_PUBLIC_MOCK_DATA` off in gateway-admin, and reconcile the dashboard numbers against the actual logged events. Confirm the completion-event discriminator and `fields_json` shapes match reality.
2. **Tier-2 telemetry** (turns best-effort metrics real): log **source IP** per call (thread client IP through the dispatch context); **classify actors** agent-vs-device with friendly labels (currently `agent_kind="agent"`, `ip=""`); log Code-Mode **truncation_rate** + **artifact_writes**; **first-seen** tracking for new-vs-returning agents.
3. **Optional follow-ups**: instrument CLI dispatch tokens (`cli/helpers`); SQL aggregation in the log store to remove the 10k fetch cap for busy/7d windows; re-verify the app-wide radius change + light mode across other gateway-admin pages.
4. **Then**: close `lab-2r5br` and `lab-sohnl` once verified end-to-end.
