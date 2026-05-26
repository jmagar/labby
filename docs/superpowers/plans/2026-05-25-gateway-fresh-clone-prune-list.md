# Gateway-Only Fresh Clone Prune List

Date: 2026-05-25

Scope: a fresh clone of `jmagar/lab` trimmed down so the gateway remains fully operational.

Fully operational gateway means:

- Gateway CLI/API/MCP dispatch still works.
- Upstream MCP proxying still works.
- Gateway tool/resource/prompt discovery still works.
- `scout`, `invoke`, `tool_search`, `tool_execute`, and Code Mode gateway flows still work.
- Gateway import state, tombstones, exposure filters, schema resources, OAuth metadata, and protected admin routes still work.
- `apps/gateway-admin` can still provide the gateway management UI.

This document is intentionally conservative. If a file may be required by gateway runtime, gateway-admin, auth, discovery, semantic search, schema resources, or current Lab-backed virtual services, it is not listed as definitely removable.

## Keep

Do not trim these while claiming the gateway is fully operational:

- `crates/lab/src/dispatch/gateway.rs`
- `crates/lab/src/dispatch/gateway/**`
- `crates/lab/src/dispatch/upstream.rs`
- `crates/lab/src/dispatch/upstream/**`
- `crates/lab/src/oauth/upstream.rs`
- `crates/lab/src/oauth/upstream/**`
- `crates/lab/src/api/services/gateway.rs`
- `crates/lab/src/api/upstream_oauth.rs`
- `crates/lab/src/api/router.rs`
- `crates/lab/src/api/state.rs`
- `crates/lab/src/cli/gateway.rs`
- `crates/lab/src/cli/serve.rs`
- `crates/lab/src/mcp/server.rs`
- `crates/lab/src/mcp/catalog.rs`
- `crates/lab/src/mcp/upstream.rs`
- `crates/lab/src/registry.rs`
- `crates/lab/src/config.rs`
- `crates/lab/src/config/env_merge.rs`
- `crates/lab/src/dispatch/error.rs`
- `crates/lab/src/dispatch/helpers.rs`
- `crates/lab/src/dispatch/redact.rs`
- `crates/lab/src/dispatch/clients.rs`
- `crates/lab/src/net/**`
- `crates/lab/src/process/**`
- `crates/lab-auth/**`
- `crates/lab/tests/gateway_schema_resources.rs`
- `crates/lab/tests/logs_cli.rs`
- `apps/gateway-admin/app/(admin)/gateway/**`
- `apps/gateway-admin/app/(admin)/gateways/**`
- `apps/gateway-admin/components/gateway/**`
- `apps/gateway-admin/components/upstream-oauth/**`
- `apps/gateway-admin/lib/api/gateway-*`
- `apps/gateway-admin/lib/api/tool-exposure-draft.ts`
- `apps/gateway-admin/lib/api/service-action-client.ts`
- `apps/gateway-admin/lib/api/upstream-oauth-client.ts`
- `apps/gateway-admin/lib/hooks/use-gateways.ts`
- `apps/gateway-admin/lib/hooks/use-upstream-oauth.ts`
- `apps/gateway-admin/lib/types/gateway.ts`
- `apps/gateway-admin/lib/types/upstream-oauth.ts`
- `apps/gateway-admin/lib/gateway-env.ts`
- `apps/gateway-admin/lib/gateway-protected-route.ts`
- `apps/gateway-admin/lib/server/gateway-*`
- `apps/gateway-admin/lib/browser/gateway-detail.browser.test.ts`

Also do not trim these just because they look like non-gateway services:

- `crates/lab/src/dispatch/gateway/semantic.rs`
- `crates/lab-apis/src/qdrant.rs`
- `crates/lab-apis/src/qdrant/**`
- `crates/lab-apis/src/tei.rs`
- `crates/lab-apis/src/tei/**`

Those are part of gateway search/indexing behavior.

## Definitely Removable Runtime-External Files

These files are not required for a gateway runtime from a fresh clone. Removing them can affect docs, historical notes, development reports, or plugin packaging, but it should not remove gateway server functionality.

Use this as the first safe deletion set:

```bash
git ls-files \
  'docs/sessions/**' \
  'docs/references/**' \
  'docs/reports/**' \
  'docs/mockups/**' \
  'docs/coverage/**' \
  'docs/superpowers/**' \
  'plugins/**'
```

Notes:

- `docs/superpowers/**` includes planning artifacts. Keep a copy elsewhere if the extraction plan itself is still useful.
- `plugins/**` is plugin distribution/source packaging, not required by the gateway runtime. If the standalone deliverable must also ship as a Lab plugin, keep the specific plugin package intentionally rather than keeping the whole tree by default.
- Do not include `docs/surfaces/MCP.md`, `docs/dev/ERRORS.md`, `docs/dev/OBSERVABILITY.md`, `docs/design/SERIALIZATION.md`, or gateway-specific docs in this first deletion set unless you are also dropping developer documentation.

## Definitely Removable Local Artifacts

These are not part of a fresh clone if `.gitignore` is respected, but they are safe to delete if present in the working directory:

```bash
rm -rf \
  target \
  node_modules \
  .cache \
  .full-review \
  .superpowers \
  .worktrees \
  apps/gateway-admin/.next \
  apps/gateway-admin/out \
  apps/gateway-admin/node_modules \
  apps/gateway-admin/.tmp-chat-preview*.mjs
```

`apps/gateway-admin/out` is generated output. It is not the source of truth for gateway-admin.

## Gateway-Only UI Trim

These frontend routes and feature islands can be removed if the standalone gateway-admin app is intended to be gateway-only. This requires the matching navigation/sidebar/import cleanup before running the Next build.

```bash
git ls-files \
  'apps/gateway-admin/app/(admin)/activity/**' \
  'apps/gateway-admin/app/(admin)/chat/**' \
  'apps/gateway-admin/app/(admin)/design-system/**' \
  'apps/gateway-admin/app/(admin)/dev/**' \
  'apps/gateway-admin/app/(admin)/docs/**' \
  'apps/gateway-admin/app/(admin)/logs/**' \
  'apps/gateway-admin/app/(admin)/marketplace/**' \
  'apps/gateway-admin/app/(admin)/nodes/**' \
  'apps/gateway-admin/app/(admin)/registry/**' \
  'apps/gateway-admin/app/(admin)/settings/**' \
  'apps/gateway-admin/app/(admin)/setup/**' \
  'apps/gateway-admin/components/activity/**' \
  'apps/gateway-admin/components/ai/**' \
  'apps/gateway-admin/components/chat/**' \
  'apps/gateway-admin/components/design-system/**' \
  'apps/gateway-admin/components/logs/**' \
  'apps/gateway-admin/components/marketplace/**' \
  'apps/gateway-admin/components/nodes/**' \
  'apps/gateway-admin/components/registry/**' \
  'apps/gateway-admin/components/setup/**' \
  'apps/gateway-admin/lib/acp/**' \
  'apps/gateway-admin/lib/chat/**' \
  'apps/gateway-admin/lib/dashboard/**' \
  'apps/gateway-admin/lib/fs/**' \
  'apps/gateway-admin/lib/marketplace/**' \
  'apps/gateway-admin/lib/setup/**'
```

Also remove non-gateway frontend clients after checking no gateway page imports them:

```bash
git ls-files \
  'apps/gateway-admin/lib/api/acp-*' \
  'apps/gateway-admin/lib/api/auth-admin-*' \
  'apps/gateway-admin/lib/api/device-*' \
  'apps/gateway-admin/lib/api/doctor-*' \
  'apps/gateway-admin/lib/api/extract-*' \
  'apps/gateway-admin/lib/api/fs-*' \
  'apps/gateway-admin/lib/api/logs-*' \
  'apps/gateway-admin/lib/api/marketplace-*' \
  'apps/gateway-admin/lib/api/mcpregistry-*' \
  'apps/gateway-admin/lib/api/setup-*'
```

Required cleanup after this trim:

- Remove deleted destinations from `apps/gateway-admin/components/app-sidebar.tsx`.
- Remove deleted routes from any command palette, quick nav, breadcrumb, or dashboard entry points.
- Keep gateway auth bootstrap, protected route handling, API base URL handling, and shared UI primitives.
- Re-run the frontend build before calling the trim safe.

## Backend Trim That Is Not Yet Definite

Do not put these in the definite deletion set until the standalone gateway boundary is locked:

- `crates/lab/src/dispatch/<non-gateway-service>/**`
- `crates/lab/src/cli/<non-gateway-service>.rs`
- `crates/lab/src/api/services/<non-gateway-service>.rs`
- `crates/lab-apis/src/<non-gateway-service>/**`
- non-gateway feature entries in `crates/lab/Cargo.toml`
- non-gateway feature entries in `crates/lab-apis/Cargo.toml`

Reason: the current gateway can expose Lab-backed virtual services through the registry. Removing those modules is safe only after deciding the standalone gateway is strictly an upstream-MCP gateway and after updating the registry, catalog, feature flags, tests, and docs together.

## Verification Before Deleting More

After applying the definite runtime-external trim:

```bash
cargo check --workspace --all-features
cargo nextest run --workspace --all-features
```

After applying the gateway-only UI trim:

```bash
pnpm --dir apps/gateway-admin install
pnpm --dir apps/gateway-admin build
```

Then run a gateway smoke test against the resulting binary/server:

```bash
cargo run -p lab --all-features -- gateway list
cargo run -p lab --all-features -- serve
```

Expected gateway checks:

- `lab://gateway/servers` is still available.
- Gateway schema resources still resolve.
- Gateway upstream tools still proxy.
- `scout` returns indexed gateway tools.
- `invoke` can execute an exposed upstream tool.
- Protected gateway-admin routes still accept the configured admin auth path.

## Bottom Line

The sure trim set from a fresh clone is documentation/history/plugin packaging plus local generated artifacts. The gateway-only frontend can also be trimmed aggressively, but only with route and navigation cleanup. Backend service deletion is not yet a definite safe-delete category because the current gateway still intersects with Lab registry/catalog behavior and virtual-service exposure.
