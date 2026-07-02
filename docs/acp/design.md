---
title: ACP First-Class Service Design
created_at: 2026-04-23 17:03:03 EDT
updated_at: 2026-04-23 19:09:17 EDT
status: draft
owner: lab
---

# ACP First-Class Service Design

## Purpose

This document formalizes the design for promoting ACP to a first-class service
in `lab` without losing the current browser chat experience or duplicating MCP
management responsibilities already owned by `gateway`.

It is the canonical design record for:

- the ACP service name and ownership model
- the relationship between ACP, chat, and gateway
- the crate/module boundaries for ACP
- the target dispatch, API, CLI, and MCP surfaces
- migration phases and non-goals

## Locked decisions

The following decisions are now explicit and should be treated as stable unless
this spec is amended:

1. The canonical backend service name is `acp`.
2. The browser UI route remains `chat`.
3. ACP core capability logic belongs in `lab-apis`.
4. ACP surface adapters and registration belong in `lab`.
5. `gateway` remains the MCP control plane.
6. ACP integrates with gateway in-process, not via loopback HTTP.
7. Browser clients talk to ACP, not directly to gateway for conversational tool
   execution.
8. SSE remains the default ACP event-stream transport.
9. ACP runtime is provider agnostic.
10. Minimum supported providers are Codex, Claude, Gemini, GitHub Copilot, and
    OpenCode.
11. Chat/ACP agents must be valid deploy targets for Marketplace.
12. ACP must preserve raw `usage_update` payloads and raw `ContentBlock[]`.
13. ACP must invest in full `ContentBlock[]` rendering rather than flattening to
    plain text only.
14. ACP Registry compatibility is a first-class direction so users can install
    additional agents/providers over time.
15. The chat product remains transcript-first: no separate activity pane.
16. Reasoning and action flow belong to the assistant turn itself.
17. The stable resting order inside an assistant turn is:
    reasoning, then action flow, then assistant response text.
18. Action flow is a product rendering of visible agent work, not a debug
    inspector.
19. Action-flow rows should be humanized, narrative, compact, and updated
    in place instead of duplicated for intermediate updates.
20. Low-signal protocol noise should remain hidden or debug-only by default.
21. Mobile chat keeps the transcript primary, with sessions in a drawer.
22. The canonical visual target is a unified chain-of-thought container with a
    connected vertical action timeline, matching the interaction model of the
    shadcn AI chain-of-thought reference pattern.
23. `acp` remains the service/tool name; ACP action names stay
    resource-oriented and do not repeat the service prefix.
24. ACP machine-facing API and MCP surfaces use the shared `action + params`
    dispatch model; SSE is a transport exception for event delivery only.
25. ACP/chat is a master-only operator surface under the device-runtime model.

## Problem statement

ACP began as a product-local browser/API surface rather than a first-class
service integration. The first-class promotion has now landed; what remains
is the Phase 2 surface work and the secondary cleanups documented at the
bottom of this section.

Current state:

- browser route: `apps/gateway-admin/app/(admin)/chat/page.tsx`
- browser shell: `apps/gateway-admin/components/chat/chat-shell.tsx`
- API surface: `crates/lab/src/api/services/acp.rs`
- shared dispatch: `crates/lab/src/dispatch/acp/` (`catalog.rs`, `client.rs`,
  `params.rs`, `dispatch.rs`, SQLite `persistence.rs`, `page_context.rs`)
- capability module: `crates/lab-apis/src/acp/` (`session.rs` bounded
  `SessionHandle`, `persistence.rs` `AcpPersistence` trait, `types.rs`
  `AcpEvent`/`AcpSessionState`/`AcpSessionSummary`)
- runtime/session state: `crates/lab/src/acp/registry.rs`
- provider runtime: `crates/lab/src/acp/runtime.rs`
- registry registration: `crates/lab/src/registry.rs::build_default_registry`
  registers ACP behind the `acp` cargo feature (a member of `all`, excluded
  from gateway-only builds)

What is now landed (was previously listed missing):

- `lab-apis::acp` capability module
- shared `dispatch/acp/` service entry following the standard four-file
  layout
- registry registration as a first-class service, feature-gated behind the
  `acp` cargo feature
- HTTP `POST /v1/acp` shared-action surface alongside browser-compat REST
  routes for SSE
- MCP one-tool-per-service surface via the catalog-driven `lab.acp` tool
- explicit `lab_apis::acp::META` service metadata aligned with the rest of
  the platform

What is still missing or in progress:

- typed `lab acp ...` CLI subcommands (Phase 2; the shared dispatch path is
  reachable, but there is no clap-typed CLI shim)
- removal of legacy `Bridge*` compatibility/projection types — kept for the
  on-disk JSON event log written by `JsonFileAcpPersistence`; coordinated
  Rust + frontend wire-format change deferred
- batch migration of pre-structured `acp-providers.json` entries; today
  re-installing a provider migrates one entry at a time
- provider workspace jail and contained file-access policy — provider FS
  capabilities are off until a real jail exists, but the jail itself is
  future work

At the same time, `gateway` already owns upstream MCP management:

- upstream config
- connection pooling
- upstream auth
- tool/resource/prompt discovery
- exposure policy
- tool routing

That means ACP should not become a second MCP manager.

## Goals

1. Promote ACP to a first-class service in `lab`.
2. Preserve `gateway` as the canonical MCP control plane.
3. Keep `chat` as the UI route and presentation layer.
4. Establish one shared ACP execution path across API, CLI, and MCP.
5. Avoid browser-direct gateway execution semantics.
6. Avoid local loopback HTTP between ACP and gateway inside one process.
7. Preserve resumable, sequence-based event streaming semantics.
8. Make ACP/chat a valid Marketplace deployment target for agents, skills,
   commands, and MCP-oriented packages.
9. Support multiple ACP-capable providers behind one provider-agnostic runtime
   contract.
10. Preserve high-fidelity provider payloads needed for rich rendering and
    analytics.
11. Lock the ACP chat UI contract so backend and frontend evolution stay aligned.

## Non-goals

1. Replacing SSE with WebSockets by default.
2. Collapsing ACP into `gateway`.
3. Collapsing `chat` UI concerns into the ACP backend service.
4. Making ACP a remote upstream SDK client in the normal HTTP-service sense.
5. Reworking every current browser component before ACP service promotion.
6. Reducing provider outputs to a lowest-common-denominator plain-text model.
7. Treating raw protocol noise as first-class product UI by default.

## Recommended architecture

Recommended flow:

`browser -> acp service -> gateway runtime -> upstream MCP`

### Why this architecture

This architecture keeps each subsystem responsible for one coherent concern:

- `chat` renders the UI
- `acp` manages sessions and provider/runtime orchestration
- `gateway` manages upstream MCP infrastructure

This is preferable to:

- browser -> gateway directly
- acp -> gateway over the public HTTP API
- acp owning a second MCP management stack

## UI contract

The ACP chat product is transcript-first.

### Transcript structure

The chat surface should remain a single transcript column.

There should not be a separate activity pane or competing debug lane in the
main product experience.

Each assistant turn may contain three ordered sections:

1. reasoning block
2. action flow block
3. assistant response text

These sections are optional and may appear incrementally while streaming, but
the stable resting structure should preserve that order.

The canonical visual target for an assistant turn is a unified
chain-of-thought container:

- one compact container attached to the assistant turn
- reasoning and action flow rendered inside that container
- assistant response rendered as the primary readable answer below the chain
- no split inspector or competing activity pane

### Reasoning block

Reasoning is rendered as a compact collapsible block owned by the assistant
turn.

Rules:

- it appears only when ACP emits reasoning/thought chunks
- it opens automatically while reasoning is streaming
- it can be collapsed by the user
- it remains attached to the turn that produced it
- it is visually secondary to the assistant response

### Action flow

Action flow is the product rendering for visible agent work inside an assistant
turn. It is not a transport inspector and not a second pane.

The default reading mode must remain conversational rather than protocol-centric.

Each row should answer:

1. what the agent is doing now
2. what happened before the answer was produced
3. where to inspect more detail if needed

Rules:

- rows are humanized and task-oriented
- rows should read like a live work narrative
- rows may be grouped when it improves narrative legibility
- rows update in place when subsequent ACP updates correlate to the same action
- the operation keeps its position in the turn instead of moving around
- compact expansion may reveal useful raw details or artifacts
- raw payloads are secondary detail, not the default view
- low-signal protocol churn stays hidden or debug-only
- rows are rendered in a connected vertical timeline rather than as isolated
  utility cards
- inline chips, sources, previews, and artifacts should sit inside the timeline
  flow without breaking its visual continuity

Inline artifacts should stay compact enough that the action flow still reads as
a timeline rather than a stack of heavy inspector cards.

The target interaction pattern is the same family as the shadcn AI
chain-of-thought example:

- compact chain header
- connected step timeline
- human-readable step labels
- quiet status/meta
- inline chips and previews
- expandable nested detail only when needed

ACP should adopt that pattern as the canonical chat rendering target.

## Reference component set and theming contract

The imported shadcn AI components under `apps/gateway-admin/components/ai/` are
reference implementation assets only. They are not the ACP product contract and
not the Labby design authority.

They may be adapted, restyled, split, recomposed, or replaced as needed to fit:

- the ACP rendering contract in this document
- the Labby design system contract in `docs/design/design-system-contract.md`
- Aurora semantic tokens and shared primitives

Initial reference component set:

- `chain-of-thought`
- `reasoning`
- `tool`
- `artifact`
- `attachments`
- `code-block`
- `sources`
- `web-preview`
- `terminal`
- `test-results`
- `task`
- `plan`
- `confirmation`
- `schema-display`
- `file-tree`
- `snippet`
- `context`
- `inline-citation`
- `queue`
- `sandbox`
- `agent`
- `commit`
- `environment-variables`
- `package-info`
- `stack-trace`

### Theming and adaptation rules

These imported components must be treated as implementation accelerators only.
When used in ACP chat, product code must conform to the Aurora/Labby design
system rather than upstream shadcn defaults.

#### Tokens and colors

- product code must use Aurora semantic tokens, not shadcn generic surface
  tokens
- `bg-card`, `bg-background`, `bg-muted`, `text-foreground`,
  `text-muted-foreground`, and `border-border` must be remapped to the Aurora
  equivalents defined in `docs/design/design-system-contract.md`
- no raw hex, rgba, or hsl values may be introduced in ACP product rendering
- status styling must use muted Aurora status colors rather than saturated
  warning/error fills

#### Typography

- page and major panel headings should use the Aurora display ramp with
  `Manrope`
- transcript body, controls, metadata, chips, inspectors, and timeline rows
  should use `Inter`
- the final assistant response remains the primary reading surface and should
  use the standard Aurora body treatment
- reasoning labels, step metadata, and inline operational detail should use the
  compact control/dense-data typography range rather than ad hoc size utilities

#### Surfaces and elevation

- the main transcript container and dominant assistant-turn surfaces should use
  the Aurora Tier 2 panel language
- supporting inline blocks inside a turn should step down to calmer Tier 1 or
  control-surface treatments when they are secondary
- ACP chat should not mix flat shadcn card styling with Aurora lifted panel
  styling on the same surface
- imported components that ship as isolated cards should usually be recomposed
  into the unified assistant-turn container instead of rendered as independent
  floating cards

#### Radius and spacing

- all ACP chat surfaces must use the Aurora radius scale
- avoid arbitrary radii except for the one legacy tuned variant already allowed
  by the design-system contract
- spacing should follow the compact operator spacing contract: 8px to 16px for
  most internal layout, 20px to 24px only for major group separation
- imported components with roomy demo spacing should be tightened to Labby
  operator density

#### Interaction and motion

- focus-visible, hover, and active states must use the shared Aurora focus ring
  and restrained active treatment
- expand/collapse motion is allowed, but decorative animation and loud glows
  are not
- selected/current action state should be communicated through border emphasis,
  text hierarchy, and subtle glow rather than bright filled accents

#### Chat-specific component mapping

- `chain-of-thought` is the reference interaction shell for the assistant-turn
  reasoning/action container, but it must be rendered in Aurora panel language
- `reasoning` should be used as the compact collapsible reasoning section within
  an assistant turn, not as a standalone inspector
- `tool`, `task`, `plan`, `confirmation`, and `commit` should be adapted into
  the connected vertical action timeline rather than rendered as unrelated card
  blocks
- `artifact`, `attachments`, `sources`, `inline-citation`, `file-tree`,
  `schema-display`, `snippet`, `terminal`, `test-results`, `stack-trace`,
  `web-preview`, and `package-info` should be treated as compact inline
  artifacts or expandable detail blocks attached to a timeline step
- `agent`, `context`, `environment-variables`, `queue`, and `sandbox` should be
  used only where they support the conversational narrative; they should not
  turn the transcript into a general-purpose inspector workspace

#### Engineering rule

- if an imported AI component conflicts with the Labby design-system contract,
  the component must be changed; the design system wins
- shared Aurora primitives under `components/ui/` remain the source of truth for
  baseline button, card, badge, input, tooltip, collapsible, and related
  behavior
- `/design-system` should be updated when ACP introduces a shared new chat
  interaction pattern that becomes part of the reusable product language

## Event-to-render mapping contract

ACP event rendering must use a three-layer model:

1. raw ACP/provider events
2. canonical ACP chat render model
3. Labby/Aurora chat components

ACP events must not map directly to imported shadcn AI components.

### Why

Direct event-to-component binding would make vendor component structure the de
facto product model.

That is the wrong dependency direction.

The stable contract should instead be:

- backend preserves raw provider/runtime fidelity
- frontend derives a provider-agnostic render model
- rendered UI is composed from Labby/Aurora components that may internally use
  adapted shadcn AI building blocks

### Layer 1: raw event layer

ACP must preserve raw event fidelity for replay, debugging, and future richer
rendering.

Examples of raw preserved inputs include:

- message chunks
- thought chunks
- tool call start/update/finish events
- plan/task updates
- permission requests and outcomes
- source/resource references
- available commands
- current mode/config updates
- session info updates
- `usage_update`
- prompt stop reason
- raw `ContentBlock[]`

This layer is append-only history, not the direct product rendering contract.

### Layer 2: canonical ACP chat render model

The frontend should derive a stable render model from raw ACP events.

The render model should be provider-agnostic and conversationally oriented.

Initial canonical render node families:

- `reasoning_group`
- `timeline_step`
- `artifact`
- `assistant_response`
- `permission_request`
- `usage_summary`
- `source_set`
- `code_block`
- `web_preview`
- `terminal_output`
- `test_result_set`
- `schema_view`
- `file_tree_view`
- `environment_snapshot`

Recommended initial `timeline_step.kind` set:

- `tool`
- `task`
- `plan`
- `confirmation`
- `commit`
- `agent`
- `context`
- `queue`
- `sandbox`

Render-model rules:

- correlated updates must merge into the same logical render node when they
  describe the same ongoing action
- intermediate protocol churn should update an existing render node in place
  rather than create duplicate transcript rows
- the render model should preserve enough detail to expand into raw payloads or
  structured artifacts when needed
- the render model should support incremental streaming without changing the
  stable resting order of an assistant turn

### Layer 3: component mapping layer

Labby/Aurora chat components render the canonical ACP chat render model.

Imported shadcn AI components may be used as implementation building blocks, but
they are subordinate to the render model and Labby design system.

Initial mapping guidance:

- `reasoning_group` -> adapted `chain-of-thought` shell plus `reasoning`
- `timeline_step(kind=tool)` -> adapted `tool`
- `timeline_step(kind=task)` -> adapted `task`
- `timeline_step(kind=plan)` -> adapted `plan`
- `timeline_step(kind=confirmation)` -> adapted `confirmation`
- `timeline_step(kind=commit)` -> adapted `commit`
- `timeline_step(kind=agent)` -> adapted `agent`
- `timeline_step(kind=context)` -> adapted `context`
- `timeline_step(kind=queue)` -> adapted `queue`
- `timeline_step(kind=sandbox)` -> adapted `sandbox`
- `artifact(code)` or `code_block` -> adapted `code-block` or `snippet`
- `artifact(files)` or `file_tree_view` -> adapted `attachments` or `file-tree`
- `artifact(sources)` or `source_set` -> adapted `sources` or `inline-citation`
- `artifact(web)` or `web_preview` -> adapted `web-preview`
- `artifact(tests)` or `test_result_set` -> adapted `test-results`
- `artifact(terminal)` or `terminal_output` -> adapted `terminal`
- `artifact(schema)` or `schema_view` -> adapted `schema-display`
- `artifact(env)` or `environment_snapshot` -> adapted
  `environment-variables`
- `usage_summary` -> Aurora-native compact usage block; not a standalone vendor
  card by default
- `assistant_response` -> Aurora-native transcript body renderer

### Assistant-turn ordering

The stable assistant-turn rendering order remains:

1. `reasoning_group`
2. ordered `timeline_step` sequence with attached artifacts
3. `assistant_response`

Secondary metadata such as `usage_summary` should attach to the turn quietly and
must not displace the primary conversational reading order.

### Mapping ownership

The canonical event-to-render mapping contract belongs to ACP chat semantics,
not to any individual vendor component library.

That means:

- backend and frontend must agree on the semantic render-node vocabulary
- component swaps or redesigns should not require redefining ACP event meaning
- provider-specific additions should extend the render model deliberately rather
  than leak raw transport structure into the default UI

### Mobile behavior

On mobile:

- the transcript remains the primary surface
- the session list becomes a drawer
- the drawer should not remain the resting state after selecting or creating a
  session
- input stays pinned to the bottom
- transcript space takes priority over secondary chrome

## Ownership model

## Why ACP uses `lab-apis` + `dispatch` instead of the `gateway` pattern

ACP should follow the standard capability-module architecture:

- core capability logic in `lab-apis`
- shared operation semantics in `dispatch`
- thin shims in CLI, API, and MCP

It should not follow the `gateway` pattern of living entirely in `lab`.

### Why

ACP is a capability module, not a control-plane exception.

Its core concerns are reusable service concerns:

- session lifecycle
- provider runtime abstraction
- persistence
- event sequencing and replay
- provider capability reporting
- preservation of structured provider payloads

Those concerns need to be owned below product surfaces so they can be shared and
tested consistently.

ACP also needs to support multiple surfaces with one canonical execution path:

- browser/API
- CLI
- MCP

That is exactly what the `lab-apis -> dispatch -> thin adapters` model is for.

### Why this differs from `gateway`

`gateway` is a documented control-plane exception.

Its primary responsibility is coordinating product runtime behavior inside
`lab`, including:

- upstream config mutation
- reconcile
- pool swapping
- exposure policy
- runtime orchestration

Those are product-control-plane responsibilities, not a reusable capability
module in the same sense as ACP.

ACP is different:

- it has a provider-agnostic runtime core
- it owns persistent session state
- it owns event history and replay behavior
- it is expected to serve multiple adapters consistently
- it should remain evolvable independently of the browser UI

Because of that, ACP should not be modeled as another `gateway`-style
exception.

### Resulting implementation shape

ACP should be implemented as:

1. `lab-apis::acp`
   - types
   - registry/core runtime logic
   - persistence
   - provider abstraction
2. `crates/lab/src/dispatch/acp`
   - action catalog
   - params/schema/help
   - surface-neutral execution
3. thin shims in:
   - `crates/lab/src/api/services/acp.rs`
   - future CLI ACP commands
   - future MCP `acp` tool

This is the correct model even though ACP is product-local, because product-local
does not automatically mean `lab`-only. ACP belongs to the product-local
capability-module class, not the control-plane-exception class.

### `lab-apis::acp`

`lab-apis::acp` owns the ACP capability core.

Responsibilities:

- ACP session registry core behavior
- session lifecycle state
- event sequencing and replay logic
- runtime/provider abstraction
- provider registry and capability reporting
- provider lifecycle primitives
- persistence primitives for sessions and event history
- capability-level health logic
- ACP capability request/response and event types
- preservation of raw provider payloads such as `usage_update` and
  `ContentBlock[]`

Default target:

- ACP reusable capability logic should live in `lab-apis`

Nuance:

- tightly binary-coupled subprocess launch behavior may remain in `lab` if it
  proves not meaningfully reusable as SDK logic
- that exception is reuse-driven, not aesthetic; ACP should not keep broad
  service semantics in `lab` merely for symmetry with `gateway`

Non-responsibilities:

- axum routing
- CLI parsing
- MCP registration
- browser presentation
- gateway policy ownership

### `crates/lab/src/dispatch/acp`

The shared dispatch layer owns ACP operation semantics.

Responsibilities:

- ACP action catalog
- ACP schema/help metadata
- param validation/coercion
- service/capability resolution
- calling the ACP capability core
- surface-neutral results and `ToolError` mapping

Initial target layout:

```text
crates/lab/src/dispatch/acp.rs
crates/lab/src/dispatch/acp/
  catalog.rs
  client.rs
  dispatch.rs
  params.rs
```

Optional domain modules when ACP grows:

- `providers.rs`
- `sessions.rs`
- `events.rs`
- `permissions.rs`

### Surface adapters in `lab`

#### API

The API remains a thin adapter over shared dispatch.

Current ACP route handlers should be migrated so they delegate to shared ACP
dispatch/service logic rather than owning behavior directly.

#### CLI

ACP should gain a typed CLI surface for session operations.

Initial commands can cover:

- list sessions
- inspect one session
- start session
- prompt session
- cancel session
- provider health

#### MCP

ACP should expose one MCP tool named `acp`, following the existing
`action + params` pattern.

### Browser UI

The browser UI route remains `chat`.

The browser should:

- talk to ACP endpoints only
- remain on session-authenticated same-origin requests
- not own direct tool execution semantics against gateway
- render rich provider content from preserved `ContentBlock[]` rather than
  depending on lossy flattening
- derive product presentation from backend-owned ACP semantics rather than
  becoming the canonical owner of those semantics

## Gateway relationship

### Gateway remains authoritative for MCP

`gateway` continues to own:

- upstream config
- connection lifecycle
- auth/token/OAuth handling
- discovery of tools/resources/prompts
- exposure filtering
- execution routing to upstream MCP servers

### ACP consumes gateway through a narrow internal interface

ACP should integrate with gateway through a direct in-process interface.

Initial interface scope should stay intentionally narrow:

- list exposed tools available to ACP
- call one exposed tool
- optional later: read resource
- optional later: get prompt

Marketplace should also be able to target ACP/chat agents through ACP-owned
integration points rather than browser-local hacks. That means ACP needs a
stable notion of deployable agent targets that Marketplace can address for:

- agents
- skills
- commands
- MCP-oriented packages

ACP should not call gateway through:

- public HTTP routes
- browser requests
- synthetic loopback MCP-over-HTTP inside the same process

ACP/chat is a master-only operator surface:

- the master exposes ACP service endpoints and the `/chat` UI
- non-master devices do not expose ACP endpoints or the chat UI directly
- non-master devices may still participate indirectly through provider,
  execution, or deploy targets coordinated by the master

## Transport model

### Commands

Keep command operations as request/response:

- start session
- prompt session
- cancel session
- list/get session
- provider health

Machine-facing ACP command surfaces must follow the shared dispatch contract:

- MCP input remains `action + params`
- HTTP machine-facing ACP routes remain `action + params`
- CLI remains typed and human-facing, with internal mapping to the shared ACP
  action catalog

### Event streaming

Keep session event delivery as SSE.

Reasons:

- the current interaction model is asymmetric
- sequence-based replay is already natural
- same-origin auth is simpler
- reconnection semantics stay explicit
- it avoids WebSocket-specific state complexity

SSE is the sanctioned transport exception to the shared request envelope:

- it exists only for event-stream delivery
- it must not become a second semantic API model
- action semantics still belong to shared ACP dispatch, not to bespoke frontend
  route behavior

### Resume contract

Resume remains sequence-based:

- client subscribes with `since=<last_seq>`
- server returns backlog with `seq > since`
- server continues live streaming

## Provider model

ACP must be provider agnostic at the runtime boundary.

The design target is one ACP capability surface with provider-specific adapters
behind it.

Minimum first-class provider targets:

- Codex
- Claude
- Gemini
- GitHub Copilot
- OpenCode

This means ACP should not hardcode a Codex-specific runtime contract into its
core service identity. Codex is the current implementation seed, not the long-
term architecture boundary.

### Provider abstraction requirements

The provider abstraction should support:

- provider health
- session start
- prompt submission
- cancellation
- session updates and notifications
- raw usage events
- raw content block payloads
- provider capability reporting

The shared backend contract should preserve ACP richness before any UI-specific
projection.

The preserved event/domain set should include:

- message chunks
- thought/reasoning chunks
- tool calls
- tool call updates
- plans
- permission requests and outcomes
- available commands
- current mode
- config option updates
- session info updates
- usage updates
- prompt stop reason
- structured `ContentBlock[]`

### Provider abstraction implementation pattern

Provider abstraction in Rust must use enum dispatch, not `dyn Trait`.

`async fn in trait` is not object-safe. A `Box<dyn ProviderRuntime>` with async
methods will not compile in stable Rust without workarounds.

The required pattern for the known, closed provider set is enum dispatch:

- define a concrete `ProviderRuntime` enum with one variant per provider
- implement shared behavior via `match` delegation or the `enum_dispatch` crate
- keep the match exhaustive so new providers require explicit handling at compile
  time

If dynamic extensibility beyond the known provider set becomes a requirement
later — for example to support ACP Registry-installed providers as runtime
plugins — `dynosaur` provides a stable workaround for async trait object
dispatch. That decision should be made deliberately when the requirement is
concrete, not adopted prematurely.

The provider interface methods enumerated in "Provider abstraction requirements"
above remain the semantic contract. Enum dispatch is the required implementation
mechanism for the initial closed provider set.

### ACP Registry direction

ACP Registry support should remain a first-class direction for provider and
agent installation.

The intended user model is similar to Zed:

- users can discover/install ACP-compatible agents
- installed agents/providers become available to ACP runtime selection
- `lab` remains responsible for policy, registration, and presentation

This does not require full ACP Registry implementation in phase 1, but the
architecture should not block it.

## Service naming

Canonical backend service name: `acp`

Presentation/UI route name: `chat`

Rationale:

- `acp` describes the backend capability
- `chat` describes the UI affordance
- this avoids mixing conversation UI with backend service identity

> **Terminology note:** The term "ACP" in this document refers to the internal
> Agent Client Protocol used by this product. The IBM/BeeAI ACP specification
> was archived in August 2025 and merged into the Agent-to-Agent (A2A)
> protocol. That external specification is not the foundation for this design.
> References to `ContentBlock[]` throughout this document use Claude API
> terminology; they are not derived from any external ACP specification.

### Type naming convention

ACP-owned types must use the `Acp` prefix.

Examples:

- `AcpSession` not `BridgeSession`
- `AcpEvent` not `BridgeEvent`
- `AcpSessionRegistry` not `BridgeSessionRegistry`
- `AcpProvider` not `BridgeProvider`

The `Bridge*` prefix was an artifact of the current product-local
implementation where ACP existed as a product bridge rather than a first-class
service. As ACP is promoted, types must adopt the `Acp*` prefix to reflect
their service identity.

New types introduced during ACP service promotion must use `Acp*`. Existing
`Bridge*` types in the backend and frontend should be renamed as they are
migrated, starting with any types that cross the API boundary or appear in
public dispatch surfaces.

## Initial ACP action catalog

The first-class service should start with a small stable action set.

Recommended initial actions:

- `provider.get`
- `provider.list`
- `provider.select`
- `session.list`
- `session.get`
- `session.start`
- `session.load` <!-- Status: deferred — not implemented as of 0.13.x -->
- `session.prompt`
- `session.cancel`
- `session.close`
- `session.events`
- `target.list` <!-- Status: deferred — not implemented as of 0.13.x -->

Likely later additions:

- `session.resume`
- `session.fork`
- `tool.list`
- `tool.call`
- `session.permissions.respond`
- `session.mode.set`
- `session.config.set`
- `target.deploy`
- `registry.install`

Action naming follows the shared service contract from `docs/DISPATCH.md`:

- `acp` is the service/tool name
- action names are machine-oriented resource operations
- action names must not repeat the `acp.` service prefix inside the action
  string

The exact CLI syntax can remain typed and human-friendly while mapping to these
canonical service operations internally.

## Registration model

ACP should be registered as an always-available product-local service.

That means:

- registry entry in `build_default_registry()`
- shared catalog participation
- API route adapter over dispatch
- MCP tool registration
- CLI command registration
- `PluginMeta` publication

Expected registration posture:

- service name: `acp`
- browser route: `chat`
- product category: `Ai`
- service docs entrypoint: `docs/acp/README.md`
- ACP remains a product-local capability module, not a control-plane-only
  exception

The ACP browser route is not itself the service registration boundary; it is
only one consumer.

Marketplace integration should treat ACP as a deploy target once ACP target
modeling exists. That deploy relationship belongs in ACP service semantics, not
in ad hoc browser-only code.

Required verification posture for ACP promotion:

- SDK tests in `lab-apis`
- dispatch unit tests
- API adapter tests
- MCP adapter tests
- CLI tests
- browser/session rendering and stream-behavior tests where ACP chat semantics
  are involved

## Data fidelity and rendering

ACP must preserve and expose high-fidelity provider payloads.

### Usage data

Raw `usage_update` payloads must be preserved.

Rationale:

- provider-specific token/cost accounting should not be irreversibly flattened
- future UI, analytics, and export paths may need original usage semantics

### Content blocks

Raw `ContentBlock[]` must also be preserved.

Rationale:

- providers increasingly emit structured multimodal content
- lossy text-only flattening makes rich rendering and future interoperability
  harder
- ACP should remain compatible with richer provider surfaces over time

### Rendering direction

ACP should invest in full `ContentBlock[]` rendering.

That means the design should assume:

- structured block storage
- structured block transport to the browser
- UI renderers that understand provider content blocks directly

Plain-text summaries may still exist for convenience, but they are derived
views, not the source of truth.

## Observability and redaction

ACP must satisfy the shared observability contract in `docs/OBSERVABILITY.md`
before it is considered fully online.

Mandatory ACP instrumentation boundaries:

- CLI ACP dispatch
- MCP ACP dispatch
- API ACP dispatch
- provider runtime start/finish/error boundaries
- ACP-to-gateway bridge calls such as exposed-tool listing and tool execution
- SSE subscription, resume, disconnect, and reconnect boundaries

Minimum required dispatch fields remain the shared contract:

- `surface`
- `service = "acp"`
- `action`
- `elapsed_ms`
- `request_id` for API dispatch where applicable
- `kind` on failure

ACP-specific observability rules:

- session start/load/prompt/cancel/close/events actions must each be traceable
- provider runtime failures must be attributable to provider identity and action
  context without leaking credentials
- bridge calls into gateway must preserve caller context so tool execution is
  traceable end to end
- SSE resume flows should be diagnosable through sequence/reconnect context
  without logging sensitive payload content

Never-log material for ACP includes:

- provider credentials and tokens
- cookies and authorization headers
- secret env values
- raw prompt bodies when they may contain secrets or user-sensitive content
- unredacted raw provider payloads when they contain secrets, credentials, or
  sensitive inline data

ACP is not online until one successful and one failing path are traceable
end-to-end with correct redaction.

## ACP configuration model

ACP configuration follows the project-wide split between secrets/URLs in env and
preferences in `config.toml`.

Rules:

- provider credentials and secrets belong in `~/.labby/.env`
- ACP non-secret preferences belong in `config.toml`
- browser chat uses same-origin session auth only and must not depend on
  browser-visible bearer tokens
- ACP config loading belongs in `lab`, not `lab-apis`

Expected ACP config areas:

- selected/default provider
- session/event persistence paths
- retention limits for sessions and event history
- provider-specific non-secret preferences
- browser/chat UX defaults that are product preferences rather than credentials

The exact ACP config schema may evolve, but it must remain aligned with
`docs/CONFIG.md` and `docs/ENV.md`.

## ACP error contract

ACP should reuse the shared error taxonomy and dispatcher-level kinds from
`docs/ERRORS.md` by default.

Rules:

- provider/runtime transport failures should map through the shared stable kinds
- dispatch validation failures should use the normal dispatcher-level kinds
- ACP should not invent a parallel error vocabulary for routine session or
  provider failures
- any ACP-specific public error kind must be documented deliberately as a spec
  change

Default expected behavior:

- MCP uses the shared structured error envelope
- HTTP uses the shared structured JSON error envelope
- API and MCP must agree on semantic `kind` values for the same ACP failure
  class

## Session lifecycle state machine

ACP sessions should use an explicit state model.

Initial canonical states:

- `creating`
- `idle`
- `running`
- `waiting_for_permission`
- `completed`
- `cancelled`
- `failed`
- `closed`

Allowed transitions:

- `creating -> idle`
- `creating -> failed`
- `idle -> running`
- `idle -> closed`
- `running -> completed`
- `running -> failed`
- `running -> cancelled`
- `running -> waiting_for_permission`
- `waiting_for_permission -> running`
- `waiting_for_permission -> cancelled`
- `waiting_for_permission -> failed`
- `completed -> closed`
- `cancelled -> closed`
- `failed -> closed`

Rules:

- `closed` is terminal
- `cancel` stops current in-flight work but preserves transcript/history
- `close` removes the session from the active working set without implying hard
  deletion of persisted history
- `resume` means continue a paused/interrupted session when supported; if a
  provider/runtime cannot resume transport state, ACP may continue with a new
  turn in the same session instead of silently creating a different session

## Session persistence and retention

ACP session storage should be durable by default on the master.

Canonical durable store:

- SQLite on the master

Rationale:

- ACP state is local control-plane data
- the project already uses SQLite-backed local state elsewhere
- SQLite gives ACP transactional session/event append behavior without adding an
  external database dependency

Persistence split:

- SQLite is the canonical durable store for ACP state
- in-memory state owns only live runtime handles and transient subscriber state
- filesystem sidecars may be added later for large artifacts or exports, but
  they are not the canonical session/event store

Persisted data should include:

- session metadata
- event history
- derived summary fields used for lists and previews
- provider identity
- target identity
- permission outcomes relevant to the session transcript

ACP should not persist:

- ephemeral in-process runtime handles
- raw provider credentials
- transient auth material

Recommended initial durable tables/collections:

- `acp_sessions`
- `acp_session_events`
- `acp_session_summaries`
- `acp_permission_requests`
- `acp_targets`
- `acp_target_installs`

Retention defaults:

- closed sessions remain queryable
- active sessions are never pruned automatically
- event history is bounded per session
- old closed sessions may be pruned by age-based retention
- pruning policy should preserve session summaries even when deep event history
  is evicted

Recommended path/config posture:

- default durable store path under `~/.labby/`
- ACP persistence path should be configurable through `config.toml`
- an env override may exist later if it aligns with the shared config contract

## Provider identity and selection rules

Provider selection should be session-scoped.

Defaults:

- ACP may expose a global default provider used only when creating a new session
- each session records its selected provider explicitly
- changing provider mid-session is not allowed in the initial design
- provider capability reporting should expose:
  - a normalized capability summary
  - optional raw provider-specific detail for inspection

This keeps session semantics stable and avoids mixed-provider transcripts inside
one conversational session in v1.

## ACP target model for Marketplace deploy

ACP needs a stable target model so Marketplace can deploy agents and related
assets into ACP-managed runtimes.

A target is a named ACP-managed install/runtime destination.

Initial target classes:

- `agent_runtime`
- `skill_runtime`
- `command_runtime`
- `mcp_runtime`

Defaults:

- targets are system-scoped first, not browser-session-scoped
- ACP owns install, update, remove, and list semantics for those targets
- Marketplace deploys address ACP targets through ACP service semantics rather
  than browser-only flows
- conflicts must fail explicitly with a structured conflict response rather than
  silently overwriting existing registrations

## Permission and approval model

ACP should support an explicit permission pause-and-resume model.

Defaults:

- permission pauses are session-scoped
- approvals are granted per request by default
- supported initial outcomes:
  - allow once
  - deny once
  - allow for session

Permission requests should carry:

- action summary
- target or resource being touched
- risk or destructive hint
- optional structured raw details for expansion

The UI/API/CLI/MCP should all operate on the same semantic permission object.
Persistent global approval policy can come later; it should not complicate the
initial ACP service boundary.

## ContentBlock normalization policy

ContentBlock normalization must be additive, not lossy.

Rules:

- raw provider blocks remain preserved
- ACP derives normalized render nodes from those raw blocks
- unknown block types must be preserved even when the default UI cannot render
  them richly
- unknown block types should render through a fallback inspector/detail block
  rather than being dropped

Initial first-class normalized block families should include:

- text/markdown
- reasoning
- tool/action
- code
- file/tree
- citations/sources
- web preview
- terminal/log output
- test results
- confirmation/permission

## Surface contract

ACP should expose one coherent semantic surface across MCP, CLI, and API.

The browser chat remains a consumer of ACP semantics, not the owner of them.

### MCP tool actions

The MCP tool name remains `acp`.

Initial MCP `action` set:

- `provider.get`
- `provider.list`
- `provider.select`
- `session.list`
- `session.get`
- `session.start`
- `session.load` <!-- Status: deferred — not implemented as of 0.13.x -->
- `session.prompt`
- `session.cancel`
- `session.close`
- `session.events`
- `target.list` <!-- Status: deferred — not implemented as of 0.13.x -->

Likely later MCP additions:

- `session.resume`
- `session.fork`
- `session.permissions.respond`
- `session.mode.set`
- `session.config.set`
- `tool.list`
- `tool.call`
- `target.deploy`
- `registry.install`

Rules:

- MCP remains one tool per service
- input stays `action + params`
- `help` and `schema` project from shared ACP dispatch metadata
- MCP does not own ACP semantics independently of dispatch

### CLI commands

ACP CLI remains typed and human-facing.

Initial CLI command shape:

- `lab acp providers list`
- `lab acp providers get <provider>`
- `lab acp providers select <provider>`
- `lab acp sessions list`
- `lab acp sessions get <session_id>`
- `lab acp sessions start`
- `lab acp sessions load <session_id>` <!-- Status: deferred — not implemented as of 0.13.x -->
- `lab acp sessions prompt <session_id> --text <prompt>`
- `lab acp sessions cancel <session_id>`
- `lab acp sessions close <session_id>`
- `lab acp sessions events <session_id>`
- `lab acp targets list` <!-- Status: deferred — not implemented as of 0.13.x -->

Likely later CLI additions:

- `lab acp sessions resume <session_id>`
- `lab acp sessions fork <session_id>`
- `lab acp sessions permissions respond <session_id> <request_id> --allow|--deny`
- `lab acp sessions mode set <session_id> <mode>`
- `lab acp sessions config set <session_id> --key <key> --value <value>`
- `lab acp tools list`
- `lab acp tools call <tool_name> --json <payload>`
- `lab acp targets deploy <target> --package <ref>`
- `lab acp registry install <package>`

Rules:

- CLI syntax stays ergonomic and typed
- CLI maps internally to the shared ACP action catalog
- CLI output remains a thin adapter over dispatch results

### API endpoints

ACP API must distinguish between:

- machine-facing ACP dispatch endpoints
- browser chat streaming endpoints

Initial machine-facing ACP endpoint:

- `POST /v1/acp`

Request shape:

- `{ "action": "...", "params": { ... } }`

Initial browser/session event endpoint:

- `GET /v1/acp/sessions/{session_id}/events?since=<seq>`

Initial browser-oriented ACP helper routes may exist only when they are thin
transport adapters over ACP semantics, for example:

- `GET /v1/acp/provider`
- `GET /v1/acp/sessions`

But the canonical machine-facing API contract remains dispatch-oriented:

- `POST /v1/acp` is the stable semantic endpoint
- any convenience route must remain a projection of the same ACP service logic
- convenience routes must not invent alternate ACP semantics

Likely later API endpoints:

- `POST /v1/acp/targets/{target_id}/deploy`
- `POST /v1/acp/registry/install`

Rules:

- API error envelopes follow the shared JSON error contract
- API success semantics must stay aligned with MCP and CLI
- SSE event delivery is a transport endpoint, not a second ACP semantic model

## Migration phases

### Phase 1: Formalize ACP as a service

- create `lab-apis::acp`
- move core ACP types/runtime/session logic there
- add `dispatch/acp`
- keep browser behavior functionally stable
- adapt API handlers to call dispatch
- preserve raw usage/content data in the ACP model

### Phase 2: ACP-to-gateway integration

- define narrow in-process ACP-to-gateway interface
- route ACP tool execution through gateway rather than ad hoc or duplicated
  logic
- keep gateway authoritative for policy and upstream state
- define ACP target/deploy model for Marketplace integration
- avoid provider-specific coupling in the ACP core

### Phase 3: Surface completion

- add typed CLI
- add MCP `acp` tool
- finalize service registration and metadata
- expand docs and verification coverage
- add richer provider selection and target deployment flows
- expand `ContentBlock[]` rendering support
- evaluate ACP Registry install flows

## Risks

### Risk: ACP duplicates gateway responsibilities

Mitigation:

- keep the ACP-to-gateway interface narrow
- make gateway authoritative for MCP control-plane behavior

### Risk: Browser concerns leak into service design

Mitigation:

- keep `chat` UI documentation and ACP service documentation separate
- keep browser auth/presentation concerns outside `lab-apis::acp`

### Risk: ACP promotion becomes a large rewrite

Mitigation:

- migrate in phases
- preserve the current browser route and transport model
- move existing logic behind stable boundaries before expanding capability

## Open questions

These questions remain intentionally open and should be resolved in follow-up
planning, not ad hoc implementation:

1. What exact trait or interface should ACP use for gateway tool execution?

   **Resolution:** ACP accesses `GatewayManager` in-process via `AppState` (an `Arc`-shared concrete struct); no loopback HTTP is used. `GatewayManager` exposes `discovered_tools()` for listing; a `call_tool` execution method is Phase 2 work.

2. Which ACP actions are exposed on day one through MCP and CLI?

   **Resolution:** The implemented Phase 1 action set is: `provider.get`, `provider.list`, `provider.select`, `session.list`, `session.get`, `session.start`, `session.start_and_prompt`, `session.prompt`, `session.cancel`, `session.close`, `session.bulk_close`, `session.events`, `session.subscribe_ticket`, `session.permission.approve`, `session.permission.reject`. CLI typed subcommands are Phase 2.

3. Should ACP publish `PluginMeta`, or should product-local always-on services
   use a separate metadata pattern consistent with `gateway`?

   **Resolution:** ACP publishes `PluginMeta` (with `META` constant in `lab-apis::acp`) and is registered via `build_default_registry()` like other product-local services (now behind the `acp` cargo feature). No separate metadata pattern was needed.

4. How much of the current `crates/lab/src/acp/` code moves into
   `lab-apis::acp` in the first migration pass versus later cleanup?
5. What is the exact ACP target/deploy contract between Marketplace and ACP?
6. How should installed ACP Registry agents/providers be represented in config
   and metadata?
7. What is the canonical cross-provider render model for `ContentBlock[]` in the
   browser UI?

## Decision summary

ACP will be promoted as a first-class product capability with its core logic in
`lab-apis`, its shared operation semantics in `dispatch`, its browser route
remaining `chat`, and its upstream MCP integration flowing through `gateway`
via an in-process interface.
