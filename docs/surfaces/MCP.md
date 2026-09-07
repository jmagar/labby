---
title: "MCP Surface"
created: "2026-07-30"
updated: "2026-08-17"
---

# MCP Surface

Labby exposes the supported gateway product through stdio and Streamable HTTP
MCP. The same service dispatch layer backs MCP, CLI, and HTTP.

## Entry Points

- Local stdio: `labby mcp`
- Hosted Streamable HTTP: `labby serve`, endpoint `/mcp`
- Protected MCP routes: route-specific paths configured through the gateway

See [TRANSPORT.md](./TRANSPORT.md) for transport and authentication boundaries.

## Services

The generated [service catalog](../generated/service-catalog.md) is authoritative.
The current services are:

- `gateway`
- `doctor`
- `server_logs`
- `setup`
- `snippets`
- `fs` when the feature is enabled
- `lab_admin` when runtime-enabled

Each service tool accepts:

```json
{
  "action": "service.action",
  "params": {}
}
```

Every service also supports shared `help` and `schema` discovery. Generated
MCP help lives in [../generated/mcp-help.md](../generated/mcp-help.md).

## Tool Results And `outputSchema`

Every builtin service tool returns the dispatch envelope as
`structuredContent` on success, mirrored by one JSON text block:

```json
{ "ok": true, "service": "gateway", "action": "gateway.list", "data": {} }
```

Builtin service tools — plus the `add_server`, `gateway_status`, and `settings` admin app
tools — advertise this envelope as their MCP `outputSchema`. The normative
contract is [mcp-tool-output.md](../contracts/mcp-tool-output.md) and the
published schema is
[dispatch-envelope.schema.json](../contracts/schemas/dispatch-envelope.schema.json);
a drift test binds the runtime schema to the published file. `data` is
deliberately unconstrained: one tool serves many actions, so a tool-level
schema cannot describe per-action payloads.

Scope and caveats:

- **On `tools/list` this is Raw-mode-only.** Builtins are suppressed from
  `tools/list` whenever Code Mode is enabled, so under Code Mode the only
  builtin schema a client sees there is `server_logs`. The `codemode*` tools
  advertise their own execution-trace schema instead. Under Code Mode,
  builtin services instead join the **Code Mode catalog** as in-process
  peers (`__in_process__<service>` namespaces, root scope only), so the
  envelope schema and the callable capability arrive together through
  `codemode.search` / `codemode.describe`.
- **Error envelopes are outside `outputSchema`.** An `isError: true` result
  carries the `{ "ok": false, … }` error envelope
  ([agent-error-contract.md](../contracts/agent-error-contract.md)). The
  exemption of error results from `outputSchema` conformance is converged
  ecosystem convention, not explicit MCP spec text.
- **No protocol-version gating.** The schema is serialized regardless of the
  negotiated protocol version; older clients ignore the unknown field.
- **`mcp_app` advertises no schema** — its control payload is
  `{"kind": "mcp_app_control", …}`, not the envelope, and an inaccurate
  schema is a hard client-side error in strict SDKs.
- Upstream tools relay their own `outputSchema` **shape** unchanged and their
  **result payloads** byte-identically. Documentation strings inside those
  schemas (`description`, `title`, `$comment`) are sanitized; schema-semantic
  keywords (`enum`, `const`, `default`, `examples`, `pattern`, `format`,
  `$ref`, property names) are not.

## Gateway And Code Mode

Without Code Mode, eligible upstream tools are projected into the downstream
catalog subject to route scopes and exposure filters. With Code Mode enabled,
raw upstream tools are hidden from normal `tools/list`. The synthetic surface
provides two text entry points:

- `codemode_read` is available to `lab:read`, `lab`, and `lab:admin`. It is
  annotated read-only and can discover or invoke only upstream tools whose live
  descriptor explicitly sets `readOnlyHint: true` without a contradictory
  `destructiveHint: true`. Missing or ambiguous annotations fail closed.
- `codemode` is the full execution surface for `lab` and `lab:admin`. The
  optional `codemode_ui` tool has the same execution authority and adds the
  Lab-owned trace inspector.

The full-execution tools are annotated as write-capable and potentially
destructive. Their annotations describe the approval boundary; upstream tool
authorization is still enforced again at dispatch time.

Approval-facing Code Mode descriptors include enabled, route-scoped upstream
names and normalized operator hints. They change when those configuration
determinants change, but remain stable across runtime health and discovered-tool
churn. Call `codemode.search(...)` and `codemode.describe(...)` inside a run to
inspect the current route-scoped tool catalog.

### Labby MCP App manager

The root gateway always advertises the `mcp_app` control tool, but its own MCP
App UI is opt-in. The tool manages `manager`, `codemode`, `gateway_status`,
`server_logs`, `add_server`, `settings`, or `all`. Every Labby-owned app surface
is opt-in and defaults off; the text-only control tool remains available so an
administrator can enable exactly the surfaces they want, including the manager UI
itself. The default target remains `codemode` for backward compatibility with the
original inspector-only control contract.

Reading status or opening the manager requires `lab` or `lab:admin`; changing
visibility requires `lab:admin`. The `mcp_app` control tool is intentionally
unavailable on protected subset routes because these switches mutate
gateway-global state, and the control tool itself cannot be disabled. Its UI metadata and `ui://` resource
can be disabled independently like the other Labby-owned app surfaces. Changes
are persisted and publish both `tools/list_changed` and
`resources/list_changed` without rebuilding the upstream pool.

Disabling a surface removes its app tool/metadata and owned `ui://` resources,
and direct reads of a disabled owned resource fail as unknown. It does not tear
down the underlying text/service capability where one exists: disabling the
Code Mode inspector leaves `codemode` available, disabling the Server Logs
app leaves the `server_logs` service tool available without app metadata, and
disabling Settings leaves the underlying `setup` service contract intact. The
Code Mode inspector retains the existing `code_mode.mcp_ui_enabled` setting;
the other switches live under `[mcp_apps]`.

Synthetic Code Mode keeps ordinary raw upstream tools hidden, but valid upstream
MCP Apps pass through automatically. An app owner must carry an exposed native
`ui://` binding (`ui.resourceUri` or `openai/outputTemplate`) on an allowed
upstream with `proxy_resources = true`; `expose_resources` applies to that exact
widget URI. Callback-only markers such as `ui.visibility=["app"]` or
`openai/widgetAccessible=true` are admitted only when the same upstream has such
an exposed owner. Duplicate tool names across global or subject-scoped OAuth
upstreams fail closed and are omitted rather than binding an arbitrary owner.

`lab:read` catalogs omit destructive upstream app tools/callbacks; invoking a
known destructive app tool requires `lab` or `lab:admin` before any elicitation.
OAuth app tools and their native `ui://` reads remain bound to the authenticated
subject's cached connection, and a subject-scoped resource denial is terminal
rather than falling back to a global connection. These passthrough rules are
independent of Labby-owned app toggles.

Code Mode may call exposed upstream MCP tools only. Lab actions are not callable
from inside its sandbox. Large upstream results must be projected or sliced
inside the sandbox before return.

## Authentication And Routes

The root administrative MCP endpoint uses the configured bearer or OAuth mode.
Public protected routes validate route-scoped Lab OAuth JWTs and their configured
resource/scope contract. A static operator bearer token is not a public resource
credential.

## Destructive Actions

When the client supports elicitation, destructive service actions use the shared
2026-07-28 MRTR confirmation flow: the dispatcher returns `input_required` and
validates the answer from the retried request's `inputResponses` together with
Labby's opaque, protocol-standard `requestState`. The state is short-lived,
single-use, and server-bound to the canonical action, normalized params,
authenticated caller, transport/session, route, and catalog security metadata;
mismatched, expired, or replayed confirmations fail closed.

When the client does **not** support form elicitation, destructive dispatch
fails closed before the action or any upstream transport runs. There is no
`params.confirm`, `--yes`, or header equivalent on the MCP path — request params
are payload, not authorization. The caller must use an elicitation-capable MCP
client or an operator surface with its own explicit confirmation contract.

`ActionSpec.destructive` is the single source of truth for this gate.
Authorization scope and confirmation are separate checks.

## Tool Annotations

Labby forwards each upstream tool's `annotations` object **verbatim** — including
`title`, unknown or future fields, and the absence of the block. It does not fill
in missing hints, overwrite hints it disagrees with, strip fields it does not
understand, or rename the tool while copying it. This holds on every listing path:
the aggregated path, the subject-scoped OAuth path, and through nested gateways.

Upstream hints are attacker-controlled data from Labby's perspective. Per the MCP
spec, clients must not make tool-use decisions based on annotations from untrusted
servers. Labby relays them for presentation; it does not vouch for them.

Independently of what an upstream claims, Labby derives its own fail-closed
`destructive` judgement for gating a proxied tool (`cached_upstream_tool`): a tool
is treated as destructive unless its annotations explicitly say otherwise. That
value never reaches the wire.

Annotations on Labby's **own** tools are implemented by the shared
`PermanentToolRegistry` descriptor builders. Two properties of that
current contract matter to clients: a Labby tool fronts
a whole service, so a tool-level hint is the least-safe **union** of that
service's actions and must not be read as a claim about a specific `action`; and
in a labby → labby chain these hints feed the next hop's own gate, so they are
advisory to clients but not inert.

Per-action truth (`destructive`, `requires_admin`) is available for the seven
registered service tools via `{"action": "help"}` or the `lab://<service>/actions`
resource. It is **not** available for `codemode`, `codemode_ui`, `mcp_app`,
`add_server`, `gateway_status`, or `settings`, which are not registry services.

Note that tool visibility and `lab://<service>/actions` are scoped by
`route_scope`, **not** by the caller's admin scope: action metadata crosses that
boundary even though action execution does not.

## Notifications

Catalog notifications are evaluated against each peer's visible contract,
coalesced, and held until in-flight tool calls drain. Do not restore global
broadcast semantics or notification delivery during an open turn.

`tools/list` assembles the complete visible contract, sorts it globally by tool
name, and then paginates it. Continuation cursors are bound to that contract's
revision; a cursor from a changed catalog is rejected instead of being resumed
at an unsafe offset. A session's notification baseline advances only after it
receives the final page of a complete listing. Subscribing before that point
keeps the baseline unpublished so the next relevant catalog trigger emits
`notifications/tools/list_changed`.

`resources/list`, `resources/templates/list`, and `prompts/list` can require
live upstream fan-out. Their first page therefore retains the complete result
set in route-shared memory and binds the continuation cursor to that snapshot.
Later pages read the retained, authorization-audience-isolated snapshot rather
than repeating upstream discovery. Snapshots are bounded and process-local; an
expired, evicted, or pre-restart cursor fails with `invalid_cursor` and callers
must restart from the first page.

## Resource Subscriptions

Labby serves resource subscriptions through `subscriptions/listen` only. The
deprecated `resources/subscribe` / `resources/unsubscribe` RPC pair is **not
offered**, and the capability is not advertised to sessions that would have to
use it.

MCP advertises one `resources.subscribe` flag for both mechanisms, so the
boundary is drawn per session rather than by clearing the flag outright:

| Session | Lifecycle | `resources.subscribe` advertised | Usable mechanism |
|---|---|---|---|
| Modern (2026-07-28) | `discover` | yes | `subscriptions/listen` |
| Legacy (pre-2026-07-28) | `initialize` | **no** | none |

Clearing the flag globally would break modern subscriptions — rmcp intersects a
client's requested `SubscriptionFilter` against the advertised capability — and
would also break the gateway's own upstream subscription negotiation. The
capability is therefore withheld in the legacy `initialize` adapter only.

A legacy session could not use either mechanism regardless: Labby implements no
`resources/subscribe` handler, and rmcp gates `subscriptions/listen` to modern
sessions. Advertising the flag to those sessions promised something that could
not work.

Delivering subscriptions to legacy clients over HTTP would additionally require
serving the 2025-06-18 transport era — the standalone `GET`/SSE stream and its
session management — which the 2026-07-28 transport replaced with
request-scoped streams. That is a transport-layer decision, not a subscription
one.

## Supported Product Boundary

The MCP server does not expose ACP, Marketplace, Registry-browser, Fleet/node,
Deploy-product, or old Agent Artifact Manager tools. Historical contracts are
preserved only under [../archive/retired-labby](../archive/retired-labby/).

On Linux, File Stash uses the ordinary one-tool service shape for
bounded metadata operations and exposes authorized file reads as
`stash://me/files/{opaque_file_id}` resources.
The URI is a stable object identity, not a filesystem path or filename. See
[STASH.md](../services/STASH.md) for the authorization, size, and error contract.
Unsupported platforms omit the service and its resources from MCP discovery.

## Agent Skills (SEP-2640)

Labby implements the draft MCP Skills extension behind the `skills` cargo
feature. The pinned draft revision, URI grammar, and verification requirements
live in [`docs/contracts/skills-extension.md`](../contracts/skills-extension.md),
which is also published in-band as the `lab://contracts/skills-extension`
resource so a client that does not speak the extension can still discover it.

Labby declares `io.modelcontextprotocol/skills` with an empty settings object —
supported, with no optional features. It does **not** declare `directoryRead`,
and a client must not call `resources/directory/read` against it.

### Methods

| Method | Behavior |
|--------|----------|
| `skills/list` | First-party skills plus every enabled, skills-proxying upstream the caller's route can reach |
| `skills/get` | One entry by URI. `-32602` means the URI is not a skill this server serves |
| `resources/read` | Serves `skill://` files. Skill URIs do **not** appear in `resources/list`; the manifest is the discovery surface |

### Product surfaces

Labby does not register a duplicate `skills` action tool. Agents use the native
`skills/list`, `skills/get`, and `resources/read` protocol methods. The local
CLI offers `labby skills list|search|get|read` for operator inspection. Managed
Artifact lifecycle operations are separate and are exposed through the
authenticated `artifacts` tool and `POST /v1/artifacts`.

### Authorization

`skills/list`, `skills/get`, and `skill://` reads require the same scope as
listing resources or prompts (`lab:read` and up) — **not** admin. Agents are the
intended consumers. The operator-facing `gateway.skills.list` action is separate
and does require admin, because it reports configuration state (which upstreams
opted in, what was excluded and why) rather than skill content.

Skills methods inherit the same per-client throttling posture as every other MCP
method. Labby has no generic MCP rate limiter, so they are not specially
protected — nor specially exposed.

### Operator-provided skills

Drop a skill directory into `$LABBY_HOME/skills/<name>/` and it is served under
the same reserved `labby` origin as the bundled ones — one first-party namespace
from a client's point of view. A bundled skill wins a name collision, so a
dropped-in directory cannot redefine what an existing `skill://labby/…` URI
means.

The tree is read **once at startup**, and each file's digest is computed from
the bytes read in that same pass. Adding or editing a skill therefore needs a
restart. That is deliberate: re-reading per request would let a file change
between publishing a digest and serving the file it describes, which is exactly
the mismatch a conforming client must refuse.

A skill is skipped, with a logged reason, when it contains a symlink at any
depth (the target could sit outside the root and would be served as first-party
content), has no `SKILL.md`, has a directory name disagreeing with its
frontmatter `name`, holds a file over 1 MiB, or holds more files than the
per-skill manifest cap. One bad directory never costs an operator their other
skills.

### Origin namespacing

Proxied skills are relabelled as `skill://<upstream-name>/…`. The label is
host-assigned, which is what the threat model requires: a skill from one origin
must never shadow a same-named skill from another. Nothing is deduplicated by
name — names are labels, not identifiers, and one server may legitimately serve
two skills sharing a final segment. `labby` is reserved for first-party skills
and an upstream with `proxy_skills` enabled may not claim it.

### Exposure and the manifest-bound read gate

A proxied skill is visible when the upstream sets `proxy_skills` (opt-in,
unlike `proxy_resources`/`proxy_prompts`), the skill passes `expose_skills`, and
the caller's route may reach that upstream.

Skill-file reads are **manifest-bound**: a read is granted because the URI
appears in a verified skill manifest, not because `expose_resources` allows it.
The two gates are independent — `expose_resources` neither grants nor blocks a
skill-file read, and a skill manifest cannot make an ordinary upstream resource
readable.

### What digest verification does not prove

The SEP is explicit that digests are unsigned, come from the same server as the
content, and that *"[a]ny intermediary on the path, such as a gateway, can
rewrite both the listing and the content together. Hosts MUST NOT treat a digest
match as a security boundary."*

Labby is exactly that intermediary. Verification here is a **consistency check**
— it catches corruption, truncation, and staleness after a skill is updated. It
is not tamper detection. A digest match proves the bytes are the ones the entry
described; it proves nothing about whether those bytes are safe.

### `allowed-tools` through a gateway

A skill's `allowed-tools` frontmatter names tools in its *origin's* namespace.
Downstream of Labby the catalog is aggregated, so those names could otherwise
resolve against a different server's tools — or against Labby's own privileged
ones.

Every aggregated entry therefore carries an `_meta` block under
`ai.dinglebear.labby/skillOrigin` describing what the origin actually accounts
for downstream:

| Field | Meaning |
|-------|---------|
| `label` | The host-assigned origin label, also the first URI segment |
| `toolAccess` | `direct` or `code_mode_only` |
| `reachableTools` | Present only when `direct`: the downstream tool names this origin accounts for |
| `note` | Present only when `code_mode_only`: why there is nothing to scope against |

Under Code Mode, raw upstream tools are hidden from `tools/list`, so
`reachableTools` is deliberately **omitted** rather than emitted empty —
publishing downstream names that do not exist would be a worse answer than
saying so plainly.

This lives in `_meta`, never in `frontmatter`. The SEP requires frontmatter to
be the author's YAML verbatim and requires a host to refuse the skill on any
field-by-field discrepancy against the fetched `SKILL.md`, so anything Labby
adds has to sit outside it.

Every value here is a fact about Labby's own catalog, never an interpretation of
skill content — the skill remains data, not directives.

### Enabling skills proxying is a trust decision

`proxy_skills` is flipped through `gateway.update`, which is `destructive: false`
and therefore not elicitation-gated the way `gateway.remove` is. Config mutation
is reversible and backup-first, so that classification stands — but enabling
skills aggregation means an upstream's instructions reach agents through Labby.
`gateway.skills.list` shows which upstreams have it on, along with each
catalog's cache age and what was excluded or truncated.
