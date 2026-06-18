# Observability

This document is the canonical observability contract for `lab`.

It defines:

- where instrumentation is mandatory
- which structured fields are required
- how caller context flows across boundaries
- what must never be logged
- what must be verified before a service is considered online

This is not optional guidance. Service integrations and shared infrastructure must conform to it.

## Goal

Every user-visible service action must be traceable end to end across:

- CLI dispatch
- MCP dispatch
- API dispatch
- shared SDK transport
- service health probes

When a request fails, operators must be able to answer:

- which surface invoked it
- which service and action ran
- which instance was targeted
- which outbound request was attempted
- whether the failure happened in validation, auth, transport, or server response handling

## Ownership

Observability is split across two layers:

- `lab` owns caller context and dispatch logging
- `lab-apis` owns outbound request logging and transport failure detail

That means:

- CLI, MCP, and API must log the user-visible action boundary
- `HttpClient` must log every outbound request
- service modules must not invent custom logging formats

## Mandatory Instrumentation Points

The following boundaries must emit structured logs.

### CLI Dispatch

Every CLI service action must emit one dispatch event.

Required fields:

- `surface = "cli"`
- `service`
- `action`
- `elapsed_ms`

Optional when applicable:

- `instance`
- `operation = "health"`
- `kind` on failure

### MCP Dispatch

Every MCP tool action must emit one dispatch event.

If the client has opted into MCP logging notifications, any notification derived
from that dispatch must reuse the same action context and apply the same
redaction rules before shipping error text back to the client.

Required fields:

- `surface = "mcp"`
- `service`
- `action`
- `elapsed_ms`

Optional when applicable:

- `instance`
- `operation = "health"`
- `kind` on failure
- `input_tokens` / `output_tokens` — estimated request/response token counts
  (≈chars/4 heuristic; `output_tokens = 0` on failure) on the dispatch finish event

### API Dispatch

Every product API service action must emit one dispatch event.

Required fields:

- `surface = "api"`
- `service`
- `action`
- `elapsed_ms`
- `request_id`

Optional when applicable:

- `instance`
- `operation = "health"`
- `kind` on failure
- `input_tokens` / `output_tokens` — estimated request/response token counts
  (≈chars/4 heuristic; `output_tokens = 0` on failure) on the dispatch finish event

This same contract applies to auth-adjacent HTTP handlers that are part of the
product surface, including:

- `/auth/session`
- `/auth/logout`
- `/v1/nodes/oauth/relay/start`
- OAuth authorize/callback/token handlers where `lab` itself is the actor

Those routes must not silently bypass the normal dispatch schema just because
they are not mounted under `/v1/{service}`.

### Actor Correlation

Operator-facing events that have an authenticated subject must use `actor_key`
for activity scoping instead of persisting or exposing the raw subject. The
actor key is:

- `HMAC-SHA256(subject, LAB_ACTOR_KEY_SECRET)`
- hex encoded as 64 lowercase characters
- stable for one installation as long as `LAB_ACTOR_KEY_SECRET` is preserved
- intentionally not portable across installations with different secrets

`LAB_ACTOR_KEY_SECRET` is a secret value stored in `~/.lab/.env`. If absent,
`lab` generates it on first use. Empty or anonymous subjects have no
`actor_key`; `mine_only` style activity queries must exclude those rows rather
than inventing a sentinel actor.

Compute `actor_key` once when binding an authenticated session, then clone that
bound value into later events. Do not derive it inside tracing subscriber
callbacks or per-log-event hot paths.

New activity-producing callsites should build events through
`observability::activity_event::ActivityEvent`. The builder takes typed
`Subsystem`, `Surface`, and `LogLevel` values from
`dispatch::logs::types`, then produces a `RawLogEvent` for the existing ingest
pipeline. Do not add new activity callsites that spell `surface` or
`subsystem` as string literals.

The raw subject remains a credential-adjacent identifier and must not be stored
in persisted log fields or returned to the Activity UI. A short redacted display
tag is allowed only for human diagnostics and must not be used for
authorization or filtering.

### Local Log Ingest Boundary

The local-master `logs` subsystem is a shared observability consumer, not a replacement for dispatch logging.

Rules:

- `main.rs` owns tracing setup and attaches the local log ingest layer once
- normalization and redaction happen before persistence and before SSE fanout
- the local store is fed from tracing-aware runtime events, not by scraping terminal output
- `/v1/logs/stream` is live push from the in-process subscriber hub, not database tailing
- reserved remote-ingest fields remain in the event model intentionally so future fleet and syslog ingest can converge on the same query contract without schema churn

### Device Runtime Ingest

Device-runtime HTTP handlers participate in the same API dispatch contract.

At minimum, the following actions must be traceable on the master:

- `device.status`
- `device.metadata`
- `device.syslog.batch`
- `device.logs.search`
- `device.oauth.relay.start`
- `fleet.ws.initialize`
- `fleet.ws.enrollment_required`
- `fleet.ws.log.event`

Non-master startup warnings for failed websocket connect, initialize, metadata upload, status push, or bootstrap log delivery must be logged without leaking device tokens or raw secret config content.

### Shared Outbound Requests

`lab-apis::core::HttpClient` must emit:

- one `request.start` event before every outbound call
- one `request.finish` event on success
- one `request.error` event on failure

This applies to all shared request helpers, including:

- `get_json`
- `get_json_query`
- `get_void`
- `post_json`
- `post_void`
- `put_json`
- `patch_json`
- `delete`
- `delete_query`

`HttpClient` logs must inherit the caller span from CLI, MCP, or HTTP dispatch.

### Outbound RMCP Client Requests

Outbound RMCP client operations are part of the same observability contract as
shared HTTP requests.

Every proxied upstream RMCP operation must emit:

- one start event before the outbound RPC
- one finish event on success
- one error event on failure or timeout

Required fields:

- `upstream`
- `capability`
- `operation`
- `elapsed_ms` on finish/error

When the call originates from API or HTTP MCP, the RMCP events must inherit the
surrounding caller context, including `request_id` when present. Timeouts must
be logged as explicit failures rather than disappearing into generic disconnect
noise.

For negotiated RMCP logging notifications sent back to MCP clients:

- reuse the same `surface/service/action/elapsed_ms[/kind]` payload shape as local dispatch logs
- omit `kind` on success notifications
- preserve the caller-derived failure severity (`warning` for caller/user errors,
  `error` for internal or upstream failures)

### Health Probes

Health probes are not normal business actions and must be distinguishable in logs.

When a health check runs, logs must include:

- `operation = "health"`

Health probes must also preserve the normal dispatch and request fields for their surface.

### Destructive Actions

Destructive actions must log:

- intent before execution
- outcome after execution

Intent logs must make it clear which action is about to mutate state. Outcome logs must indicate success or failure.

Gateway reconcile actions must log their mutation intent and outcome:

- `gateway.add`
- `gateway.update`
- `gateway.remove`
- `gateway.reload`

Those actions must also log reconcile phase transitions and outcome details
without exposing credential-bearing URLs, commands, tokens, or secret env
values.

## Required Fields

### Dispatch Events

All dispatch events must include:

- `surface`
- `service`
- `action`
- `elapsed_ms`

Failure events must also include:

- `kind`

Additional fields when applicable:

- `instance`
- `request_id`
- `operation`
- `upstream`
- `capability`

### Request Events

All `HttpClient` request events must include:

- `method`
- `path`
- `host`

`request.finish` must also include:

- `status`
- `elapsed_ms`

`request.error` must also include:

- `elapsed_ms`
- `kind`
- `message`

If the implementation logs a URL, it must be redacted and must not contain secrets or embedded credentials.

## Correlation Rules

Caller context must flow downward.

Rules:

- CLI spans must wrap SDK calls
- MCP spans must wrap SDK calls
- HTTP spans must wrap SDK calls
- `HttpClient` request events must inherit those spans rather than creating detached logs

The practical result must be:

- outbound request logs can be tied back to the invoking surface
- HTTP-originated requests can be tied back to a `request_id`
- multi-instance requests can be tied back to an `instance`
- outbound RMCP proxy activity can be tied back to the invoking surface and
  request when one exists

For device-runtime uploads, operators must be able to correlate:

- the non-master startup or flush attempt
- the outbound request to the master
- the master-side device ingest handler

## Error Classification

The public error taxonomy remains the stable contract.

Relevant kinds include:

- `auth_failed`
- `not_found`
- `rate_limited`
- `validation_failed`
- `network_error`
- `server_error`
- `decode_error`
- `internal_error`

Dispatch layers may also emit:

- `unknown_action`
- `unknown_subaction`
- `missing_param`
- `invalid_param`
- `unknown_instance`

Transport failures must preserve enough message detail to distinguish likely classes such as:

- DNS resolution failure
- TCP connection failure
- TLS certificate validation failure
- timeout

Those details may live in the error message while still mapping to the stable `network_error` kind.

## Redaction Rules

The following data must never be logged:

- API keys
- bearer tokens
- passwords
- cookies
- authorization headers
- secret env values

Additional rules:

- do not log full request headers unless explicitly sanitized
- do not log request bodies by default
- do not log query parameters when they contain secrets
- do not echo secrets in doctor output, prompts, logs, generated docs, or UI flows
- do not log raw discovered MCP config file contents; only metadata such as path, source, and hash are acceptable
- do not persist bearer tokens, cookies, authorization headers, or raw secret material in the local log store
- do not fan out unredacted structured fields to live SSE subscribers
- upstream-controlled field values (tool names, prompt names, resource URIs from external MCP servers)
  must be sanitized before rendering in human log output — strip Unicode control characters except
  tab and newline to prevent ANSI escape injection. `sanitize_field_value()` in
  `log_fmt/formatter.rs` is the canonical implementation; apply it before any terminal styling.
- `resource_uri` field values must have query strings and fragments stripped before logging
  (`redact_resource_uri_for_logging()` in `dispatch/upstream/pool.rs`). Pre-signed S3 tokens,
  OAuth params, and similar credential-bearing query parameters must not appear in log output.
- upstream URL values must have userinfo (username:password) stripped before logging
  (`upstream_target_redacted()` in `dispatch/upstream/pool.rs`).

Shell wrapper boundary: the user-installed `lab` shell wrapper emits CLI-PREFLIGHT output via `printf` to
stderr before the Rust binary starts. This output is pre-binary and therefore not processed by
`init_tracing()`, `LogIngestLayer`, or any redaction rules. Treat it as an unstructured stderr
boundary — it must not emit credential-bearing content.

### Upstream OAuth Redaction

The outbound upstream OAuth flow (see [UPSTREAM.md](../services/UPSTREAM.md)) adds the following fields to the never-log list. They must not appear at any level, in dispatch events, request logs, tracing spans, error messages, or MCP notifications:

- OAuth `code` (authorization code from the callback)
- OAuth `state` (CSRF token)
- PKCE `code_verifier`
- `access_token`, `refresh_token`, and `id_token` from any token response
- the raw `token_response_json` payload
- `token_blob` ciphertext and `token_blob_nonce`
- `client_secret` (from the `*_CLIENT_SECRET` env var named by `client_secret_env`)
- `Authorization` headers constructed from upstream OAuth tokens
- `LAB_OAUTH_ENCRYPTION_KEY`

Credential and state row types implement `Debug` manually to enforce this; never `#[derive(Debug)]` on them.

## Level Rules

Use these level conventions consistently:

- `INFO` for successful dispatch and successful request completion
- `WARN` for expected caller or service failures such as validation, auth, or not found
- `ERROR` for unhandled or internal failures

Do not use ad hoc `println!` debugging in place of structured logs.

## Verification Requirements

A service is not considered online until observability is verified.

Minimum verification:

1. one successful action shows a dispatch event and downstream request events
2. one failing action shows a dispatch failure with a stable `kind`
3. the failing path preserves enough transport or response detail to diagnose the class of failure
4. logs do not expose secrets

Verification may use:

- unit tests for shared helpers
- mock-server tests for request behavior
- live read-only smoke tests against a real service when available

Destructive actions do not need live verification by default, but their intent and outcome logging must follow the same contract.

## Onboarding Gate

When bringing a new service online, observability is required before the service is complete.

That means the service must have:

- dispatch logging at every public surface it exposes
- shared `HttpClient` request logging for its outbound calls
- correct error kind mapping
- redaction compliance
- verification evidence that the request path is traceable end to end

If those conditions are missing, the service is not fully online even if the CLI, MCP, or HTTP action itself works.

## Example Shapes

Illustrative success fields:

```json
{
  "surface": "http",
  "service": "marketplace",
  "action": "mcp.list",
  "request_id": "req-123",
  "method": "GET",
  "path": "/v0.1/servers",
  "host": "registry.modelcontextprotocol.io",
  "status": 200,
  "elapsed_ms": 42
}
```

Illustrative failure fields:

```json
{
  "surface": "cli",
  "service": "marketplace",
  "action": "mcp.list",
  "method": "GET",
  "path": "/v0.1/servers",
  "host": "registry.modelcontextprotocol.io",
  "kind": "network_error",
  "message": "registry request failed",
  "elapsed_ms": 311
}
```
