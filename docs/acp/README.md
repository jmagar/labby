---
title: ACP Docs
created_at: 2026-04-23 17:03:03 EDT
updated_at: 2026-04-23 17:41:01 EDT
status: draft
owner: lab
---

# ACP

This directory is the documentation entrypoint for ACP in `lab`.

ACP is the first-class product-local service that owns conversational session
orchestration, provider runtime lifecycle, prompt execution, transcript
assembly, and event streaming.

The browser UI route remains `chat`, but the canonical backend service name is
`acp`.

## Scope

ACP covers:

- provider health and runtime lifecycle
- session creation, listing, prompting, cancellation, and resume semantics
- event persistence, replay, sequencing, and SSE delivery
- transcript-oriented session state for browser and machine-facing consumers
- provider-agnostic runtime orchestration
- raw `usage_update` preservation
- raw `ContentBlock[]` preservation and rendering
- marketplace deployment as a target for ACP/chat agents
- future CLI, MCP, and API surfaces over one shared dispatch layer

ACP does not own:

- upstream MCP configuration, discovery, auth, exposure, or routing
- browser-specific presentation concerns
- direct gateway-admin UI layout decisions

Those remain separate:

- `gateway` is the MCP control plane
- `chat` is the browser UI over ACP

## Canonical documents

- [design.md](./design.md) — formal ACP design spec and architecture record

## Current direction

The target architecture is:

`browser -> acp service -> gateway runtime -> upstream MCP`

Key locked decisions:

- canonical first-class service name: `acp`
- browser route name: `chat`
- ACP core capability logic belongs in `lab-apis`
- ACP surface adapters belong in `lab`
- ACP integrates with gateway through a narrow in-process interface
- SSE remains the default event-stream transport
- ACP runtime is provider agnostic
- minimum provider targets are Codex, Claude, Gemini, GitHub Copilot, and OpenCode
- marketplace deployments should be able to target ACP/chat agents directly
- ACP should preserve raw `usage_update` and raw `ContentBlock[]`
- ACP should invest in full `ContentBlock[]` rendering
- ACP Registry compatibility should remain a first-class direction so users can
  install additional agents/providers over time

## HTTP API

The preferred machine-facing HTTP entrypoint is the shared service dispatch
route:

```http
POST /v1/acp
Content-Type: application/json

{
  "action": "session.start",
  "params": {
    "provider": "codex",
    "cwd": "/home/example/project",
    "title": "Investigate build"
  }
}
```

The request body matches the MCP service contract: `action` is one ACP action
name from the catalog and `params` is the action-specific object. Authenticated
HTTP session actions are scoped to the caller principal by the API adapter.
Destructive actions use the shared HTTP confirmation gate and require
`params.confirm: true`.

The REST-shaped browser compatibility routes under `/v1/acp/sessions/*` remain
available for the hosted chat UI. SSE event delivery is the transport exception:
browser clients still stream events from
`GET /v1/acp/sessions/{session_id}/events?ticket=...`.

## Provider prompt idle timeout

The ACP runtime watches for provider updates while a prompt is in flight and
will close the prompt loop on its own if the provider goes silent after it has
already started speaking.

- **Purpose.** Some providers stream assistant output but never emit a terminal
  `StopReason`, leaving the prompt loop blocked on `read_update()` forever.
  Once at least one assistant chunk has been seen, the runtime arms an idle
  timer; if no further provider update arrives within that window, the runtime
  treats the prompt as completed and tears the loop down.
- **Default.** 5 seconds (`Duration::from_secs(5)`). Defined in
  `crates/lab/src/acp/runtime.rs` as `DEFAULT_PROMPT_IDLE_TIMEOUT`.
- **Override.** Set `LAB_ACP_PROMPT_IDLE_TIMEOUT_MS` to a positive integer
  number of milliseconds. Zero, missing, and unparseable values fall back to
  the default. The override is read per-tick from the environment, so changes
  take effect for new prompts without restarting the binary.
- **Behavior when it fires.** The runtime emits two SSE events on the session
  stream and then exits the prompt read loop:
  1. a `session_state` update transitioning the session to `Completed`, and
  2. a `provider_info` event with
     `{"type":"idle_completion","title":"Prompt completed after provider idle timeout","status":"completed","timeout_ms":<value>}`.
  The prompt lifecycle is marked finished; the session itself remains
  registered and can accept a new prompt. The timer only arms after the first
  assistant output chunk — providers that never produce output are not
  short-circuited by this timeout (cancellation and process-level supervision
  cover that case).
- **Tuning guidance.** Raise this value when working with slow providers that
  pause mid-response (for example, large tool batches or long thinking
  pauses). Lower it for snappier UX with chatty providers that reliably emit
  a stop reason. The companion `LAB_ACP_PERMISSION_TIMEOUT_MS` controls a
  different timer (permission decisions) and is documented separately.

## Status

ACP is registered as a first-class always-on service. The pieces in place:

- `lab-apis::acp` — capability module with `META`, `AcpEvent`, `AcpSessionState`,
  `AcpSessionSummary`, `AcpPersistence` trait, and bounded `SessionHandle`
  abstraction.
- `crates/lab/src/dispatch/acp/` — shared dispatch layer (`catalog.rs`,
  `client.rs`, `params.rs`, `dispatch.rs`, `persistence.rs` SQLite impl,
  `page_context.rs` sanitizer).
- `crates/lab/src/acp/` — runtime, registry, providers, persistence (legacy
  JSON file fallback).
- `crates/lab/src/api/services/acp.rs` — HTTP surface, both `POST /v1/acp`
  shared-action route and the REST-shaped browser compatibility routes for SSE.
- Registration in `crates/lab/src/registry.rs` so the shared catalog and MCP
  envelope discover ACP automatically.

Phase 2 work that is still pending:

- Typed CLI subcommands (`lab acp ...`). Today the catalog is reachable from
  HTTP and via the shared dispatch path, but there is no clap-typed CLI shim.
- Browser-facing UI contract refinements tracked in [design.md](./design.md).

## Security and runtime posture

This section reflects landed protections from the review remediation epic and
the remaining gaps. It does not claim work that has not shipped.

Landed:

- Provider filesystem capabilities are disabled at the `ClientCapabilities`
  level until a contained workspace policy and permission flow exist —
  provider-side `fs.read_text_file` and `fs.write_text_file` are off
  (`runtime.rs::lab_client_capabilities`).
- Permission decisions are explicit: there is no auto-approval path. Each
  permission request emits an event and waits for an authenticated decision
  bounded by `LAB_ACP_PERMISSION_TIMEOUT_MS` (default 60 s).
- HTTP authentication propagates `AuthContext.sub` to the registry; sessions
  are bound to the creating principal and subscribe/prompt/cancel/close
  enforce the binding. Anonymous principals are rejected at the API
  boundary.
- Browser SSE attaches a `subscribe_ticket` per stream rather than relying on
  cookie auth alone.
- ACP Registry installs validate `agent_id` before any path is constructed,
  blocking traversal and shell metacharacter injection in install dirs.
- ACP Registry binary installs accept HTTPS archives only, reject local/private
  archive hosts before download, pin validated resolved addresses into the
  download client, and abort streams above the documented 256 MiB archive cap
  while removing partial files.
- Provider subprocesses spawn with `env_clear()` and a fixed allowlist
  (`PATH`, `HOME`, locale vars, terminal vars, Windows `SystemRoot`). Per-
  provider entries can extend this allowlist explicitly via the structured
  `env` field on `AcpProviderEntry`.
- Provider commands and arguments are stored as a structured
  `command + args + cwd + env` shape in `acp-providers.json`. Quoted args
  and paths-with-spaces round-trip verbatim. Legacy entries without an
  `args` key fall back to whitespace-splitting `command` (one-time read
  fidelity gap; re-installing migrates the entry).
- Docker-hosted Codex ACP providers must disable Codex's inner filesystem
  sandbox with `sandbox_mode="danger-full-access"` in the provider args or
  `CODEX_HOME/config.toml`. The Docker container is the outer isolation
  boundary; Docker's default seccomp profile blocks the nested namespace
  setup used by Codex `workspace-write` and `read-only` modes. At ACP registry
  startup, Lab emits a `container_sandbox_incompatible` warning if it detects
  a container runtime with an unsafe Codex sandbox mode.
- Provider stderr is line-redacted and length-capped before being forwarded
  on the SSE event stream (`MAX_PROVIDER_STDERR_CHARS`, redaction via
  `dispatch::redact`).
- Per-session command and prompt queues are bounded
  (`SESSION_COMMAND_QUEUE_CAPACITY`). The per-session AcpEvent channel
  feeding the registry hub is bounded at `ACP_EVENT_CHANNEL_CAPACITY` (1024)
  and back-pressures to the provider's stdio reader on persistence stalls
  rather than growing memory unboundedly.
- HMAC-signed `permission_outcome` payloads detect tampering of persisted
  decisions. The fallback ephemeral key path emits truthful metadata about
  its persistence model so operators do not assume cross-restart guarantees.
- SSE backfill is capped at the SQL layer
  (`load_events_since_capped(.., BACKFILL_CAP=10_000)`), preserving "last N
  events" semantics without materialising the full event range in memory.
- Page-context sanitization is predicate-based with an explicit deny-list
  for prompt-injection terms; the allowed character set is structural
  (`is_safe_page_context_char`) rather than a hand-spelled `&[char]`. The
  policy is documented in the source.
- The provider prompt idle timeout
  (`LAB_ACP_PROMPT_IDLE_TIMEOUT_MS`, default 5 s) and its observable firing
  behavior are documented above.

Remaining gaps tracked but not closed in this remediation pass:

- The legacy `Bridge*` compatibility projection types
  (`crates/lab/src/acp/types.rs`) are still consumed by the legacy JSON-file
  persistence and mirrored by the frontend. Removing them is a coordinated
  Rust + frontend wire-format change deferred until the legacy
  `JsonFileAcpPersistence` is retired.
- Provider sandboxing beyond the disabled-capabilities posture is out of
  scope here. Workspace jails and permission-flow-driven file access remain
  future work tracked separately.
- Typed `lab acp ...` CLI subcommands are not yet shipped (Phase 2).
- The on-disk `acp-providers.json` format includes structured `args`, `cwd`,
  and `env` fields that the install paths populate, but pre-existing
  installations written before this format change still serialize through
  the legacy fallback. Re-install migrates one entry at a time on demand;
  there is no batch migration script.
