# Research Findings: Upstream MCP OAuth PKCE Plan

**Plan:** docs/superpowers/plans/2026-04-17-upstream-mcp-oauth-pkce.md
**Epic bead:** lab-l840
**Date:** 2026-04-17
**Agents (9):** architecture-strategist, security-sentinel, data-integrity-guardian, framework-docs-researcher, best-practices-researcher, learnings-researcher, repo-research-analyst, pattern-recognition-specialist, code-simplicity-reviewer

---

## Top 5 Must-Fix Before Implementation

1. **PKCE S256 enforcement + `plain` rejection** (security CRITICAL)
2. **Callback subject binding to authenticated session** (security CRITICAL — open-redirector / cross-user CSRF risk)
3. **`resource` parameter on auth + token requests** (MCP spec MUST per RFC 8707)
4. **Single-flight refresh per `(upstream, subject)`** (data-integrity HIGH — RT rotation race revokes entire grant on most ASes)
5. **Take-once `DELETE ... RETURNING` atomicity** (data-integrity HIGH — replay window otherwise)

---

## Findings by Plan Task

### Task 1 — Config & Persistence Primitives

**HIGH**
- **Stringly-typed config**: `mode: String` and `strategy: String` should be serde-tagged enums. Flatten `UpstreamOauthRegistrationConfig` into a tagged enum (`ClientMetadataDocument { url }` | `Preregistered { client_id, client_secret_env }`).
- **Schema gaps**: Add `PRIMARY KEY (upstream_name, subject)` on credentials, `PRIMARY KEY (upstream_name, subject, csrf_token)` on state. Mirror existing `NOT NULL` discipline. Plan only lists row fields, not DDL.
- **Take-once must be `DELETE ... RETURNING` in single statement**: Existing `take_authorization_request` is the model. SELECT-then-DELETE creates a replay window even within one connection because the 4-conn pool serializes per-connection only.
- **Missing field**: Credentials row needs `access_token_expires_at: i64` so `cleanup_expired` can prune without re-parsing JSON.
- **Refresh tokens stored plaintext**: Encrypt `token_response_json` at rest (chacha20poly1305 keyed from `~/.labby/.env`) or at minimum add `TODO(sec)` and document file-permission threat.

**MEDIUM**
- **Mutual exclusion**: `oauth: Option<...>` vs existing `bearer_token_env: Option<String>` — plan does not require validation that both cannot be set. Add `validate()` or `serde(try_from)`.
- **State TTL unspecified**: Set hard 10-minute max enforced in `save_upstream_oauth_state`.
- **Cleanup ownership**: Extend existing `cleanup_expired()` (sqlite.rs:398) to cover the two new tables.
- **Tests missing**: Add concurrent take-once race test (mirror `sqlite_store_redeems_auth_code_only_once_under_race`); add concurrent-refresh race test; add upsert-overwrite test.

**Cut/defer (simplicity)**
- Drop `"dynamic"` registration strategy from MVP — no test coverage in plan, ~50 LOC, no concrete upstream needs it.
- Consider replacing `UpstreamOauthStateRow` SQLite table with `DashMap` TTL cache (~100 LOC saved). **Spec note**: best-practices says state SHOULD be server-side + single-use + ≤10 min TTL — DashMap satisfies this.

---

### Task 2 — RMCP Manager + Store Adapters

**rmcp 1.4 API verification** (all confirmed):
- `CredentialStore`, `StateStore`, `OAuthState::new`, `start_authorization_with_metadata_url`, `AuthorizationManager`, `AuthClient<reqwest::Client>` — signatures match plan.
- **Token refresh is MANUAL**: rmcp does NOT auto-refresh on 401. Caller must `auth_manager.refresh_token()` explicitly or implement middleware. **Add explicit step.**
- **AuthClient ↔ StreamableHttpClientWorker integration unclear**: Need a PoC test before Task 2 — confirm whether `AuthClient` is passed as the `client` parameter or wraps the worker separately.
- **Refresh-token rotation requires CredentialStore update**: each successful refresh must persist new tokens.
- **Empty-scopes auto-selection**: plan assumes `start_authorization_with_metadata_url(&[])` triggers metadata-driven scope selection — verify in test.

**HIGH (security)**: Enforce `code_challenge_method=S256` only. Reject `plain`. Plan must assert this in `AuthorizationManager` config.

**HIGH (spec MUST)**: Send `resource` parameter (RFC 8707) on both auth and token requests. Value = canonical upstream MCP URL.

**HIGH (security)**: After RMCP resolves AS metadata, verify `issuer` claim matches discovered AS URL (RFC 8414 §3.3).

**HIGH (data-integrity + security)**: Concurrent refresh race for same `(upstream, subject)` — both requests refresh, second invalidates first's rotated token (most ASes treat replay as attack signal and revoke entire grant). Add single-flight `tokio::sync::Mutex` keyed by `(upstream, subject)` in `UpstreamOauthManager`.

**MEDIUM**: Add explicit redaction rule — never log `code`, `state`, or any deserialized token field. Cross-reference docs/OBSERVABILITY.md.

**MEDIUM (spec)**: On `invalid_grant` from token endpoint, MUST restart full flow — delete creds, mark `needs_reauth`. No silent retry.

---

### Task 3 — HTTP Routes

**CRITICAL**: Callback handler MUST fail-fast when no authenticated session is present. Subject MUST be derived from session/bearer (NOT from `state` parameter). Validate that derived subject equals subject stored in pending state row. Open-redirector / cross-user CSRF risk otherwise.

**HIGH**: Constrain success/failure redirect target to same-origin allowlist (e.g., from `LAB_CORS_ORIGINS`). Never use caller-supplied URL.

**HIGH**: `/v1/gateway/oauth/status` must never serialize `client_secret` or any token material. Audit response shape.

**HIGH (pattern)**: Route prefix split — begin/status/clear under `/v1/gateway/oauth/*`, callback under `/auth/upstream/callback` (mirrors `/auth/google/callback`). Document intentionally or consolidate.

**MEDIUM (architecture)**: Add a `dispatch/gateway/oauth.rs` shim so manager calls go through dispatch layer. Preserves layer contract; enables future CLI command.

**MEDIUM (repo-research)**: Destructive `clear` action — use `confirmation_required` envelope per docs/ERRORS.md.

**LOW (security)**: Build redirect URI from configured base URL (not request Host header) to prevent confusion behind reverse proxy.

**LOW (pattern)**: `api/upstream_oauth.rs` placement — other per-service routes live in `api/services/*`. Either move or document the exception.

**Cut (simplicity)**: Pick JSON `authorization_url` only; defer browser-redirect mode (browser can do `window.location.href = url`).

---

### Task 4 — Subject-Scoped Pool

**HIGH (architecture)**: `HashMap<String, Arc<UpstreamPool>>` is unbounded. With N users × M OAuth upstreams, FDs and memory grow linearly. Add LRU + idle-TTL eviction (e.g., `moka` or `RwLock<LruCache>`); make cap configurable; emit metric.

**HIGH (architecture)**: Cache invalidation refcount gap — removing an entry from the map drops the map's strong ref but in-flight requests still hold their `Arc`. Document `clear_credentials` as graceful-drain, OR add notify-based cancellation. Add test for "clear during in-flight call".

**HIGH (pattern)**: Consider `HashMap<(String, String), Arc<AuthClient<reqwest::Client>>>` keyed by `(upstream_name, subject)` instead of duplicating entire `UpstreamPool` per subject. Avoids implementor duplicating connection-management infrastructure (circuit breakers, catalog caches, discovery). Plan should weigh this explicitly.

**MEDIUM (architecture)**: `gateway.reload()` must walk subject pools and evict entries for upstreams removed from config — otherwise subject pools become parallel reality diverging from operator config.

**MEDIUM (architecture)**: `RequestContext.extensions` → `http::request::Parts` propagation is an rmcp-side feature; `mcp/server.rs` does not currently read this. Add explicit verification step that the rmcp version supports it for streamable-HTTP server handlers. If not, need custom axum middleware with task-local.

**MEDIUM (pattern)**: Task 4 Step 4 collapses extraction (`Parts`), reading (`AuthContext`), resolution (`sub`), and dispatch (pool select) into one step. Split — the resolution step has a documented fallback ("OAuth-enabled upstreams hidden if no auth").

**MEDIUM (architecture)**: stdio scope-out — catalog must dynamically filter OAuth-tagged upstreams based on transport, otherwise stdio clients see tools that always 401. `actions_for()` takes `transport: Transport` hint.

---

### Task 5 — Docs

Mostly fine. Add operator guidance for:
- Multi-tenant pattern keying by `(upstream_name, subject)` — prior art: Composio `entity_id:app_slug`, Cloudflare `workers-oauth-provider`, Pipedream Connect.
- Token-at-rest encryption decision.
- Cache eviction tuning knobs once added.

---

### Task 6 — End-to-End

**Simplicity**: Scope to the proxied tool-call path only; reuse Task 2 Step 5 mock auth-server fixtures rather than re-implementing.

Add coverage for:
- `invalid_grant` triggers re-auth flow
- Different subjects do not share connection (subject isolation)
- Concurrent refresh single-flight

---

## Cross-Cutting Conventions (repo-research)

- **ToolError serialization**: never derive `Serialize`; hand-write or wrap (sdk_kind promotion is hand-coded).
- **Error kinds**: any new `kind` (e.g., `upstream_oauth_failed`, `needs_reauth`) must be declared in `docs/ERRORS.md` first.
- **Async trait style**: use native `async fn in trait`. Do NOT add `async-trait` crate.
- **Standard dispatch fields**: `surface`, `service`, `action`, `elapsed_ms`, `kind` on failure (per docs/OBSERVABILITY.md). Plan must instrument routes/manager accordingly.
- **Module style**: no `mod.rs` files. `oauth/upstream.rs` declares `oauth/upstream/{manager,store,types}.rs`.

---

## Boundary Reaffirmed (learnings)

- Inbound (lab-auth Google) and outbound (upstream OAuth) are entirely separate. `lab-auth/src/google.rs` MUST NOT be reused.
- Reuse only generic SQLite/CRUD/session-callback patterns.
- Subject-scoped upstream credentials and PKCE state are net-new — no prior incidents to mine.

---

## Open Questions for /lavra-design

1. **State storage**: SQLite (per plan) vs DashMap TTL cache (simplicity). Both spec-compliant.
2. **Cache shape**: per-subject `UpstreamPool` (per plan) vs per-`(upstream,subject)` `AuthClient` (pattern). Affects LRU complexity.
3. **Token-at-rest encryption**: implement now, or `TODO(sec)` for follow-up?
4. **Dynamic Client Registration**: drop from MVP (simplicity) or keep as third strategy (spec SHOULD)?
5. **rmcp PoC**: validate `AuthClient` ↔ `StreamableHttpClientWorker` integration before Task 2 Step 4.

---

## Spec Sources

- MCP Authorization 2025-06-18: https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization
- OAuth 2.1 draft-13, RFC 9728 (PRM), RFC 8414 (AS metadata), RFC 7636 (PKCE), RFC 7591 (DCR), RFC 8707 (Resource Indicators), RFC 9700 (Security BCP)
- CIMD: draft-ietf-oauth-client-id-metadata-document-01
- rmcp 1.4: https://docs.rs/rmcp/latest/rmcp/transport/auth/
- Prior art: Cloudflare workers-oauth-provider, Composio entity_id pattern, Pipedream Connect, krasserm.github.io/2025/08/06/agent-authorization
