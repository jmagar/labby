# lab/src — Product Surfaces And Dispatch

This directory is the product layer for `lab`.

It owns:

- CLI
- MCP
- API
- output formatting
- config loading
- shared product-surface orchestration

Before editing here, align with:

- `docs/ARCH.md`
- `docs/dev/DISPATCH.md`
- `docs/dev/OBSERVABILITY.md`
- `docs/dev/ERRORS.md`
- `docs/SERIALIZATION.md`
- `docs/dev/SERVICE_ONBOARDING.md`

## Layer Contract

`lab-apis` is the upstream SDK layer.

`crates/lab/src` is the product layer above it.

The intended dependency direction is:

- `cli -> dispatch -> lab-apis`
- `mcp -> dispatch -> lab-apis`
- `api -> dispatch -> lab-apis`

Forbidden:

- `cli -> mcp`
- `api -> mcp`
- `cli -> api`
- `mcp -> api`

If multiple surfaces need the same operation semantics, that logic belongs in the shared dispatch layer, not in one surface module.

## Ownership

### `dispatch/`

The shared dispatch layer should own:

- operation catalog
- param metadata
- param validation
- destructive-op metadata
- client and instance resolution
- SDK calls
- surface-neutral results
- surface-neutral dispatch errors

### `cli/`

CLI owns:

- typed `clap` parsing
- human command UX
- human output formatting
- confirmation prompts

CLI does not own shared operation semantics.

### `mcp/`

MCP owns:

- tool registration
- protocol envelopes
- `help` and `schema` exposure
- elicitation behavior

MCP does not own shared operation semantics.

### `api/`

API owns:

- axum routing
- request extraction
- status mapping
- HTTP response shaping

API does not own shared operation semantics.

## Practical Rules

- Do not call MCP dispatch modules from CLI.
- Do not call MCP dispatch modules from API.
- Do not read env directly in multiple surface modules when shared client resolution can own it.
- Do not duplicate dispatch timing, logging, or error-shaping helpers per service when they can be shared.
- Do not move upstream request construction or response parsing out of `lab-apis`.

## CLI Contract

Typed CLI is the human-facing contract.

When adding a new service:

- prefer typed subcommands
- keep command and flag UX human-oriented
- map those commands onto shared service operations

Do not force MCP-style `action + params` onto the CLI unless the project docs explicitly allow it.

## Observability

Surface layers must comply with `docs/dev/OBSERVABILITY.md`.

That means:

- dispatch events belong at the surface boundary
- caller context must flow into downstream request logs
- surfaces should use shared helpers when possible instead of inventing per-service log shapes

## Errors And Serialization

Surface layers must comply with:

- `docs/dev/ERRORS.md`
- `docs/SERIALIZATION.md`

That means:

- no transport-local reinvention of stable error kinds
- no duplicated envelope semantics drifting across surfaces
- no presentation concerns leaking into `lab-apis`

## When In Doubt

Ask these questions:

1. Does this define what the operation means?
   If yes, it belongs in the shared dispatch layer or `lab-apis`.
2. Does this define how one transport exposes that operation?
   If yes, it belongs in `cli`, `mcp`, or `api`.
3. Does more than one surface need this?
   If yes, do not leave it trapped in a sibling surface module.
