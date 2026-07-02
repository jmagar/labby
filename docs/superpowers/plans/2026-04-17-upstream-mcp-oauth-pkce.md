# Upstream MCP OAuth PKCE Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Revision:** 2026-04-17 — integrates research findings from `2026-04-17-upstream-mcp-oauth-pkce.research.md`. See `## Decisions (Locked)` for the design choices that close out the open questions surfaced by review.

**Goal:** Add spec-aligned outbound OAuth `authorization_code` + PKCE support for OAuth-protected upstream MCP servers connected through the gateway, with persisted credentials and user-scoped upstream access.

**Architecture:** Keep `lab` as the only `rmcp` crate and treat upstream OAuth as a client-side concern distinct from the existing inbound `lab-auth` server. Reuse the existing SQLite/auth infrastructure patterns, but store outbound upstream OAuth credentials separately and key them by `(upstream_name, lab_user_sub)`. Preserve the current global gateway pool for unauthenticated/static-bearer upstreams; **add a per-`(upstream, subject)` `AuthClient` cache** that the existing pool's `connect_http_upstream` consults when an upstream is OAuth-enabled. Outbound OAuth must target each upstream MCP server's advertised authorization server via Protected Resource Metadata and Authorization Server Metadata; it must not reuse the Google-specific inbound provider logic used for logging users into `lab`.

**Tech Stack:** Rust 2024, `rmcp` 1.4 auth/client APIs, `axum`, `tokio`, `reqwest`, `rusqlite`, `chacha20poly1305` (new), existing `lab-auth` SQLite store, streamable HTTP MCP.

---

## Top Must-Fix Items (security/correctness blockers)

These items were absent from the original plan and **must** be implemented exactly as specified below. Items 1–5 came from the research pass; 6–11 came from the engineering review.

1. **PKCE S256 only.** Refuse the flow if AS metadata's `code_challenge_methods_supported` is absent OR does not include `S256`. **Never fall back** to whatever the AS advertises (RFC 7636 default = `plain`). After negotiation, also assert the selected method is `S256`. (Task 2 §6.)
2. **Callback session binding.** Callback fails fast when no authenticated session is present. Subject is derived from the session/bearer, **not** from the `state` parameter. The derived subject must equal the subject stored in the pending state row. (Task 3 §5.)
3. **Callback `upstream_name` binding.** The `upstream_name` from the callback URL must also be validated against the pending state row's `upstream_name` (the SQL PK contains it; enforce in code too). Without this, an authenticated victim browser can be redirected to `/auth/upstream/callback?upstream=<chosen-by-attacker>&state=<victim-state>&code=<attacker-code>` and the attacker's code gets bound to the victim's subject. (Task 3 §5.)
4. **RFC 8707 `resource` parameter.** Send the canonical upstream MCP URL on both the authorization request and the token request. Canonicalization: RFC 3986 §6.2.2 (lowercase scheme/host, normalize percent-encoding). Pick one explicit trailing-slash policy and apply identically at both `start_authorization` and `token` request sites. MCP 2025-06-18 spec MUST. (Task 2 §6.)
5. **Issuer binding.** After AS metadata discovery, normalize both `metadata.issuer` and the discovered AS URL using the same RFC 3986 §6.2.2 algorithm; require **exact byte-string equality**. Refuse with `oauth_issuer_mismatch` otherwise. (Task 2 §6.)
6. **Single-flight refresh per `(upstream, subject)`.** Concurrent refresh from two requests rotates the refresh token twice and the AS revokes the entire grant. Serialize via `tokio::sync::Mutex` keyed by `(upstream, subject)`. After acquiring, **re-read** credentials before deciding to refresh. **Restrict the post-refresh retry to idempotent HTTP methods** (`GET`, `HEAD`, `OPTIONS`) — POST/MCP `tool_call` may have already mutated upstream state before returning 401, and a retry would double-execute. On non-idempotent methods, surface the original 401 to the caller without retry. (Task 2 §5.)
7. **Proactive refresh.** Before sending a request, if `access_token_expires_at - now < 30s`, refresh first (still under the per-key lock). Avoids 401-storm cascade and removes the retry-safety problem above for the common case. (Task 2 §5.)
8. **Take-once via `DELETE … RETURNING`.** PKCE state must be deleted and read in a single SQL statement to prevent replay across the 4-connection pool. (Task 1 §4.)
9. **chacha20poly1305 nonce: always fresh.** Every call to `seal()` MUST generate a new 12-byte random nonce; the refresh upsert path MUST NOT preserve the prior `token_blob_nonce`. Nonce reuse with the same key under any two distinct plaintexts is catastrophic for chacha20poly1305 (XOR recovers both). The encryption helper API generates nonces internally; the upsert must store the nonce returned from `seal()`, not the previous one. (Task 2 §2 + §5/6.)
10. **AuthClient cache atomicity.** Use `DashMap::entry()` (or `OnceCell`-per-key) so two concurrent first-requests for the same `(upstream, subject)` do not both run discovery, decrypt, or DCR. The build closure runs while no readers are blocked. (Task 4 §3.)
11. **Decryption failure → `oauth_needs_reauth`.** If `encryption::open()` fails (likely operator rotated `LAB_OAUTH_ENCRYPTION_KEY`), the credential store returns `oauth_needs_reauth` to the caller, NOT `internal_error`. Avoids leaking credential-existence as an oracle and gives users a self-service recovery path. (Task 2 §3, store.rs.)

---

## Decisions (Locked)

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | **State storage:** SQLite table (with `DELETE … RETURNING` take-once, 10-min hard TTL, integrated into existing `cleanup_expired`). | Survives restart; matches existing inbound auth-server patterns; cleanup is consolidated. |
| D2 | **Per-user cache shape:** `DashMap<(String, String), Arc<AuthClient<reqwest::Client>>>` keyed by `(upstream_name, subject)` — **not** per-subject `UpstreamPool`. Use `entry().or_try_insert_with` (or `OnceCell`-per-key) so the build closure runs without blocking readers. | Reuses the single global `UpstreamPool`; only OAuth credentials are per-user. `DashMap::entry` makes cold-miss atomic — two concurrent first-requests do not both run discovery/decrypt/DCR. |
| D3 | **Token-at-rest encryption:** `chacha20poly1305` AEAD applied to `token_response_json`. Key from `~/.labby/.env` as `LAB_OAUTH_ENCRYPTION_KEY` (base64-encoded 32 bytes). Loaded once at `AppState` construction; startup fails fast if missing or wrong length. **Every `seal()` generates a fresh random 12-byte nonce**; upsert stores the new nonce. Decryption failures surface as `oauth_needs_reauth`. | Refresh tokens are long-lived bearer secrets; 0600 perms insufficient against backup/snapshot leaks. Fresh nonce per call is non-negotiable for chacha20poly1305. |
| D4 | **Registration strategies:** Keep all three (`client_metadata_document`, `dynamic`, `preregistered`) as first-class. Each has explicit test coverage in Task 2 §7. | MCP spec SHOULD support DCR; operator flexibility against varied upstreams. |
| D5 | **rmcp PoC:** Gating spike (Task 0) confirms `AuthClient<reqwest::Client>` ↔ `StreamableHttpClientWorker::new` integration **before** Task 2 §4. Plan B is **defined**: a custom `tower::Service` middleware over `reqwest::Client` that reads the bearer from a `tokio::task_local` per request. The spike runs as `examples/spike_rmcp_auth_client.rs` against a wiremock OAuth upstream **and** is invokable against a real OAuth upstream by an operator. | rmcp does not auto-refresh on 401 and `AuthClient` ↔ worker wiring is not documented; named fallback prevents the "stop and revise" deadlock. |
| D6 | **Manual token refresh + retry safety.** rmcp 1.4 does not auto-refresh; `UpstreamOauthManager` interposes a refresh middleware. **Proactive refresh** if `expires_at - now < 30s`. Reactive: on 401, single-flight per `(upstream, subject)`, persist new credentials, then **retry once only for idempotent methods** (`GET`/`HEAD`/`OPTIONS`). Non-idempotent methods (POST `tool_call`) surface the original 401 with `oauth_needs_reauth`. On `invalid_grant`, delete credentials and surface `oauth_needs_reauth`. | Avoids RT-rotation race (spec MUST: rotation), avoids double-executing destructive tool calls. |
| D7 | **Authorization-URL response shape:** JSON only (`{ "authorization_url": "..." }`). No browser-redirect mode in MVP. | Removes content negotiation; one path to test. |
| D8 | **HTTP route placement + ownership:** `crates/lab/src/api/upstream_oauth.rs` (flat in `api/`, alongside `api/oauth.rs`). All HTTP handlers funnel through `dispatch/gateway/oauth.rs`, which calls `UpstreamOauthManager`. The `oauth_clients` `DashMap` lives on `GatewayManager`, but **all OAuth methods on `GatewayManager` are thin pass-throughs** to `UpstreamOauthManager`; tests target the manager. | Single ownership of OAuth state; HTTP routes never bypass the dispatch shim. |
| D9 | **Catalog stdio filter:** `actions_for(...)` filters OAuth-tagged upstreams when transport is stdio. | Stdio has no stable subject identity. |
| D10 | **Reload eviction with `client_id` guard:** On `gateway.reload()`, snapshot cache keys under read lock, then write-lock per-key for removal of upstreams missing from the new config. **Each `oauth_clients` cache entry stores the `client_id` it was built with**; cache lookup compares against current config and refuses to reuse on mismatch (returns `oauth_needs_reauth`), preventing silent re-bind to a redefined upstream. | Closes the config-rebind silent re-authentication gap; bounds eviction lock scope. |

---

### Scope Guard

This plan intentionally targets OAuth-enabled upstreams on authenticated HTTP surfaces first.

- `/mcp` over HTTP: supported in this plan
- hosted web UI / browser flow: supported in this plan
- stdio transport: not supported for user-scoped upstream OAuth in this plan

Reason: current stdio sessions do not carry a stable authenticated user identity, while HTTP routes already do.

### Auth Boundary

- Inbound auth to `lab` remains the existing `lab-auth` responsibility.
- `lab-auth` may keep using Google as an upstream identity provider for logging users into `lab`.
- Outbound auth from `lab` to upstream MCP servers is a separate concern and must use the upstream MCP server's advertised OAuth surface.
- Do not reuse `crates/lab-auth/src/google.rs` for upstream MCP OAuth.
- Reuse only the generic pieces from the existing auth implementation: SQLite file management, callback-routing patterns, request/session identity plumbing, and secret-handling conventions.

### Cross-cutting Conventions

- **No `mod.rs` files.** `oauth/upstream.rs` declares `oauth/upstream/{manager,store,types,refresh,encryption}.rs`.
- **ToolError serialization** is hand-written; never `#[derive(Serialize)]` on new error variants.
- **Error kinds** must be added to `docs/ERRORS.md` first. New kinds introduced by this plan: `oauth_needs_reauth`, `oauth_state_invalid`, `oauth_resource_mismatch`, `oauth_issuer_mismatch`, `oauth_unsupported_method`.
- **Async traits** use native `async fn in trait`. Do not add `async-trait`.
- **Standard dispatch fields** on every event: `surface`, `service` (`upstream_oauth`), `action`, `elapsed_ms`, `kind` on failure. HTTP routes additionally include `request_id`.
- **Redaction.** Never log: `code`, `state`, raw `token_response_json`, deserialized access/refresh tokens, `Authorization` headers, `client_secret`. Cross-reference `docs/OBSERVABILITY.md`.

---

## File Structure

- Modify: `crates/lab/src/config.rs`
  Purpose: add upstream OAuth config types (serde-tagged enums) + mutual-exclusion validation against `bearer_token_env`.
- Modify: `crates/lab-auth/src/types.rs`
  Purpose: define persisted outbound OAuth row types (encrypted token blob field).
- Modify: `crates/lab-auth/src/sqlite.rs`
  Purpose: add tables (with PKs, `NOT NULL`, indices) and CRUD helpers for outbound OAuth credentials and PKCE state. Extend `cleanup_expired` to cover both new tables.
- Create: `crates/lab/src/oauth/upstream.rs`
  Purpose: module entrypoint for outbound upstream OAuth.
- Create: `crates/lab/src/oauth/upstream/types.rs`
  Purpose: resolved outbound OAuth config and `(upstream, subject)` cache keys.
- Create: `crates/lab/src/oauth/upstream/encryption.rs`
  Purpose: `chacha20poly1305` AEAD wrapper for `token_response_json` + key loading from `LAB_OAUTH_ENCRYPTION_KEY`.
- Create: `crates/lab/src/oauth/upstream/store.rs`
  Purpose: `rmcp::transport::auth::CredentialStore` and `StateStore` adapters backed by `lab-auth::sqlite::SqliteStore`. Encrypts/decrypts at the boundary.
- Create: `crates/lab/src/oauth/upstream/refresh.rs`
  Purpose: per-`(upstream, subject)` single-flight `Mutex` map and 401 → refresh → persist → retry middleware.
- Create: `crates/lab/src/oauth/upstream/manager.rs`
  Purpose: start authorization, finish callback, load credentials, build `AuthorizationManager`, and create `AuthClient<reqwest::Client>` for one `(upstream, subject)`.
- Create: `crates/lab/src/api/upstream_oauth.rs`
  Purpose: HTTP routes for begin/callback/status/clear flows for upstream OAuth (D8).
- Modify: `crates/lab/src/api/state.rs`
  Purpose: mount outbound upstream OAuth manager into shared app state.
- Modify: `crates/lab/src/api/router.rs`
  Purpose: mount master-only upstream OAuth routes and pass request auth/session context through.
- Modify: `crates/lab/src/api/oauth.rs`
  Purpose: add helper(s) for extracting authenticated subject data from request context where useful.
- Modify: `crates/lab/src/dispatch/upstream/pool.rs`
  Purpose: add OAuth-aware HTTP upstream connection branch — looks up `(upstream, subject)` `AuthClient` from the manager and passes it through `StreamableHttpClientWorker`. Single-flight refresh middleware lives here or in `oauth/upstream/refresh.rs`.
- Modify: `crates/lab/src/dispatch/gateway/manager.rs`
  Purpose: hold the global pool plus a reference to `UpstreamOauthManager`. On `reload()`, evict cached `AuthClient` entries for upstreams removed from config.
- Create: `crates/lab/src/dispatch/gateway/oauth.rs`
  Purpose: thin dispatch shim so HTTP and (future) CLI surfaces both call the manager via dispatch (preserves layer contract).
- Modify: `crates/lab/src/mcp/server.rs`
  Purpose: resolve authenticated subject from RMCP request extensions (D2's gating verification step in Task 4) and route upstream tool/resource/prompt operations through the OAuth-aware connect path. Catalog filtering for stdio (D9).
- Modify: `crates/lab/src/catalog.rs`
  Purpose: `actions_for()` accepts a `transport: Transport` hint and filters OAuth-tagged upstreams on stdio.
- Modify: `docs/CONFIG.md`
  Purpose: document upstream OAuth config (tagged-enum shape) and operator setup including `LAB_OAUTH_ENCRYPTION_KEY`.
- Modify: `docs/UPSTREAM.md`
  Purpose: document outbound OAuth discovery, PKCE flow (S256-only, RFC 8707 `resource`, issuer binding), per-`(upstream, subject)` caching, and `oauth_needs_reauth` semantics.
- Modify: `docs/GATEWAY.md`
  Purpose: document operator flow, gateway actions, runtime semantics, graceful-drain on `clear_credentials`, reload eviction.
- Modify: `docs/ERRORS.md`
  Purpose: register new stable kinds (`oauth_needs_reauth`, `oauth_state_invalid`, `oauth_resource_mismatch`, `oauth_issuer_mismatch`, `oauth_unsupported_method`).
- Modify: `docs/OBSERVABILITY.md`
  Purpose: extend redaction list (`code`, OAuth `state`, token fields).

---

### Task 0: rmcp Integration PoC (Gating Spike)

**Files:**
- Create: `crates/lab/examples/spike_rmcp_auth_client.rs` (runnable example, not a unit test)

- [ ] **Step 1: Write a minimal spike**

Confirm four integration points before any production code:

1. `AuthClient<reqwest::Client>` can be constructed and passed to `StreamableHttpClientWorker::new(...)`.
2. The worker injects `Authorization: Bearer <token>` on outbound requests automatically — OR identify the wrapper API that does so.
3. On a synthesized 401 from a wiremock upstream, confirm whether refresh fires automatically (expected: **not**, per rmcp 1.4 docs).
4. The spike must run against (a) a wiremock OAuth upstream by default, **and** (b) a real OAuth upstream when `SPIKE_REAL_AS_URL` is set, so the operator can validate end-to-end interactively before Task 2 starts.

- [ ] **Step 2: Document findings inline**

Write the result into `crates/lab/src/oauth/upstream/refresh.rs` (created in Task 2) as a header comment: rmcp version verified, whether `auth_manager.refresh_token()` is manual, and which integration path was confirmed (auto-injection or fallback).

- [ ] **Step 3: Plan B if integration fails**

If rmcp does **not** propagate `Authorization` headers from `AuthClient`, fall back to:

> **Custom `tower::Service` middleware over `reqwest::Client`** that reads the bearer from a `tokio::task_local::<Arc<AuthClient>>` set by the manager before each request. This keeps the rmcp transport unchanged and isolates the auth concern.

Do not consider Plan B as "stop and revise" — implement it inline in `oauth/upstream/refresh.rs` as the auth wrapper around `reqwest::Client`. Time-box the spike + Plan B to **2 working days total**.

- [ ] **Step 4: Commit**

```bash
git add crates/lab/examples/spike_rmcp_auth_client.rs
git commit -m "spike: validate rmcp AuthClient integration with StreamableHttpClientWorker"
```

---

### Task 1: Add Config And Persistence Primitives

**Files:**
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab-auth/src/types.rs`
- Modify: `crates/lab-auth/src/sqlite.rs`
- Test: `crates/lab/src/config.rs`
- Test: `crates/lab-auth/src/sqlite.rs`

- [ ] **Step 1: Add outbound OAuth config types as tagged enums**

Use serde-tagged enums (no stringly-typed discriminants). Add a flat `oauth` field on `UpstreamConfig` and validate mutual exclusion with `bearer_token_env`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamOauthConfig {
    pub mode: UpstreamOauthMode, // tagged enum
    pub registration: UpstreamOauthRegistration, // tagged enum
    pub scopes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamOauthMode {
    AuthorizationCodePkce,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum UpstreamOauthRegistration {
    ClientMetadataDocument { url: String },
    Preregistered {
        client_id: String,
        #[serde(default)]
        client_secret_env: Option<String>,
    },
    Dynamic, // DCR — initial_access_token (if any) supplied via env at runtime
}

impl UpstreamConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.bearer_token_env.is_some() && self.oauth.is_some() {
            return Err(ConfigError::ConflictingAuth { name: self.name.clone() });
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Add failing config tests**

Cover supported and unsupported combinations, including mutual-exclusion failures:

Run: `cargo test -p lab config::tests::upstream_oauth -- --nocapture`
Expected: FAIL because the new config shape and validation do not exist yet.

- [ ] **Step 3: Add outbound OAuth SQLite row types**

Token blob is encrypted; expiry is denormalized for cheap pruning. **`Debug` impls must be redacted** (no ciphertext, nonce, or verifier in formatted output) — implement manually, do not derive.

```rust
pub struct UpstreamOauthCredentialRow {
    pub upstream_name: String,
    pub subject: String,
    pub client_id: String,
    pub granted_scopes_json: String,
    pub token_blob: Vec<u8>,                // chacha20poly1305(token_response_json) — D3
    pub token_blob_nonce: Vec<u8>,          // 12 bytes, FRESH per seal() call
    pub token_received_at: i64,
    pub access_token_expires_at: i64,       // denormalized for cleanup_expired pruning
    pub refresh_token_present: bool,        // SEC-9: enables cleanup of access-only stale rows
}

// Manual redacted Debug — never derive.
impl std::fmt::Debug for UpstreamOauthCredentialRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamOauthCredentialRow")
            .field("upstream_name", &self.upstream_name)
            .field("subject", &"<redacted>")
            .field("client_id", &self.client_id)
            .field("token_blob", &"<redacted>")
            .field("token_blob_nonce", &"<redacted>")
            .field("access_token_expires_at", &self.access_token_expires_at)
            .field("refresh_token_present", &self.refresh_token_present)
            .finish()
    }
}

pub struct UpstreamOauthStateRow {
    pub upstream_name: String,
    pub subject: String,
    pub csrf_token: String,
    pub pkce_verifier: String,                // sensitive — manual redacted Debug
    pub created_at: i64,
    pub expires_at: i64,                      // hard-capped to created_at + 600s
}

// Manual redacted Debug — never derive (pkce_verifier is sensitive).
impl std::fmt::Debug for UpstreamOauthStateRow { /* redact subject, csrf_token, pkce_verifier */ }
```

The `refresh_token_present` bool unlocks a follow-up `cleanup_expired` pass: rows where `access_token_expires_at < now AND NOT refresh_token_present` are dead and can be pruned. Implement the pruning in this MVP (was deferred in the prior revision).

- [ ] **Step 4: Add SQLite tables and CRUD helpers**

Schema requirements (mirror `lab-auth` style):

```sql
CREATE TABLE IF NOT EXISTS upstream_oauth_credentials (
    upstream_name             TEXT NOT NULL,
    subject                   TEXT NOT NULL,
    client_id                 TEXT NOT NULL,
    granted_scopes_json       TEXT NOT NULL,
    token_blob                BLOB NOT NULL,
    token_blob_nonce          BLOB NOT NULL,
    token_received_at         INTEGER NOT NULL,
    access_token_expires_at   INTEGER NOT NULL,
    refresh_token_present     INTEGER NOT NULL,    -- 0 or 1; SEC-9
    PRIMARY KEY (upstream_name, subject)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS upstream_oauth_state (
    upstream_name   TEXT NOT NULL,
    subject         TEXT NOT NULL,
    csrf_token      TEXT NOT NULL,
    pkce_verifier   TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,
    PRIMARY KEY (upstream_name, subject, csrf_token)
) WITHOUT ROWID;
```

CRUD helpers:

```rust
pub async fn upsert_upstream_oauth_credentials(&self, row: UpstreamOauthCredentialRow) -> Result<(), AuthError>;
pub async fn find_upstream_oauth_credentials(&self, upstream_name: &str, subject: &str) -> Result<Option<UpstreamOauthCredentialRow>, AuthError>;
pub async fn delete_upstream_oauth_credentials(&self, upstream_name: &str, subject: &str) -> Result<(), AuthError>;

pub async fn save_upstream_oauth_state(&self, row: UpstreamOauthStateRow) -> Result<(), AuthError>;
/// Atomic take-once — single DELETE … RETURNING statement (mirrors take_authorization_request).
pub async fn take_upstream_oauth_state(
    &self,
    upstream_name: &str,
    subject: &str,
    csrf_token: &str,
    now: i64,
) -> Result<Option<UpstreamOauthStateRow>, AuthError>;
```

The take-once query must be exactly:

```sql
DELETE FROM upstream_oauth_state
 WHERE upstream_name = ?1
   AND subject = ?2
   AND csrf_token = ?3
   AND expires_at > ?4
 RETURNING upstream_name, subject, csrf_token, pkce_verifier, created_at, expires_at
```

`save_upstream_oauth_state` must enforce `expires_at - created_at <= 600` (10 minutes). Reject otherwise.

Extend `cleanup_expired()` (sqlite.rs:398) to delete from both tables:

```sql
DELETE FROM upstream_oauth_state       WHERE expires_at < ?1;
DELETE FROM upstream_oauth_credentials WHERE access_token_expires_at < ?1 AND refresh_token_present = 0;
```

The second statement is the SEC-9 fix — stale access-only rows (no refresh) are dead weight and prunable.

**Cadence:** `cleanup_expired` runs as a single 60-second background `tokio::spawn` task started from `AppState`. It is **never** called per-request. The task is cancelled on graceful shutdown.

- [ ] **Step 5: Run focused persistence tests**

Required test cases:

1. `sqlite_store_upsert_upstream_oauth_credentials_round_trip`
2. `sqlite_store_takes_upstream_oauth_state_only_once_under_race` — `tokio::join!` two concurrent takes; assert exactly one wins (mirrors `sqlite_store_redeems_auth_code_only_once_under_race`).
3. `sqlite_store_rejects_state_ttl_over_600s`
4. `sqlite_store_cleanup_expired_drops_state` — insert expired row, run cleanup, assert gone.
5. `sqlite_store_credentials_isolated_per_subject` — write creds for two subjects, delete one, assert the other survives.
6. `sqlite_store_upsert_overwrites_existing_credentials` — insert twice for same `(upstream, subject)`, assert single row.

Run: `cargo test -p lab-auth sqlite_store_upstream_oauth -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/config.rs crates/lab-auth/src/types.rs crates/lab-auth/src/sqlite.rs
git commit -m "feat: add upstream oauth config and persistence primitives"
```

---

### Task 2: Build RMCP OAuth Store Adapters And Client Manager

**Files:**
- Create: `crates/lab/src/oauth/upstream.rs`
- Create: `crates/lab/src/oauth/upstream/types.rs`
- Create: `crates/lab/src/oauth/upstream/encryption.rs`
- Create: `crates/lab/src/oauth/upstream/store.rs`
- Create: `crates/lab/src/oauth/upstream/refresh.rs`
- Create: `crates/lab/src/oauth/upstream/manager.rs`
- Modify: `crates/lab/Cargo.toml` (add `chacha20poly1305`)
- Test: `crates/lab/src/oauth/upstream/manager.rs`

- [ ] **Step 1: Create outbound OAuth module skeleton**

```rust
// oauth/upstream.rs
pub mod encryption;
pub mod manager;
pub mod refresh;
pub mod store;
pub mod types;
```

- [ ] **Step 2: Implement `chacha20poly1305` encryption helper**

Load 32-byte key from `LAB_OAUTH_ENCRYPTION_KEY` (base64) **at `AppState` construction** (startup, fail-fast). Validate base64 decodes to exactly 32 bytes; otherwise refuse to start with a clear operator error. The key is never re-decoded per request.

```rust
/// Seals plaintext under the key, generating a fresh random 12-byte nonce internally.
/// Callers MUST store the returned nonce alongside the ciphertext and MUST NOT reuse it.
pub fn seal(key: &Key, plaintext: &[u8]) -> (Vec<u8> /* ciphertext */, Vec<u8> /* nonce */);

/// Opens ciphertext using the stored nonce. On failure, callers MUST surface
/// `oauth_needs_reauth` (NOT `internal_error`) — this avoids creating a credential-existence
/// oracle and gives the user a self-service recovery path after key rotation.
pub fn open(key: &Key, ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<u8>, EncryptionError>;
```

Tests:
- Round-trip plaintext.
- Wrong key fails decryption (no plaintext leak).
- Wrong nonce fails decryption.
- **Two `seal()` calls for the same plaintext produce different nonces** (assert no nonce reuse from a fresh helper).
- Base64 key of wrong length rejected at load.

- [ ] **Step 3: Implement `CredentialStore` and `StateStore` adapters**

Encrypt at the boundary — the `SqliteStore` only sees ciphertext.

```rust
pub struct SqliteCredentialStore {
    pub store: lab_auth::sqlite::SqliteStore,
    pub upstream_name: String,
    pub subject: String,
    pub key: Arc<chacha20poly1305::Key>,
}

pub struct SqliteStateStore {
    pub store: lab_auth::sqlite::SqliteStore,
    pub upstream_name: String,
    pub subject: String,
}
```

- [ ] **Step 4: Write failing tests for store adapter round-trips**

Verify:
- credentials round-trip through encrypt/decrypt
- swapping the key fails decryption (no plaintext leak)
- state save+take returns the row exactly once

Run: `cargo test -p lab upstream_oauth_store -- --nocapture`
Expected: FAIL (adapters do not exist yet).

- [ ] **Step 5: Implement proactive + reactive refresh middleware (single-flight, retry-safe)**

```rust
// refresh.rs — DashMap to avoid one outer mutex serializing all OAuth traffic.
pub struct RefreshLocks {
    // Outer DashMap is lock-free for reads. Inner Arc<Mutex<()>> serializes per (upstream, subject).
    inner: dashmap::DashMap<(String, String), Arc<tokio::sync::Mutex<()>>>,
    // Optional: bound entries to prevent unbounded growth on long-running processes.
    cap: usize, // soft cap, e.g., 10_000; oldest unused entries evicted when exceeded.
}

impl RefreshLocks {
    pub async fn acquire(&self, upstream: &str, subject: &str) -> tokio::sync::OwnedMutexGuard<()>;
}
```

The middleware (wrapping the `reqwest::Client` used by `AuthClient` — Plan A; or via task-local — Plan B from Task 0):

**Proactive path (every request):**
1. Read cached `(access_token, expires_at)`.
2. If `expires_at - now < 30s`, take the per-`(upstream, subject)` lock, re-read, refresh if still close, persist, release.
3. Send request with the (possibly refreshed) token.

**Reactive path (on 401):**
4. Take the per-`(upstream, subject)` lock.
5. Re-read credentials from the store (another task may have already refreshed).
6. If still expired, call `auth_manager.refresh_token()`.
7. On success: persist new credentials via `CredentialStore::save` **before releasing the lock** (the rotated refresh token MUST be durable before any other request retries).
8. On `invalid_grant`: delete credentials and return `oauth_needs_reauth`. **Do not retry.**
9. **Retry the original request only if its HTTP method is idempotent** (`GET`, `HEAD`, `OPTIONS`). For non-idempotent methods (`POST`, `PUT`, `DELETE`, `PATCH`), return the original 401 wrapped as `oauth_needs_reauth` to the caller — do not retry. MCP tool calls travel as POST; a retry could double-execute destructive tools.

LRU/cap on `RefreshLocks`: maintain at most `cap` entries; on overflow, drop entries whose `Arc` has only one strong ref (no in-flight refresh). Default cap 10_000; configurable.

- [ ] **Step 6: Implement `UpstreamOauthManager`**

```rust
pub async fn begin_authorization(
    &self,
    upstream: &UpstreamConfig,
    subject: &str,
    email: Option<&str>,
) -> Result<BeginAuthorization, OauthError>;

pub async fn complete_authorization_callback(
    &self,
    upstream_name: &str,
    subject: &str,
    code: &str,
    state: &str,
) -> Result<(), OauthError>;

pub async fn clear_credentials(
    &self,
    upstream_name: &str,
    subject: &str,
) -> Result<(), OauthError>;

pub async fn build_auth_client(
    &self,
    upstream: &UpstreamConfig,
    subject: &str,
) -> Result<AuthClient<reqwest::Client>, OauthError>;

pub async fn has_credentials(
    &self,
    upstream_name: &str,
    subject: &str,
) -> Result<bool, OauthError>;
```

Implementation requirements:

- Use `OAuthState::new(server_url, Some(reqwest_client))`.
- Use `start_authorization_with_metadata_url(scopes, redirect_uri, Some("lab"), client_metadata_url)`. Pass empty scopes only when config does not pin them.
- **Enforce S256 only — no fallback.**
  - After AS metadata discovery, refuse with `oauth_unsupported_method` if `code_challenge_methods_supported` is **absent** OR does not contain `"S256"`. (RFC 7636 absence = `plain` only; never accept that.)
  - After `AuthorizationManager` selects a method, also assert the selected method is `S256`; refuse otherwise.
- **Send `resource` parameter (RFC 8707)** on both authorization and token requests.
  - Compute the canonical upstream MCP URL **once per upstream** at config-load time (cached on `UpstreamConfig`) using RFC 3986 §6.2.2 normalization: lowercase scheme + host, percent-encoding normalization, default-port elision.
  - **Trailing-slash policy: preserve as configured** (do not strip). Both the authorization and token requests send the byte-identical canonical form. If the upstream rejects on `audience`/`resource` mismatch, surface `oauth_resource_mismatch`.
- **Verify issuer binding (RFC 8414 §3.3)** after AS metadata discovery, **once per upstream** (cached on `AuthorizationManager`):
  - Apply the same RFC 3986 §6.2.2 normalization to both `metadata.issuer` and the discovered AS URL.
  - Compare with **exact byte-string equality** after normalization; refuse with `oauth_issuer_mismatch` on any difference.
- Attach the SQLite-backed `CredentialStore` and `StateStore`.
- Require `authorization_code + PKCE` only.
- Discover and use the upstream MCP authorization server via RMCP auth discovery; never call Google OAuth endpoints directly.
- **Redirect URI** is built from a configured base URL (`LAB_PUBLIC_URL` or `lab.public_url`), NOT from the request `Host` header. Plus the constant suffix `/auth/upstream/callback`. **Refuse to start if `LAB_PUBLIC_URL` is unset or not absolute https://** (test-asserted).
- All errors funnel through stable kinds; never log `code`, `state`, or token fields.

- [ ] **Step 7: Add tests using a mocked OAuth-protected MCP auth server**

Required scenarios (each as a discrete test):

**Discovery + binding:**
- Metadata discovery happy path (PRM → AS metadata).
- Issuer binding rejection: AS metadata lies → `oauth_issuer_mismatch`.
- Issuer binding trailing-slash variations: `https://as` vs `https://as/` (after normalization, equal); `https://as/foo` vs `https://as/bar` (refused).
- AS metadata missing `code_challenge_methods_supported` → refused with `oauth_unsupported_method`.
- AS metadata advertises `["plain"]` only → refused.
- AS metadata advertises `["S256", "plain"]` → accepted with S256 selected.

**RFC 8707 resource parameter:**
- Present on authorization request.
- Present on token request.
- Resource value byte-identical between the two requests.
- Mismatched `aud` claim from upstream → `oauth_resource_mismatch`.

**Registration paths (each as separate test):**
- CIMD path: `client_metadata_url` resolved, client registered.
- Preregistered path with public client (`client_id` only).
- Preregistered path with confidential client (`client_id` + `client_secret_env`).
- Dynamic registration (DCR) path: AS issues credentials, persisted, reused on next start.

**Encryption round-trip:**
- Stored credentials reload through a fresh manager.
- Refresh upsert produces a **fresh nonce** (assert `nonce_after != nonce_before`).
- Decryption failure (after rotating `LAB_OAUTH_ENCRYPTION_KEY`) surfaces `oauth_needs_reauth`, not `internal_error`.

**Refresh + retry safety:**
- Proactive: token < 30 s from expiry triggers refresh before request is sent.
- Reactive: 401 triggers refresh + retry.
- Concurrent refresh single-flight: `tokio::join!` two requests on the same `(upstream, subject)`; exactly one `refresh_token` call hits the AS.
- Idempotent retry only: GET with 401 retried; POST with 401 returns `oauth_needs_reauth` without retry.
- `invalid_grant` deletes credentials, returns `oauth_needs_reauth`; second call after wipe returns same kind without re-attempting refresh.

Run: `cargo test -p lab upstream_oauth_manager -- --nocapture`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/oauth/upstream.rs crates/lab/src/oauth/upstream/* crates/lab/Cargo.toml
git commit -m "feat: add rmcp-backed upstream oauth manager with single-flight refresh and at-rest encryption"
```

---

### Task 3: Add Browser Start/Callback Routes For Upstream OAuth

**Files:**
- Create: `crates/lab/src/api/upstream_oauth.rs`
- Create: `crates/lab/src/dispatch/gateway/oauth.rs`
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/api/oauth.rs`
- Test: `crates/lab/src/api/upstream_oauth.rs`

- [ ] **Step 1: Add outbound OAuth manager to `AppState`**

```rust
pub upstream_oauth_manager: Option<Arc<crate::oauth::upstream::manager::UpstreamOauthManager>>;
```

with a `with_upstream_oauth_manager(...)` builder.

- [ ] **Step 2: Add a thin dispatch shim**

`dispatch/gateway/oauth.rs` exposes `begin_authorization`, `complete_authorization_callback`, `status`, `clear` — calls into `UpstreamOauthManager`. The HTTP handlers in `api/upstream_oauth.rs` call this shim, not the manager directly. This preserves the layer contract and makes a future CLI command trivial.

- [ ] **Step 3: Write failing route tests for start/status/callback/clear**

Endpoints:

- `POST /v1/gateway/oauth/start` — body `{ "upstream": "<name>" }`. Returns `{ "authorization_url": "..." }` (D7 — JSON only).
- `GET /auth/upstream/callback?code=...&state=...&upstream=...` — completes the flow, redirects to a same-origin success/failure URL.
- `GET /v1/gateway/oauth/status?upstream=<name>` — returns `{ "authenticated": bool, "upstream": "<name>", "expires_within_5m": bool }`. **No `subject` field** (avoids cross-account enumeration). **No raw expiry timestamp** (avoids fingerprinting). No `client_secret`, no token material, no state leakage.
- `POST /v1/gateway/oauth/clear?confirm=true` — destructive. Without `confirm=true`, returns 400 `confirmation_required` (single convention — query param only, **no envelope**). With `confirm=true`, deletes credentials and emits dispatch event.

Run: `cargo test -p lab upstream_oauth_route -- --nocapture`
Expected: FAIL.

- [ ] **Step 4: Implement start/status/clear handlers**

- Require authenticated `AuthContext` AND **master-only middleware** on every handler (same guard as other `/v1/gateway/*` routes). Test asserts non-master gets 403.
- Verify the named upstream exists and is OAuth-enabled.
- `start`: build `redirect_uri` from configured base URL; call dispatch shim.
- `status`: report only `{ authenticated, upstream, expires_within_5m }`.
- `clear`: enforce `?confirm=true` query param; on confirm, delete and emit dispatch event with `kind: "ok"`.

- [ ] **Step 5: Implement callback handler — security-critical**

The callback route:

1. Reject immediately with `oauth_state_invalid` if no authenticated session is present (cookie or bearer). **Do not** derive subject from the `state` parameter or the pending state row — derive only from the session.
2. Compute `subject = AuthContext.sub`.
3. Read `upstream_name` from the URL query param.
4. `take_upstream_oauth_state(upstream_name, subject, state, now)` — atomic single-statement DELETE … RETURNING. **Both** `upstream_name` and `subject` are part of the WHERE clause (PK already enforces this). If returns `None`, respond `oauth_state_invalid` (covers expired, replayed, cross-subject, and cross-upstream-name attempts — closes the SEC-2 token-injection vector).
5. Call `complete_authorization_callback(upstream_name, subject, code, state)`.
6. Build the success/failure redirect URL by concatenating a same-origin base (from configured `LAB_PUBLIC_URL`) with a fixed path `/gateway/oauth/result?upstream=<name>&status=<ok|fail>`. Never use any caller-supplied URL fragment for the redirect target.
7. The `/gateway/oauth/result` template MUST HTML-escape the `upstream` query parameter when rendering. Operator-controlled upstream names could otherwise inject markup. Use the existing template engine's auto-escape; assert in a unit test.
8. **Never log `code` or `state`.** Dispatch event records only `upstream`, `subject` hash, `kind`.

- [ ] **Step 6: Mount routes in the top-level router**

Mount in `api/router.rs`. **All four routes are master-only**, including `/auth/upstream/callback`. Verify the master-only middleware covers both prefixes:

- `/v1/gateway/oauth/{start,status,clear}` — gateway-scoped (action surface)
- `/auth/upstream/callback` — auth-scoped (mirrors `/auth/google/callback`)

Add explicit per-route tests: a non-master authenticated session hitting the callback gets 403, not the OAuth flow. The `/auth/google/callback` precedent **must not** be used to argue for weaker auth on `/auth/upstream/callback` — Google callback is for any-user login; this callback is for operator-only upstream provisioning.

Run: `cargo test -p lab upstream_oauth_route -- --nocapture`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/api/upstream_oauth.rs crates/lab/src/api/state.rs crates/lab/src/api/router.rs crates/lab/src/api/oauth.rs crates/lab/src/dispatch/gateway/oauth.rs
git commit -m "feat: add upstream oauth browser routes with session-bound callback"
```

---

### Task 4: Wire Per-`(upstream, subject)` `AuthClient` Cache Into Existing Pool

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/manager.rs`
- Modify: `crates/lab/src/dispatch/upstream/pool.rs`
- Modify: `crates/lab/src/mcp/server.rs`
- Modify: `crates/lab/src/catalog.rs`
- Test: `crates/lab/src/dispatch/gateway/manager.rs`
- Test: `crates/lab/src/dispatch/upstream/pool.rs`
- Test: `crates/lab/src/mcp/server.rs`

- [ ] **Step 1: Verify rmcp `RequestContext.extensions` propagation + startup self-test**

This is a **gating prerequisite** before Step 4 below.

**At test time:** add a focused test that shows `mcp/server.rs` handlers can read `http::request::Parts` from `RequestContext.extensions` for streamable-HTTP-server requests on the rmcp version pinned in `Cargo.toml`.

**At runtime:** add a startup self-test in `AppState` initialization: probe one of the two paths (extensions-propagation OR `tokio::task_local` fallback) is wired correctly. **Refuse to start if neither works** — emit a clear operator error pointing at the rmcp version and the Plan B fallback. This prevents per-request silent failures across rmcp upgrades.

If the spike-time test fails: rmcp does not propagate Parts. Fall back plan: install an axum middleware that stashes `AuthContext` in a `tokio::task_local`, and have the rmcp handler read the task-local. Implement the chosen path in production code; the startup self-test confirms it on every boot.

- [ ] **Step 2: Add failing tests describing the new runtime model**

- Global pool unchanged for static-bearer and unauthenticated upstreams.
- OAuth-enabled upstreams resolve through the per-`(upstream, subject)` `AuthClient` cache.
- Two authenticated subjects making concurrent requests to the same OAuth upstream get separate `AuthClient` instances (token isolation).
- `clear_credentials` for subject A does not affect subject B.

Run: `cargo test -p lab subject_scoped_upstream -- --nocapture`
Expected: FAIL.

- [ ] **Step 3: Add the `AuthClient` cache to `GatewayManager`**

Inside `GatewayManager`:

```rust
// DashMap, not RwLock<HashMap>. Atomic per-key build via entry().
oauth_clients: Arc<dashmap::DashMap<(String, String), Arc<CachedAuthClient>>>,

struct CachedAuthClient {
    client: Arc<AuthClient<reqwest::Client>>,
    /// client_id this entry was built with — guards against silent re-bind
    /// after operator changes UpstreamConfig (D10/ARCH-4).
    built_with_client_id: String,
}
```

Helpers (all are thin wrappers — actual logic lives in `UpstreamOauthManager`; `GatewayManager` is the cache owner only):

```rust
async fn get_or_build_auth_client(&self, upstream: &UpstreamConfig, subject: &str) -> Result<Arc<AuthClient<reqwest::Client>>, OauthError>;
async fn evict_subject_client(&self, upstream: &str, subject: &str);
async fn evict_upstream_clients(&self, upstream: &str); // used by reload()
```

`get_or_build_auth_client` flow:

1. Compute the resolved `client_id` from current `UpstreamConfig`.
2. `entry((upstream, subject))`:
   - **Occupied** with matching `built_with_client_id` → return cached `Arc`.
   - **Occupied** with mismatched `client_id` → evict, build fresh (D10 — closes silent re-bind).
   - **Vacant** → call `UpstreamOauthManager::build_auth_client`, insert. Build runs without blocking other readers (DashMap entry semantics).
3. Wrap returned `Arc` with the `RefreshLocks` middleware (Task 2 Step 5).

`clear_credentials` semantics: removes cache entry + deletes credential row. In-flight `Arc<AuthClient>` holders complete naturally — this is just Rust ownership, not a designed drain protocol. Document accordingly in `docs/GATEWAY.md`.

`reload()` flow:

1. Snapshot all cache keys under read access (DashMap iter is concurrent-safe but we read keys into a `Vec`).
2. For each key whose `upstream_name` is no longer in the new config: `evict_upstream_clients(name)` — `DashMap::remove`. Per-key write only; no global write lock.
3. Background `cleanup_expired` task continues unaffected.

- [ ] **Step 4: Add OAuth-aware HTTP upstream connection branch**

In `connect_http_upstream(...)` (`dispatch/upstream/pool.rs`):

```rust
match upstream.auth_mode() {
    AuthMode::None | AuthMode::StaticBearer { .. } => { /* unchanged */ }
    AuthMode::Oauth => {
        let subject = current_subject_required()?;
        let auth_client = gateway.get_or_build_auth_client(&upstream.name, &subject).await?;
        StreamableHttpClientWorker::new(auth_client, transport_config_without_auth_header)
    }
}
```

Do not set `transport_config.auth_header` for OAuth-managed connections.

- [ ] **Step 5: Thread authenticated subject into HTTP MCP upstream selection**

Decompose into four substeps (was a single muddled step in the original plan):

**5a.** In `mcp/server.rs` HTTP-bound handlers (list_tools / call_tool / list_resources / get_prompt), read `http::request::Parts` from `RequestContext<RoleServer>.extensions`. (Or task-local fallback per Step 1.)

**5b.** Read `AuthContext` from `Parts.extensions`.

**5c.** Resolve `subject = AuthContext.sub`. If absent: documented fallback — OAuth-enabled upstreams are hidden / unavailable for this request. Static and unauthenticated upstreams continue to work.

**5d.** Pass the resolved subject through to upstream operations. The pool's `connect_http_upstream` (Step 4) consults the resolved subject when building the per-`(upstream, subject)` `AuthClient`.

- [ ] **Step 6: Catalog filter for stdio**

In `catalog.rs`, `actions_for(...)` accepts a `transport: Transport` argument. When `Stdio`, filter out OAuth-tagged upstreams. Document in `docs/UPSTREAM.md`.

- [ ] **Step 7: Run targeted runtime tests**

Required tests (each as a discrete scenario):

```bash
cargo test -p lab subject_scoped_upstream -- --nocapture
cargo test -p lab server_reads_subject_scoped_upstream_pool -- --nocapture
cargo test -p lab catalog_hides_oauth_upstreams_on_stdio -- --nocapture
cargo test -p lab reload_evicts_removed_upstream_oauth_clients -- --nocapture
cargo test -p lab cache_atomic_first_request_no_double_build -- --nocapture
cargo test -p lab cache_refuses_stale_client_id_after_config_change -- --nocapture
cargo test -p lab startup_self_test_request_context_extensions -- --nocapture
```

`cache_atomic_first_request_no_double_build`: spawn N tokio tasks all calling `get_or_build_auth_client` for the same `(upstream, subject)` simultaneously; assert `UpstreamOauthManager::build_auth_client` is invoked exactly once (use `AtomicUsize`).

`cache_refuses_stale_client_id_after_config_change`: prime cache, mutate config to a different `client_id`, assert next call rebuilds and the old `Arc` is dropped (or refused).

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/gateway/manager.rs crates/lab/src/dispatch/upstream/pool.rs crates/lab/src/mcp/server.rs crates/lab/src/catalog.rs
git commit -m "feat: add per-(upstream,subject) auth client cache with reload eviction"
```

---

### Task 5: Expose Operator Surface And Validation

**Files:**
- Modify: `docs/CONFIG.md`
- Modify: `docs/UPSTREAM.md`
- Modify: `docs/GATEWAY.md`
- Modify: `docs/ERRORS.md`
- Modify: `docs/OBSERVABILITY.md`
- Test: `crates/lab/src/config.rs`

- [ ] **Step 1: Document new upstream config shape**

Examples for:
- CIMD-backed PKCE upstream
- Preregistered public client
- Preregistered confidential client (`client_secret_env`)
- Dynamic Client Registration (with notes on initial-access-token handling)
- Mutual exclusion with `bearer_token_env` and the resulting validation error

- [ ] **Step 2: Document `LAB_OAUTH_ENCRYPTION_KEY`**

Operator setup: how to generate (`openssl rand -base64 32`), where to store (`~/.labby/.env`), key-rotation strategy (delete creds, re-authorize). Document failure mode: missing key blocks startup.

- [ ] **Step 3: Document runtime semantics explicitly**

- OAuth-enabled upstreams are HTTP-only in this phase.
- Credentials are stored per `(upstream, lab user)`; refresh tokens encrypted at rest.
- Static bearer upstreams remain global.
- `gateway.reload` evicts cached `AuthClient` entries for upstreams removed from config; persisted credentials are NOT invalidated.
- `clear_credentials` is graceful-drain: in-flight requests complete with old tokens.
- On `invalid_grant`, the user sees `oauth_needs_reauth` and must re-initiate authorization.
- Concurrent refresh is single-flight per `(upstream, subject)`.

Reference prior art for the multi-tenant pattern: Composio (`entity_id:app_slug`), Cloudflare `workers-oauth-provider`, Pipedream Connect.

- [ ] **Step 4: Document browser/operator flow**

1. Operator signs into Labby.
2. Operator starts upstream authorization (`POST /v1/gateway/oauth/start`).
3. Browser navigates to `authorization_url`.
4. Upstream AS authenticates, redirects to `/auth/upstream/callback`.
5. `lab` validates session, takes state row atomically, exchanges code, stores encrypted credentials, redirects to `/gateway/oauth/result`.
6. Subsequent `/mcp` and hosted UI requests use the per-`(upstream, subject)` `AuthClient`.

- [ ] **Step 5: Register error kinds**

Add to `docs/ERRORS.md`:
- `oauth_needs_reauth` — refresh failed with `invalid_grant`; user must re-authorize.
- `oauth_state_invalid` — callback state missing, expired, replayed, or subject-mismatched.
- `oauth_resource_mismatch` — upstream refused the `resource` parameter or returned a token with a wrong audience.
- `oauth_issuer_mismatch` — AS metadata `issuer` did not match the discovered AS URL.
- `oauth_unsupported_method` — upstream AS only offered `plain` PKCE (or omitted PKCE).

- [ ] **Step 6: Update redaction list**

Add to `docs/OBSERVABILITY.md`: `code`, OAuth `state`, all token-response fields (`access_token`, `refresh_token`, `id_token`), `client_secret`, raw `token_blob`/`token_blob_nonce`. None of these may appear at any log level.

- [ ] **Step 7: Run docs/config verification**

```bash
cargo test -p lab config::tests::upstream_oauth -- --nocapture
cargo test -p lab-auth sqlite_store_upstream_oauth -- --nocapture
```

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add docs/CONFIG.md docs/UPSTREAM.md docs/GATEWAY.md docs/ERRORS.md docs/OBSERVABILITY.md
git commit -m "docs: add upstream oauth pkce gateway guidance and error kinds"
```

---

### Task 6: End-To-End Verification

**Files:**
- Test: `crates/lab/src/api/upstream_oauth.rs`
- Test: `crates/lab/src/dispatch/upstream/pool.rs`
- Test: `crates/lab/src/mcp/server.rs`

Task 6 is **trimmed** — manager-level tests (Task 2 §7) cover discovery, registration paths, encryption, refresh semantics, and `invalid_grant`. Task 6 only verifies what the pool-level integration adds beyond that:

- [ ] **Step 1: Subject isolation across the pool boundary**

Two subjects (`alice`, `bob`) make a proxied tool call to the same OAuth-enabled upstream. Assert:
- Two distinct `AuthClient` instances built (one per subject).
- `clear_credentials("alice")` does not affect `bob`'s in-flight or subsequent calls.
- Bob's request continues to work after Alice's eviction.

Reuse Task 2 mock auth-server fixtures.

- [ ] **Step 2: Single-flight refresh enforced at the pool level**

Two parallel proxied calls on the same `(upstream, subject)` pair, both with an expired access token. Assert:
- Exactly one `refresh_token` request reaches the AS (single-flight at the pool level, not just the manager level).
- Both calls succeed with the new token.
- The rotated refresh token is persisted before either call's retry.

- [ ] **Step 3: Run focused end-to-end tests**

```bash
cargo test -p lab upstream_oauth_end_to_end -- --nocapture
```

Expected: PASS

- [ ] **Step 4: Run full crate verification**

```bash
cargo test -p lab-auth -- --nocapture
cargo test -p lab -- --nocapture
cargo test --workspace --all-features --no-fail-fast
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src crates/lab-auth/src
git commit -m "test: verify upstream oauth pkce flow end to end with subject isolation and single-flight refresh"
```

---

## Implementation Notes

- Do not add `rmcp` as a dependency of `lab-auth`. Keep `lab` as the only `rmcp` crate.
- Reuse `lab-auth::sqlite::SqliteStore` and the existing SQLite file, but add new outbound tables instead of overloading inbound auth-server tables.
- Do not reuse `lab-auth`'s Google provider implementation for outbound upstream OAuth. Google is only relevant to `lab`'s inbound login flow.
- Default scope selection should let RMCP auto-select from `WWW-Authenticate`, Protected Resource Metadata, and Authorization Server metadata when config does not pin scopes.
- Prefer Client ID Metadata Documents first, then preregistration, then dynamic registration, matching the MCP authorization spec.
- Do not try to make stdio support user-scoped upstream OAuth in this plan.
- The `/mcp` handler MUST receive authenticated subject identity from HTTP request extensions (verified in Task 4 Step 1). It must not guess. Startup self-test refuses to start if neither propagation path works.
- Do NOT log `code`, OAuth `state`, or any token field at any level. Use redacted `Debug` impls — never `#[derive(Debug)]` on credential or state row types.
- Token-at-rest encryption is mandatory; do not start the manager without `LAB_OAUTH_ENCRYPTION_KEY`. Key is loaded once at `AppState` construction.
- chacha20poly1305 nonce is **always fresh** per `seal()` call; never preserve the prior nonce on upsert.
- Refresh retry is **idempotent-only**; non-idempotent methods surface `oauth_needs_reauth` instead of retrying.
- Decryption failures surface as `oauth_needs_reauth`, never `internal_error`.
- `LAB_PUBLIC_URL` MUST be configured before startup; redirect URIs are derived from it, never from the request `Host` header.
- All HTML rendering of operator-supplied `upstream` names (e.g., the result page) MUST go through the template engine's auto-escape.

## Expected End State

- Operators can connect an OAuth-protected upstream MCP server without asking every user for client secrets.
- Browser-based authorization uses PKCE-S256-only (no fallback) and the OAuth authorization-code flow with RFC 8707 `resource` binding and RFC 8414 issuer-binding verification.
- Tokens persist across restarts (encrypted with chacha20poly1305 + fresh nonce per upsert) and refresh automatically; refresh is single-flight per `(upstream, subject)` with proactive (30 s before expiry) and reactive (on 401) paths. Retries are idempotent-only.
- `invalid_grant` and decryption failure both surface `oauth_needs_reauth`; no silent retry loops, no internal-error leakage.
- OAuth-enabled upstreams are isolated per authenticated `lab` user via a `DashMap<(upstream, subject), AuthClient>` cache with atomic per-key build.
- Cache is guarded against silent re-bind: rebuilds on `client_id` mismatch.
- The global gateway pool, circuit breakers, catalog cache, and stdio behavior remain intact.
- `gateway.reload` evicts removed-upstream cache entries; `clear_credentials` removes cache + row, in-flight requests complete naturally.
- All four OAuth HTTP routes (including `/auth/upstream/callback`) are master-only and explicitly tested as such.
- Background `cleanup_expired` task (60 s) prunes both expired state rows and access-only stale credentials.
- Operator runbook documents `LAB_OAUTH_ENCRYPTION_KEY` rotation: rotate → delete creds → re-authorize.
