# Errors

This document is the canonical error-handling contract for `lab`.

It defines:

- the shared transport error taxonomy
- the dispatcher-level error vocabulary
- the required envelope shapes for MCP and HTTP
- status-code mapping expectations
- when changing error kinds is a spec change

## Goal

Errors must be:

- stable across services
- machine-readable across transports
- structured enough for agents and operators to react programmatically
- specific enough to diagnose the failure class without inventing per-service vocabularies

## Ownership

Error handling is split across layers:

- `lab-apis` owns the canonical shared transport taxonomy via `ApiError`
- service modules may wrap `ApiError` with service-specific errors
- `lab` dispatch layers add caller and validation errors on top
- MCP and HTTP must emit stable structured envelopes derived from those kinds

## Canonical SDK Taxonomy

The shared transport taxonomy lives in `lab-apis::core::ApiError`.

Stable `kind()` values are:

- `auth_failed`
- `not_found`
- `rate_limited`
- `validation_failed`
- `network_error`
- `server_error`
- `decode_error`
- `internal_error`

These kinds are consumed by MCP and HTTP callers. Changing them is a spec change.

## Dispatcher-Level Kinds

Dispatch layers may add the following kinds on top of SDK errors:

- `unknown_action`
- `unknown_subaction`
- `missing_param`
- `invalid_param`
- `unknown_instance`
- `conflict` — resource already exists with the given identifier; HTTP 409
- `ambiguous_tool` — unqualified tool name resolved to multiple upstream gateway candidates; envelope carries `valid: Vec<String>` of fully-qualified `{upstream}::{tool}` names the caller must choose from, plus a `hint` explaining that callers may either pass `name = "{upstream}::{tool}"` or set `upstream` separately. HTTP 409.
- `invalid_code_mode_id` — Code Mode tool id parsing failed. Valid ids are `lab::<service>.<action>` and `upstream::<upstream-name>::<tool-name>`. HTTP 422.
- `code_mode_disabled` — Code Mode execution was requested while `[code_mode].enabled` is false. Discovery and schema lookup can remain enabled without allowing execution. HTTP 403.
- `code_execution_failed` — Code Mode child-process JavaScript evaluation failed before completing the runner protocol. HTTP 422.
- `tool_call_limit_exceeded` — a Code Mode snippet attempted more host-brokered tool calls than `max_tool_calls` allows. HTTP 429.
- `schema_unavailable` — Code Mode schema lookup found a tool, but its upstream schema was missing or exceeded the safe return size after sanitization. HTTP 422.
- `queue_saturated` — bounded runtime queue is full; caller should retry after the current work drains. HTTP 429.

### Fleet-WS install hardening kinds (lab-zxx5.18)

- `symlink_rejected` — the target write path (or a component along it) is a symlink. Emitted by `marketplace.install_component` when `write_atomic`'s defense-in-depth check finds the tempfile is a symlink or the target's parent chain resolves through a symlink outside `write_root`. HTTP 422.
- `path_traversal_rejected` — the relative component path contains `..`, `.`, or is absolute; or the canonicalized target resolves outside the install root. Raised before any write. HTTP 422.
- `content_too_large` — a single component exceeds `MAX_COMPONENT_FILE_SIZE` (5 MB) or the aggregate of all components in one `install_component` RPC exceeds `MAX_COMPONENT_AGGREGATE_SIZE` (32 MB). Enforced before the handler is spawned so oversized payloads can't OOM the node or lock a worker permit. HTTP 413.
- `invalid_encoding` — an install_component `files[].encoding` is missing, not `"utf8"` or `"base64"`, or the base64 payload fails to decode. No implicit fallback — explicit encoding is required to defeat utf8/base64 ambiguity. HTTP 422.
- `install_timeout` — the `agent.install` download watchdog fired (no bytes received for 30s), or the overall RPC ack timeout elapsed. The partial tempfile is cleaned up on fire. HTTP 504.

### mcpregistry-specific kinds

- `no_remote_transport` — `server.install` called on a server with no HTTP remote transports (stdio-only); cannot be added as a gateway upstream
- `ssrf_blocked` — registry-sourced URL resolves to a private, loopback, link-local, or ULA address; blocked to prevent SSRF
- `sync_in_progress` — a registry sync is already running; callers should retry later. HTTP status: 503.
- `integrity_missing` — registry-sourced executable/package metadata lacks required SHA-256 integrity data; install fails closed. HTTP status: 502.
- `integrity_mismatch` — downloaded executable/package bytes do not match registry-provided SHA-256 metadata; install fails closed. HTTP status: 502.

`no_remote_transport` and `ssrf_blocked` use `ToolError::Sdk { sdk_kind, message }`. HTTP status: 422.
`sync_in_progress` uses `ToolError::Sdk { sdk_kind, message }`. HTTP status: 503.
`integrity_missing` and `integrity_mismatch` use `ToolError::Sdk { sdk_kind, message }`. HTTP status: 502.

### Stash-specific kinds

The following kinds are emitted by the `stash` dispatch service.

- `conflict` — advisory lock timed out waiting for exclusive access to a component (re-uses the global `conflict` kind; HTTP 409).
- `unsupported_provider` — provider kind is not implemented, or a remote gateway deploy was requested for a provider that only supports direct filesystem sync. HTTP 422.
- `unsupported_component_kind` — a requested operation is not valid for the component's `kind` (e.g. requesting binary execution for a `skill`). HTTP 422.
- `sync_failed` — provider push or pull failed due to an I/O error on the provider's remote root. HTTP 502.
- `workspace_too_large` — the component workspace exceeds `MAX_WORKSPACE_SIZE` (200 MiB) before a save or import. HTTP 413.
- `file_too_large` — a single file inside the workspace exceeds `MAX_FILE_SIZE` (50 MiB). HTTP 413.
- `path_traversal` — a path escapes the target root (re-uses global `path_traversal_rejected`; emitted during import and export). HTTP 422.
- `symlink_rejected` — a symlink was encountered during a workspace walk (re-uses global `symlink_rejected`; emitted during save, import, and export). HTTP 422.
- `export_target_not_empty` — the output directory for `component.export` is non-empty and `force` is not set. HTTP 409.
- `ambiguous_kind` — component kind could not be auto-detected from the source path and no `kind` override was provided. HTTP 422.

### Setup / env_merge kinds (lab-bg3e.3)

Stable kinds emitted by `crates/lab/src/config/env_merge.rs` (the shared
`.env` merge primitive used by `setup.draft.commit` and, later,
`extract.apply`). Surfaced through `MergeError::kind()` and pass-through to
`ToolError::Sdk { sdk_kind }` envelopes.

- `merge_temp_create` — could not create the same-directory `tempfile` used
  for atomic write. Filesystem permission or quota issue on the parent dir.
  HTTP 500.
- `merge_sync_failed` — `File::sync_all` on the temp file returned an error
  before persist. Indicates an I/O or storage-backend failure mid-flush.
  HTTP 500.
- `merge_persist_cross_fs` — `tempfile::persist()` was rejected because the
  temp and target are on different filesystems (EXDEV). The merge module
  always allocates the temp inside the target's parent dir, so this should
  not surface in practice — emit only as a defensive signal. HTTP 500.
- `merge_write_conflict { reason }` — the merge aborted before persist
  because the target's mtime changed since the caller's snapshot
  (`reason: "mtime_skew"`) or, in v2, fs2 lock contention
  (`reason: "lock_contention_v2"`). v1 only emits `mtime_skew`. HTTP 409.
- `write_failed { reason }` — generic post-temp write failure; `reason` is
  one of `storage_full`, `permission_denied`, or `other(os_msg)`. HTTP 500.
- `commit_rollback_failed` — `setup.draft.commit` attempted a rollback to
  the most recent backup and the rollback itself failed. The envelope names
  the backup path so an operator can recover manually. HTTP 500.
- `audit_timeout` — `setup.draft.commit` aborted because the inline
  `doctor.audit.full` call did not return within `AUDIT_TIMEOUT` (30s).
  Caller should retry after fixing whichever service probe is hanging
  (typically a misconfigured `*_URL` for an unreachable host). HTTP 504.

Removed from drafts (not used in code; do not reintroduce):
`merge_locked_by_other`, `merge_concurrent_write`, `backup_failed_disk_full`,
`preflight_failed`. The first two collapse into `merge_write_conflict`; the
third into `write_failed`; the fourth is unnecessary because
`setup.draft.commit` returns the doctor.audit.full body inline on failure
instead of double-wrapping.

### Marketplace artifact update kinds

- `git_not_available` — `artifact.update.check` could not spawn `git`. Install git on the controller host to use update checking. HTTP 500.
- `marketplace_auth_required` — `artifact.update.check` received git exit code 128 while fetching a marketplace; the message names the marketplace and does not include credentials or git stderr. HTTP 401.
- `not_forked` — an artifact update action was requested for a plugin without forked `.stash.json` metadata. HTTP 404.
- `stale_preview` — `artifact.update.apply` was called with a pending preview whose upstream fingerprint no longer matches the current marketplace source. Caller must run `artifact.update.preview` again. HTTP 409.
- `ai_backend_not_configured` — `artifact.merge.suggest` or AI merge application needs an AI backend, but no merge backend is configured. HTTP 422.
- `content_contains_secrets` — `artifact.merge.suggest` rejected changed artifact content before transmission because it matched credential-like patterns. HTTP 422.

Additional MCP destructive-confirmation flow-control case:

- `confirmation_required`

### Auth Protocol Exception

`invalid_grant` is a documented auth-route exception for OAuth protocol
failures. It is emitted by the auth server for invalid, expired, reused, or
mismatched authorization codes and refresh tokens.

- surface: HTTP auth routes only
- status: `400 Bad Request`
- contract owner: `docs/OAUTH.md`

This kind does not replace the canonical shared SDK taxonomy for service
dispatch. It exists because OAuth token endpoints have a protocol-level error
vocabulary that callers expect.

### HTTP-Only Dispatcher Kinds

The following kinds are emitted exclusively by the HTTP surface. MCP handles the same guard differently (via elicitation), and CLI handles it via `--yes` / `-y`.

#### `confirmation_required`

**When:** A destructive action (`ActionSpec.destructive == true`) is dispatched over HTTP without `params["confirm"] == true`.

**Surface:** HTTP only. MCP uses elicitation; CLI requires `--yes`.

**Resolution:** Set `"confirm": true` inside the request body's `params` object and re-submit.

**Status code:** `422 Unprocessable Entity`

**Envelope:**

```json
{ "kind": "confirmation_required", "message": "action `snippets.delete` is destructive — set `confirm: true` in params to proceed" }
```

**Implementation note:** Emitted as `ToolError::Sdk { sdk_kind: "confirmation_required" }` from `handle_action` in `crates/lab/src/api/services/helpers.rs`.

### MCP-Only Dispatcher Kinds

#### `upstream_error`

**When:** A proxied upstream MCP server call fails — connection lost, timeout, response too large (`LAB_UPSTREAM_MAX_RESPONSE_BYTES`, default 10 MB), or the upstream returned an error.

**Surface:** MCP only. Upstream proxy is MCP-transport infrastructure.

**Resolution:** Check upstream server health. Review circuit breaker status via `lab://catalog` or logs. If the upstream is consistently failing, it will be excluded from tool listings after 3 consecutive failures.

**Status code:** `502 Bad Gateway` (when mapped to HTTP, e.g. in error.rs)

**Envelope:**

```json
{ "kind": "upstream_error", "message": "upstream `my-server` call failed: connection refused" }
```

Do not invent new kinds casually. If a new cross-service kind is needed, update the owning docs and all public surfaces together.

### Upstream OAuth Kinds

The upstream OAuth (outbound) surface adds five stable kinds for operator- and user-facing failures in the authorization-code + PKCE flow against OAuth-protected upstream MCP servers. Full flow documented in [UPSTREAM.md](../services/UPSTREAM.md).

#### `oauth_needs_reauth`

**When:** The persisted upstream OAuth credential can no longer be used to obtain a valid access token, and the user must re-initiate authorization. Concrete triggers:

- the authorization server returned `invalid_grant` on refresh (refresh token revoked, rotated twice, or otherwise invalidated)
- the encrypted `token_blob` failed to decrypt (for example after `LAB_OAUTH_ENCRYPTION_KEY` rotation)
- a 401 was received on a non-idempotent request and retry is not safe
- no persisted credential exists yet for the `(upstream, subject)` pair

**Surface:** MCP proxied calls, `/mcp`, hosted UI, `/v1/gateway/oauth/status`.

**Resolution:** Start a fresh authorization via `POST /v1/gateway/oauth/start`.

**Status code:** `401 Unauthorized`.

#### `oauth_state_invalid`

**When:** The callback at `/auth/upstream/callback` cannot match the `state` parameter to a live pending-state row for the authenticated subject and requested upstream. Causes: missing session, replayed `state`, expired state (>10 min), cross-subject attempt, or cross-upstream-name attempt.

**Surface:** `/auth/upstream/callback` only.

**Resolution:** Re-initiate authorization.

**Status code:** `400 Bad Request`.

#### `oauth_resource_mismatch`

**When:** The authorization server refused the RFC 8707 `resource` parameter, or the returned access token's `aud` claim does not match the canonical upstream MCP URL.

**Surface:** Upstream OAuth manager (begin / callback / build_auth_client).

**Resolution:** Operator must verify the upstream MCP server URL in config and the AS registration match.

**Status code:** `502 Bad Gateway`.

#### `oauth_issuer_mismatch`

**When:** The AS metadata `issuer` is missing, or an endpoint origin (scheme + host + port) does not match the issuer origin (RFC 8414 §3.3 requirement).

**Surface:** Upstream OAuth manager (discovery).

**Resolution:** Operator must contact the upstream AS owner; this is an RFC 8414 §3.3 violation.

**Status code:** `502 Bad Gateway`.

#### `oauth_unsupported_method`

**When:** The upstream AS metadata omits `code_challenge_methods_supported` or advertises only `plain`. `lab` refuses to fall back from S256.

**Surface:** Upstream OAuth manager (discovery).

**Resolution:** The upstream AS must advertise `S256`. No workaround.

**Status code:** `502 Bad Gateway`.

## Wrapping Rules

Service-specific errors must:

- wrap `ApiError` transparently where possible
- preserve the underlying `kind()` semantics for transport-layer failures
- avoid forking the shared taxonomy into service-local equivalents

Public surface code must not stringify and discard the error kind.

### `From<ServiceError> for ToolError` Placement

All `From<XError> for ToolError` impls live in `crates/lab/src/dispatch/error.rs`,
feature-gated to their service. This ensures both MCP and HTTP surfaces share a
single conversion path. Do not place these impls in `mcp/services/` or
`api/services/` — that traps the conversion in one surface module.

Pattern:

```rust
#[cfg(feature = "foo")]
impl From<lab_apis::foo::error::FooError> for ToolError {
    fn from(e: lab_apis::foo::error::FooError) -> Self {
        let kind = match &e {
            FooError::Api(api) => api.kind(),
            FooError::NotFound { .. } => "not_found",  // service-specific variants
        };
        Self::Sdk {
            sdk_kind: kind.to_string(),
            message: e.to_string(),
        }
    }
}
```

## MCP Contract

MCP error responses must be structured and machine-readable.

Canonical MCP error envelope:

```json
{
  "ok": false,
  "service": "radarr",
  "action": "movie.add",
  "error": {
    "kind": "missing_param",
    "message": "missing parameter: root_folder"
  }
}
```

Rules:

- `kind` is the stable semantic tag
- `message` is human-readable diagnostic text
- additional structured keys such as `param`, `valid`, or `hint` may be included where relevant
- clients must not need to parse free-form prose to classify the error

## HTTP Contract

HTTP error responses must use the same semantic `kind` vocabulary as MCP.

Canonical HTTP error envelope:

```json
{
  "kind": "auth_failed",
  "message": "authentication failed"
}
```

Rules:

- HTTP and MCP must agree on the semantic kind
- HTTP may use transport-appropriate status codes, but the JSON body remains structured
- HTTP must not invent a second vocabulary for the same failure class
- auth/session/logout/token routes must either use this envelope directly or
  document a protocol-required exception in the owning auth docs

Auth-specific rule:

- session-store, database, provider, and signing-key failures are internal
  failures, not "logged out" outcomes
- handlers must not downgrade store/provider outages into successful
  unauthenticated responses

## HTTP Status Mapping

Default mapping expectations:

- `auth_failed` -> `401 Unauthorized`
- `not_found` -> `404 Not Found`
- `rate_limited` -> `429 Too Many Requests`
- `validation_failed` -> `422 Unprocessable Entity`
- `missing_param` -> `422 Unprocessable Entity`
- `invalid_param` -> `422 Unprocessable Entity`
- `unknown_action` -> `400 Bad Request`
- `unknown_instance` -> `400 Bad Request`
- `ambiguous_tool` -> `409 Conflict`
- `conflict` -> `409 Conflict`
- `symlink_rejected` -> `422 Unprocessable Entity`
- `path_traversal_rejected` -> `422 Unprocessable Entity`
- `invalid_encoding` -> `422 Unprocessable Entity`
- `content_too_large` -> `413 Payload Too Large`
- `install_timeout` -> `504 Gateway Timeout`
- `confirmation_required` -> `422 Unprocessable Entity`
- `sync_in_progress` -> `503 Service Unavailable`
- `stale_preview` -> `409 Conflict`
- `ai_backend_not_configured` -> `422 Unprocessable Entity`
- `content_contains_secrets` -> `422 Unprocessable Entity`
- `invalid_grant` -> `400 Bad Request`
- `network_error` -> `502 Bad Gateway`
- `server_error` -> `502 Bad Gateway`
- `upstream_error` -> `502 Bad Gateway`
- `oauth_needs_reauth` -> `401 Unauthorized`
- `oauth_state_invalid` -> `400 Bad Request`
- `oauth_resource_mismatch` -> `502 Bad Gateway`
- `oauth_issuer_mismatch` -> `502 Bad Gateway`
- `oauth_unsupported_method` -> `502 Bad Gateway`
- `internal_error` -> `500 Internal Server Error`

## Deploy Service Kinds

The `deploy` service (feature-gated) adds the following stable kinds, all
surfaced via `DeployError::kind()` in `lab-apis/src/deploy/error.rs`:

| `kind` | HTTP status | Meaning |
|--------|-------------|---------|
| `validation_failed` | 422 | Bad input (host alias, remote_path allowlist, etc.). _(shared kind)_ |
| `auth_failed` | 401 | `LAB_DEPLOY_TOKEN` missing or headless `confirm: true` rejected. _(shared kind)_ |
| `ssh_unreachable` | 502 | SSH connection or auth failed for a target. |
| `build_failed` | 502 | Local `cargo build --release --all-features -p labby` failed. |
| `preflight_failed` | 502 | Remote arch probe, writable-dir check, or sha256 probe failed. |
| `transfer_failed` | 502 | Streaming the artifact to the remote failed. |
| `install_failed` | 502 | Atomic rename/backup on the remote failed. |
| `restart_failed` | 502 | `systemctl restart` or `is-active --wait` failed. |
| `verify_failed` | 502 | Post-install `lab --version` probe failed. |
| `partial_failure` | — | Multi-host run where some hosts failed; returned as HTTP 200 with `ok=false` in the body, not as an error response. |
| `conflict` | 409 | Another deploy is in progress for the same host. |
| `arch_mismatch` | 502 | Remote `uname -m` differs from local build triple. |
| `integrity_mismatch` | 502 | Remote sha256 of staged artifact differs from local, or registry-sourced executable/package bytes differ from expected SHA-256 metadata. |

The deploy-specific kinds (`ssh_unreachable`, `build_failed`, `preflight_failed`,
`transfer_failed`, `install_failed`, `restart_failed`, `verify_failed`,
`arch_mismatch`, `integrity_mismatch`, `conflict`) are registered in
`api/error.rs::IntoResponse` so they map to the correct HTTP status codes
when the deploy HTTP surface is wired.

MCP envelopes carry the redacted message from `DeployError::redacted_message()`;
the full structured detail is logged at WARN locally.

## Device Runtime Notes

The device runtime uses the same shared taxonomy.

Important cases in this implementation:

- master-only fleet query routes on a non-master device return `not_found`
- invalid OAuth relay target input returns `invalid_param`
- missing fleet store wiring returns `internal_error`
- failed master-bound HTTP uploads map through the normal transport-layer kinds rather than inventing device-local variants

## Message Rules

Messages must help diagnose the issue without changing the stable kind.

Rules:

- keep `kind` stable and small
- put diagnostic detail in `message`
- preserve enough detail to distinguish likely transport classes inside `network_error`
- do not leak secrets, tokens, cookies, or auth headers in messages

Examples of acceptable `network_error` message detail:

- DNS resolution failure
- TCP connect refused
- TLS validation failure
- timeout

## Spec-Change Rules

The following are spec changes:

- adding a new `ApiError::kind()` value
- renaming an existing `kind`
- changing MCP or HTTP envelope structure in a breaking way
- changing the expected status-code mapping for an existing kind

When making one of those changes, update:

- `docs/ERRORS.md`
- `docs/MCP.md`
- `docs/CONVENTIONS.md`
- `CLAUDE.md`
- any affected surface code and tests

## Verification Requirements

At minimum, verify:

1. SDK errors preserve the expected `kind()`
2. MCP emits the expected structured error envelope
3. HTTP emits the expected structured JSON error with the matching semantic kind
4. messages do not leak secrets

## Batch-result envelope

Actions that operate on multiple items in one call (e.g. `acp.session.bulk_close`) return a partial-success envelope with two arrays. Inner `failed[]` items reuse the same `{ kind, message }` shape as top-level `ToolError::Sdk` so per-item taxonomy stays consistent with the rest of the system.

```json
{
  "closed": ["session-uuid-1", "session-uuid-2"],
  "failed": [
    { "id": "session-uuid-3", "kind": "internal_error", "message": "..." }
  ]
}
```

Rules:

- `closed[]` contains the ids that completed the action.
- `failed[]` contains ids that the action attempted but errored on; per-item `kind` must be one of the canonical kinds listed above.
- Items the caller is not authorized to act on are silently omitted from BOTH arrays (preserves the `not_found` masking pattern — do not leak existence by reporting forbidden items).
- Authorization or validation errors that prevent the action from running at all return a top-level `ToolError` (not a 200 with empty arrays).

## Related Docs

- [CONVENTIONS.md](../CONVENTIONS.md)
- [MCP.md](../surfaces/MCP.md)
- [CLI.md](../surfaces/CLI.md)
- [OBSERVABILITY.md](./OBSERVABILITY.md)
