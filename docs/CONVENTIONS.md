# Conventions

These are locked implementation rules. They are not optional style suggestions.

## Workspace

- dependency versions live in the workspace root
- lints live in the workspace root
- both crates inherit from the workspace
- the workspace version is the release version
- release builds are optimized and stripped

## Rust Module Style

- no `mod.rs`
- sibling `foo.rs` plus `foo/`
- public API must be explicit rather than incidental

## Async Trait Style

Use native `async fn in trait`.

Do not introduce:

- `async-trait` as the default pattern
- `Box<dyn ServiceClient>`
- trait-object-driven service dispatch

The architecture is intentionally concrete and feature-gated rather than dyn-heavy.

## Cancellation

Cancellation is handled at the top level by dropping futures, not by threading cancellation tokens through every service method.

## HTTP Client Rules

`HttpClient` is the single transport layer for services.

It owns:

- auth injection
- retry behavior
- timeout behavior
- error mapping
- tracing

Service modules must not re-implement those concerns.

The mandatory observability contract for dispatch logging, request logging, correlation, redaction, and verification lives in [OBSERVABILITY.md](./OBSERVABILITY.md).

Additional rules:

- retry only retryable failures
- do not retry unsafe writes by default
- do not concatenate query strings manually in service code

## Error Taxonomy

Use the canonical `ApiError` taxonomy for shared transport-layer failures.

Service-specific errors may wrap that taxonomy, but they must not fork it.

The canonical error contract for stable kinds, envelopes, and mapping rules lives in [ERRORS.md](./ERRORS.md).

## Action Metadata

`ActionSpec` is the source of truth for:

- action discovery
- param validation
- destructive-op marking
- MCP help surfaces

Do not maintain separate hand-written copies of action metadata.

## Action Naming & Deprecation

Action names are dotted `<resource>.<verb>` (lowercase, dot-separated) — this is
the **canonical** form (e.g. `deploy.plan`, `setup.bootstrap`,
`marketplace.mcp.install`). The dotted form is enforced by a catalog lint:
`catalog_action_names_are_dotted` in `crates/lab/tests/architecture_orchestrator.rs`
fails CI for any catalog action that does not match `^[a-z0-9_]+(\.[a-z0-9_]+)+$`.

Some services historically shipped **bare/flat** action names (e.g. `deploy`'s
bare `plan`/`run`/`rollback`, `setup`'s flat snake_case verbs). Those bare names
are kept as **deprecated aliases** for back-compat: the canonical dotted form is
added alongside the bare name, both dispatch to the same handler, and the bare
name is exempted from the dotted-name lint via the `DEPRECATED_ACTION_ALIASES`
allowlist in the same test file. Removing an alias from that allowlist (after the
dotted form has been added) makes the lint enforce the dotted name and is the
mechanism for retiring a deprecated alias.

Rules:

- New actions MUST use the dotted `<resource>.<verb>` form only — never add a new
  bare alias.
- A deprecated alias and its canonical dotted form must dispatch identically.
- Until tooling flags them, deprecated aliases appear as equal first-class catalog
  entries (so a service's catalog can look ~doubled). A future improvement is a
  `deprecated: bool` field on `ActionSpec` so `help`/`schema`/catalog surfaces can
  flag aliases and hide them from primary listings; that is a separate code
  follow-up and does not change `ActionSpec` today.

## Batch Operations

Batch APIs must be explicit and limited to real use cases.

Rules:

- use `<verb>_many`
- prefer bounded concurrency
- return per-item results rather than all-or-nothing batch wrappers
- only add batch forms where there is a real operator use case

## Progress Reporting

Long-running CLI operations may use a sink-based progress abstraction.

MCP calls must remain progress-free.

## Public API Surface

At the `lab-apis` crate root:

- re-export client types
- re-export core primitives
- do not flatten every service type into the crate root
- keep service-specific errors and models in service modules

## Documentation Policy

`lab-apis` is a real SDK and must behave like one.

Rules:

- public items must be documented
- feature-gated items must surface that gating in docs
- rustdoc warnings must be treated seriously
- examples on public client methods should be real and compilable when practical

The `lab` binary does not need the same level of public rustdoc coverage.

## Testing Policy

Three layers:

- CI-safe unit tests
- snapshots where wire-format stability matters
- ignored live integration tests for real homelab environments

Rules:

- CI must not require real services
- live integration tests must be opt-in
- shared client logic must be tested in `lab-apis`
- snapshot tests are appropriate for wire-shape stability

## Output Rules

- formatting belongs in the output layer
- `lab-apis` types stay free of presentation concerns
- avoid ad-hoc `println!`-driven UX logic

The canonical serialization and output-boundary contract lives in [design/SERIALIZATION.md](./design/SERIALIZATION.md).

## Catalog Visibility

`lab help`, `lab.help`, and `lab://catalog` hide services whose required `PluginMeta` env vars are not present. Bootstrap/operator services remain visible. Use `LAB_SHOW_ALL=1` or `lab help --all` when you need the full compiled catalog.

## Security and Privacy

- no telemetry
- no phone-home behavior
- no credential logging
- no secret echo in prompts or doctor output
- no surprise persistence for convenience features

Observability must preserve those privacy rules. If a proposed log shape conflicts with [OBSERVABILITY.md](./OBSERVABILITY.md) redaction requirements, the log shape is wrong.
