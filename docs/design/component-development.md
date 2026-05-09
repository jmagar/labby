# Component Development Process

**Status:** Active  
**Scope:** `apps/gateway-admin` web UI feature and component work  
**Primary contract:** [Labby Design System Contract](./design-system-contract.md)  

## Purpose

This document defines the process for building substantial Labby web UI components. It captures the workflow being used for the Marketplace v2 work and should be followed for new feature pages, major component rewrites, and interactive previews.

The goal is to keep implementation grounded in an approved design, make reviewable live previews available early, and prevent public `/dev/*` previews from performing real mutations.

## Required Workflow

### 1. Create A Feature Design Spec

Start with a written feature design spec before implementing the real component.

The spec must cover:

- the user problem and feature scope
- the intended page structure and interaction model
- data sources and backend actions
- view modes, filters, panels, dialogs, and empty/loading/error states
- responsive behavior
- accessibility expectations
- read-only preview behavior for `/dev/*`
- known risks and open questions

For Marketplace v2, the spec should explicitly describe the replacement of the current tab model with a Gateway-style filter rail and the card/table density switch.

### 2. Lock The Feature Design Spec

Treat the design spec as the working contract once the user approves it.

Do not start implementation from an informal chat transcript alone. If the user changes direction, update the spec before continuing. The spec does not need to be large, but it must be specific enough that the implementation can be reviewed against it.

### 3. Review The Design System Contract

Before creating or updating components, review [design-system-contract.md](./design-system-contract.md).

The implementation must align with:

- typography rules, especially `Manrope` display usage and `Inter` working UI usage
- Aurora semantic color tokens instead of one-off colors
- surface, elevation, radius, and spacing tiers
- focus, hover, active, and motion rules
- component contracts for buttons, pills, filters, inputs, badges, tables, panels, and cards
- page-level rules for Marketplace and catalog lists
- accessibility and responsive requirements

If the feature is intentionally copying another page pattern, inspect that page directly. For Marketplace v2, the Gateway page is the reference for the filter sidebar, list density, and card/table switching patterns.

### 4. Create A Detailed Render

Create a detailed render before wiring the real implementation.

The render can be a self-contained HTML mockup file, a temporary React preview, or a staged component shell. It must be detailed enough to review layout, hierarchy, density, copy, states, and interaction affordances. It should not be a vague wireframe if the next step is production implementation.

The preferred approach for new feature work is a **self-contained HTML mockup** written to `~/.superpowers/brainstorm/content/<feature>.html` and served at `/dev/<feature-name>` via the axum mockup handler. This lets design iteration happen without any Next.js rebuilds. Once the design is locked, the HTML mockup is discarded and replaced with a real React page.

### 5. Serve The Render At `/dev/<feature-name>`

All mockups and in-development components must live under:

```text
/dev/<feature-name>
```

Examples:

```text
/dev/marketplace
/dev/setup
/dev/settings
/dev/gateway-policy
```

Do not add new ad hoc preview routes outside `/dev/*`. The `/dev` index should link to active previews when useful.

#### Two-tier serving model

There are two distinct ways a `/dev/<feature-name>` URL is served, depending on where the feature is in development:

**Tier 1 — HTML mockup (pre-React, design iteration only)**

Self-contained HTML files written to `~/.superpowers/brainstorm/content/` are served by the axum backend via the `dev_mockup_named` handler (registered at `/dev/{name}` in `router.rs`). The handler reads the newest HTML file whose stem contains the name fragment — no Next.js rebuild required.

Use this tier for rapid design iteration: layout, copy, interactive states, and flow can all be reviewed and changed without touching any React code or rebuilding the Next.js export.

Mockup files at this tier are **not** Next.js pages and will **not** appear in the `pnpm build` route listing.

**Tier 2 — Live Next.js component (production implementation)**

Once the mockup design is locked, the route becomes a real Next.js page (`app/(admin)/<feature>/page.tsx`). The Next.js static export owns the URL from this point forward. The axum `/dev/{name}` handler still serves the old mockup HTML as a reference if the file exists, but the Next.js fallback takes over once the page is in the static export.

Real Next.js pages at this tier appear in `pnpm build` output and must follow the read-only contract described below.

#### Route ownership summary

| Path pattern | Owned by | Notes |
|---|---|---|
| `/dev/api/*` | axum backend | read-only dispatch endpoints only |
| `/dev/{name}` (HTML mockup phase) | axum `dev_mockup_named` handler | reads `~/.superpowers/brainstorm/content/{name}.html` |
| `/dev/{name}` (React phase) | Next.js static fallback | after `app/(admin)/dev/{name}/page.tsx` exists |

The `dev_mockup_named` handler and its supporting functions live in `crates/lab/src/api/router.rs` alongside `dev_marketplace_readonly`. **Do not move them to `web.rs`** — the other Claude session strips `web.rs` of dev-tooling code that it deems unrelated to production serving. Do not make those handlers delegate to `serve_web_request`, which serves the Next.js SPA rather than mockup files.

### 6. Iterate And Revise The Render

Use the `/dev/<feature-name>` route for review. Iterate on layout, copy, states, and interaction details until the user confirms the direction.

During this phase, prefer fast visible revisions over backend-heavy implementation. The goal is to lock the UI shape before spending effort on production wiring.

### 7. Lock The Exact UI And Update The Spec

Once the user approves the render, update the feature design spec to match the approved UI.

The spec should record:

- the final layout model
- accepted deviations from earlier ideas
- final component states
- final data requirements
- final interaction behavior
- any approved deviations from the design system contract

This keeps implementation review grounded in the actual approved design, not an outdated first draft.

### 8. Build Real, Live Components

Replace the render with real application components that use live backend data.

For `/dev/<feature-name>`, the component must be fully live but read-only. Users should be able to navigate, filter, search, open panels, switch view modes, preview dialogs, and inspect live data. Users must not be able to perform real mutations from `/dev/*` routes, including local no-auth development runs.

Production routes may use the same components, but mutating behavior must remain behind the normal authenticated product route.

**`AppSidebar` and shared layout components.** The `/dev/` layout in `apps/gateway-admin/app/dev/layout.tsx` uses the full `AppSidebar`. `AppSidebar` is safe in local no-auth preview runs because it makes no API calls of its own — all its data comes from the session store singleton, and the user card in the footer renders only when `session.status === 'authenticated'`. `AuthBootstrap` (which calls `loadBrowserSession()`) is present only in the `(admin)` layout, so on `/dev/*` routes the session store stays at `loading` unless the route is reached through the authenticated server shell. This keeps navigation and theme controls clean without adding auth-gated API calls to preview layouts.

When adding new `/dev/*` layouts that include components beyond `AppSidebar`, verify that each new component handles the `loading` and `unauthenticated` session states without making auth-gated API calls.

## `/dev/*` Read-Only Contract

`/dev/*` routes are authenticated whenever bearer or OAuth auth is configured. They are only open when Lab is intentionally running with no auth configured for local development. Read-only enforcement is still mandatory because component previews bypass the normal product workflow boundaries and must remain safe in both authenticated and local no-auth dev runs.

Read-only protection must be implemented in two layers.

### Frontend Guard

The frontend action client must detect when the current route is under `/dev`.

In `/dev/*` mode it must:

- allow only explicitly whitelisted read actions
- block mutating actions before `fetch`
- return or throw a clear `dev_preview_read_only` error for blocked actions
- route live read requests to a dedicated dev-safe backend endpoint when required

Marketplace currently uses this pattern in `apps/gateway-admin/lib/dev/preview-mode.ts` and `performServiceAction`.

### Backend Guard

The backend must expose dev preview API endpoints separately from normal authenticated product APIs.

Dev preview endpoints must:

- be route-specific, for example `/dev/api/marketplace`
- whitelist read-only actions server-side
- reject every non-whitelisted action with `403` and kind `dev_preview_read_only`
- reuse the real dispatch path for allowed reads so previews show real data
- never trust the frontend guard as the only protection

The normal `/v1/*` API remains protected by OAuth or bearer/session auth. `/dev/api/*` exists only for safe read-only preview data.

**Implementation location.** This is a Rust axum backend, not a Next.js route handler. The backend guard for marketplace lives in `crates/lab/src/api/router.rs` as the `dev_marketplace_readonly` handler, mounted unconditionally outside the v1 auth middleware at the bottom of `build_router()`. New dev preview services must add a handler following that pattern and register it with `router.route("/dev/api/<service>", post(...))`.

**Whitelist synchronization.** The frontend `READ_ONLY_ACTIONS` set in `apps/gateway-admin/lib/dev/preview-mode.ts` and the backend `DEV_MARKETPLACE_READ_ACTIONS` constant in `router.rs` must be kept in sync. Drift between them is a silent breakage class: frontend lets the action through, backend returns `dev_preview_read_only`, the dev preview appears broken with no obvious cause. When adding a new read action to either list, add it to both.

**Extending dev preview to additional services.** When a new `/dev/<feature>` route needs live data from a service other than marketplace:

1. Add a `dev_<service>_readonly` handler in `router.rs` with its own allowed-action list.
2. Register it with `router.route("/dev/api/<service>", post(...))`.
3. Add a URL mapping entry to `devPreviewActionUrl()` in `preview-mode.ts` so the frontend routes to the dev endpoint:
   ```typescript
   if (url === '/v1/<service>') return '/dev/api/<service>'
   ```
4. Add the new action set to `READ_ONLY_ACTIONS` in `preview-mode.ts`.

### Mutation UX In Read-Only Mode

Interactive controls that would normally mutate state should still be reviewable.

Acceptable patterns:

- disable the final destructive button and explain that dev preview is read-only
- allow opening dialogs but block the submit action with a read-only message
- show install/remove/update flows as previews without sending the mutation

Do not silently hide important mutating controls if that prevents review of the real workflow.

## Revision Loop

After the real component is wired:

1. Apply user-requested revisions.
2. Update the feature design spec if behavior, layout, or data shape changed.
3. Keep `/dev/<feature-name>` live and read-only throughout the loop.
4. Re-run focused verification after each meaningful change.

## Review Pass

Before calling the component complete, run a systematic review of the implemented UI and UX.

The review must include:

- browser inspection with Chrome DevTools MCP or the available browser-devtools automation
- dark-mode visual review first
- light-mode review when the component uses surfaces, borders, charts, or status colors
- desktop and mobile viewport checks
- keyboard navigation and visible focus checks
- console error review
- network request review to confirm `/dev/*` uses read-only endpoints
- loading, empty, error, and populated states
- comparison against the locked feature design spec
- comparison against [design-system-contract.md](./design-system-contract.md)

Address all issues found in the review pass, then re-check the affected paths.

## Design System Deviations

The design system contract is the default rule. Deviations are allowed only when the end result is cleaner, prettier, and more correct for the feature.

Any deviation must be explicit.

The implementer must:

- identify the exact contract rule being deviated from
- explain why the deviation produces a better result
- describe the user-visible effect
- ask the user to approve the deviation before treating it as accepted
- record approved deviations in the feature design spec

Unapproved deviations must be treated as defects and corrected before completion.

## Completion Checklist

Use this checklist before marking component work complete:

- feature design spec exists
- feature design spec is approved
- design-system contract has been reviewed
- detailed render was created
- render or component is served at `/dev/<feature-name>`
- UI was iterated with user feedback
- final UI was locked into the spec
- real component uses live backend data
- `/dev/*` route is read-only at frontend and backend layers
- frontend `READ_ONLY_ACTIONS` and backend allowed-action list are in sync (no drift)
- unit tests exist for the frontend read-only guard (`preview-mode.ts` or equivalent)
- backend integration tests cover allowed and blocked actions at `/dev/api/<service>`
- any new components added to `/dev/` layouts verified to handle `loading`/`unauthenticated` session states without making auth-gated API calls
- user-requested revisions are complete
- Chrome DevTools MCP or browser-devtools review pass is complete (fallback: browser network/console manual check)
- design-system compliance pass is complete
- feature spec compliance pass is complete
- all review issues are addressed
- any deviations are approved and documented
