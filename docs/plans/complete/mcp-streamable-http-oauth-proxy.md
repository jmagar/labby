# Plan: Streamable HTTP + OAuth + MCP Proxy for Lab

**Goal:** Upgrade lab's MCP server to MCP 2025-11-25 compliance with streamable HTTP transport, add an MCP client/proxy for upstream servers, and secure the HTTP endpoint with OAuth 2.1 (resource server role).

**Spec baseline:** MCP 2025-11-25 specification, RFC 9728, RFC 8414, RFC 8707.

**Contract baseline:** `docs/RMCP.md` — lab's normative integration contract with rmcp.

**rmcp 1.4 verification:** All 9 feature flags confirmed on crates.io. `StreamableHttpService` constructor, `StreamableHttpServerConfig` fields, `LocalSessionManager` with native TTL, and `tower::Service` impl all verified against source at `~/.cargo/registry/src/*/rmcp-1.4.0/`. **GO — no blockers.**

---

## Phase 0: rmcp 1.4 Upgrade + Streamable HTTP Server

### What changes

| File | Change |
|------|--------|
| `Cargo.toml` (workspace) | Bump `rmcp` from `"1.3"` to `"1.4"`, add features (see below). **Verify on crates.io that 1.4 publishes these features before starting.** |
| `crates/lab/Cargo.toml` | No change — inherits workspace rmcp |
| `crates/lab/src/mcp/server.rs` | **New file.** `LabMcpServer` struct + `ServerHandler` impl, extracted from `serve.rs` |
| `crates/lab/src/mcp.rs` | Add `pub mod server;` declaration |
| `crates/lab/src/cli/serve.rs` | Remove `LabMcpServer` and helpers; import from `crate::mcp::server`. Keep transport/port resolution and CLI args only. |
| `crates/lab/src/api/router.rs` | Mount `StreamableHttpService` at `/mcp` inside the protected sub-router alongside `/v1`. Restructure auth scope (see Auth Restructuring below). |

### rmcp workspace dependency (target)

```toml
rmcp = { version = "1.4", features = [
  "server",
  "macros",
  "transport-io",
  "transport-streamable-http-server",
  "client",
  "auth",
  "transport-streamable-http-client",
  "elicitation",
  "schemars",
] }
```

**Prerequisite gate:** Verify these feature names exist in the published rmcp 1.4 on crates.io. If they don't exist or have different names, Phase 0 is blocked until resolved.

### Transport variant

No new transport variant. The existing `Http` transport gains the `/mcp` mount point. When you run `lab serve --transport http`, you get both `/v1/*` (REST API) and `/mcp` (MCP streamable HTTP). This follows RMCP.md: "HTTP MCP is mounted inside the Axum application under a dedicated MCP path such as `/mcp`."

The `Transport` enum stays as-is: `Stdio` and `Http`.

### Auth middleware restructuring

**Problem:** The current bearer auth middleware (router.rs:94-118) wraps only the `/v1` sub-router. Health probes are deliberately exempt (mounted on the outer router at lines 124-127). If `/mcp` is mounted on the outer router, it bypasses auth. If the entire router is wrapped, health probes break.

**Solution:** Create a `protected` sub-router that nests both `/v1` and `/mcp`, apply bearer auth to that, then mount alongside unprotected routes:

```rust
// Build the protected sub-router containing both /v1 and /mcp
let mut protected = Router::new()
    .nest("/v1", v1);

// Mount MCP streamable HTTP
let mcp_service = build_mcp_service(&state);
protected = protected.nest_service("/mcp", mcp_service);

// Apply bearer auth to the protected sub-router
let protected = if let Some(token) = bearer_token {
    let token = Arc::<str>::from(token);
    protected.layer(axum::middleware::from_fn(move |req, next| {
        let token = token.clone();
        async move { /* existing constant-time bearer check */ }
    }))
} else {
    protected
};

// Outer router: health probes (no auth) + protected routes (auth)
Router::new()
    .route("/health", get(health::health))
    .route("/ready", get(health::ready))
    .route("/.well-known/oauth-protected-resource", get(oauth_metadata)) // Phase 1
    .merge(protected)
    .with_state(state)
    .layer(/* middleware stack */)
```

This ensures `/mcp` gets the same auth treatment as `/v1/*` while keeping health probes and discovery endpoints exempt.

### Mounting strategy

```rust
use rmcp::transport::streamable_http_server::{
    StreamableHttpService, StreamableHttpServerConfig, session::LocalSessionManager,
};

fn build_mcp_service(state: &AppState) -> StreamableHttpService<LabMcpServer, LocalSessionManager> {
    let registry = Arc::clone(&state.registry);
    let session_manager = Arc::new(LocalSessionManager::default());

    // allowed_hosts must include the public hostname when behind a reverse proxy.
    // Default is ["localhost", "127.0.0.1", "::1"] — rmcp rejects requests whose
    // Host header doesn't match.
    let allowed_hosts = allowed_hosts_from_env();

    let config = StreamableHttpServerConfig {
        allowed_hosts,
        stateful_mode: stateful_mode_from_env(), // default: true
        ..StreamableHttpServerConfig::default()
    };

    StreamableHttpService::new(
        move || {
            let reg = Arc::clone(&registry);
            Ok(LabMcpServer { registry: reg })
        },
        session_manager,
        config,
    )
}
```

Key points:
- `LabMcpServer` already implements `ServerHandler` — the factory closure just clones the `Arc<ToolRegistry>` and constructs a new instance per session. Construction cost: two Arc increments (negligible).
- `LocalSessionManager` is rmcp's built-in in-memory session manager. Sufficient for single-process deployment.
- `AppState.registry` is already `Arc<ToolRegistry>` (confirmed in `state.rs:134`). The factory closure clones it directly — no wrapping needed.

### `allowed_hosts` configuration

`LAB_MCP_ALLOWED_HOSTS` — comma-separated hostnames rmcp accepts in the `Host` header. Required when behind a reverse proxy (Caddy/Traefik) that forwards the public hostname.

```rust
fn allowed_hosts_from_env() -> Vec<String> {
    let mut hosts = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    if let Ok(extra) = std::env::var("LAB_MCP_ALLOWED_HOSTS") {
        for h in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            // Reject wildcard — would disable Host header validation entirely
            if h == "*" {
                tracing::warn!("ignoring wildcard '*' in LAB_MCP_ALLOWED_HOSTS — would disable DNS rebinding protection");
                continue;
            }
            if !hosts.contains(&h.to_string()) {
                hosts.push(h.to_string());
            }
        }
    }
    // If LAB_RESOURCE_URL is set (Phase 1), auto-extract and add its hostname
    if let Ok(url) = std::env::var("LAB_RESOURCE_URL") {
        if let Ok(parsed) = url::Url::parse(&url) {
            if let Some(host) = parsed.host_str() {
                let h = host.to_string();
                if !hosts.contains(&h) {
                    hosts.push(h);
                }
            }
        }
    }
    hosts
}
```

### Session lifecycle — native TTL via `SessionConfig`

`LocalSessionManager` has **built-in session eviction** via `SessionConfig.keep_alive` (verified in rmcp 1.4.0 source). No custom reaper needed.

```rust
use rmcp::transport::streamable_http_server::session::{LocalSessionManager, SessionConfig};

let session_config = SessionConfig {
    keep_alive: Some(Duration::from_secs(
        std::env::var("LAB_MCP_SESSION_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300) // 5 min default (matches rmcp default)
    )),
    ..SessionConfig::default()
};
let session_manager = Arc::new(LocalSessionManager {
    session_config,
    ..LocalSessionManager::default()
});
```

**Warning:** `keep_alive: None` disables eviction entirely. Document that operators should never set this to `None` on long-running servers. The `LAB_MCP_SESSION_TTL_SECS` env var only accepts positive integers.

### `LabMcpServer` relocation

Move from `serve.rs` to `mcp/server.rs`:

**Moves to `mcp/server.rs`:**
- `LabMcpServer` struct + `ServerHandler` impl
- `action_schema()` helper
- `elicit_confirm()` + `ElicitResult` enum
- `extract_error_info()` + `static_kind()` helpers

**Stays in `serve.rs`:**
- `ServeArgs`, `Transport` enum
- `run()`, `run_stdio()`, `run_http()`
- `resolve_transport()`, `resolve_port()`, `require_http_token()`, `filter_registry()`

**Test split:**
- `static_kind_round_trips_all_tool_error_kinds` → moves to `mcp/server.rs` (tests server error handling)
- Transport/port resolution tests → stay in `serve.rs`

### No-auth safety gate

When no auth is configured (neither `LAB_MCP_HTTP_TOKEN` nor `LAB_OAUTH_ISSUER`), lab currently warns and runs unprotected. This is dangerous if the bind address isn't localhost.

**Rule:** If `host` is not `127.0.0.1`, `::1`, or `localhost` AND no auth mechanism is configured, **refuse to start** with a clear error:

```
error: refusing to bind HTTP on 0.0.0.0:8765 without authentication.
Set LAB_MCP_HTTP_TOKEN or LAB_OAUTH_ISSUER, or bind to 127.0.0.1 for local-only access.
```

This prevents accidental unauthenticated deployment on a LAN-accessible address.

### Config summary (Phase 0)

| Var | Purpose | Default |
|-----|---------|---------|
| `LAB_MCP_ALLOWED_HOSTS` | Comma-separated hostnames for Host header validation | `localhost,127.0.0.1,::1` |
| `LAB_MCP_STATEFUL` | `true`/`false` — toggle stateful session mode | `true` |
| `LAB_MCP_SESSION_TTL_SECS` | Idle session eviction TTL in seconds (rmcp native) | `300` (5 min) |

### Verification

- `lab serve --transport http` starts, `/mcp` responds to MCP POST with valid `Mcp-Protocol-Version: 2025-11-25` header
- Existing `/v1/*` routes continue to work
- `lab serve --transport stdio` is unaffected
- Bearer token auth applies to `/mcp` same as `/v1/*`
- `/health` and `/ready` remain auth-exempt
- Session reaper evicts stale sessions (test with manual disconnect)
- Non-localhost bind without auth refuses to start
- Wildcard in `LAB_MCP_ALLOWED_HOSTS` is rejected with warning

---

## Phase 1: OAuth 2.1 Resource Server

### What lab is NOT

Lab is **not** an OAuth authorization server. It does not implement `/auth/login`, `/auth/callback`, or any token issuance flow. Per MCP 2025-11-25:

> The MCP server acts as an OAuth 2.1 resource server — it validates tokens, it does not issue them.

Clients (Claude Desktop, agent frameworks, custom tools) perform the OAuth dance with an external authorization server (Google, GitHub, Keycloak, etc.) and present the resulting access token to lab.

### What lab implements

1. **RFC 9728 Protected Resource Metadata** — `GET /.well-known/oauth-protected-resource`
2. **`WWW-Authenticate` header** on 401 responses
3. **Token validation** (JWT signature verification via JWKS)
4. **Scope extraction** (informational — injected into request context)

### New files

| File | Purpose |
|------|--------|
| `crates/lab/src/api/oauth.rs` | `OAuthConfig`, `JwksManager`, `AuthContext`, metadata endpoint handler, JWT validation middleware |

### Protected Resource Metadata endpoint

```
GET /.well-known/oauth-protected-resource
```

Returns:

```json
{
  "resource": "https://lab.example.com",
  "authorization_servers": ["https://accounts.google.com"],
  "scopes_supported": ["lab:read", "lab:admin"],
  "bearer_methods_supported": ["header"]
}
```

This endpoint is unauthenticated (mounted on the outer router alongside health probes). Clients need it to discover which auth server to use.

### 401 response shape

When a request arrives without a valid token:

```
HTTP/1.1 401 Unauthorized
WWW-Authenticate: Bearer resource_metadata="https://lab.example.com/.well-known/oauth-protected-resource"
```

### Auth middleware evolution

The existing bearer token middleware does static string comparison. This phase replaces it with a layered auth stack:

1. **Static bearer** — `LAB_MCP_HTTP_TOKEN` (existing, for backward compatibility and simple deployments)
2. **OAuth JWT** — `LAB_OAUTH_ISSUER` (new, for production deployments)

If both are configured, either is accepted (try static bearer first — it's cheaper). If neither is configured and binding to a non-localhost address, refuse to start (Phase 0 safety gate).

### JWT validation pipeline

```rust
pub struct JwksManager {
    issuer: String,
    audience: String,
    cache: Arc<RwLock<JwksCache>>,
    refresh_semaphore: tokio::sync::Semaphore, // prevents thundering herd
    http: reqwest::Client,
}

struct JwksCache {
    keys: jsonwebtoken::jwk::JwkSet,
    fetched_at: std::time::Instant,
    ttl: std::time::Duration, // default: 1 hour
}

pub struct AuthContext {
    pub sub: String,
    pub scopes: Vec<String>,
    pub issuer: String,
}
```

**Pipeline steps:**

1. **JWKS discovery** — On startup, fetch `{issuer}/.well-known/openid-configuration`, extract `jwks_uri`, fetch the JWKS document. Use `reqwest` (already in `lab`'s Cargo.toml).
   - **Security:** Validate `LAB_OAUTH_ISSUER` uses HTTPS. Reject HTTP issuers (log error, refuse to start). The JWKS URI is derived from the OIDC discovery document — do not follow redirects to arbitrary hosts (use `reqwest::redirect::Policy::none()` for the JWKS fetch).

2. **JWKS cache with stale-while-revalidate** — `Arc<RwLock<JwksCache>>` with TTL-based refresh (default 1 hour). **Critical:** Never discard a working JWKS on TTL expiry if the refresh fetch fails. Keep the last-known-good keys and log at WARN. This prevents total auth outage when the issuer (Authelia, Authentik, Keycloak) reboots — common in homelab.

3. **Eager refresh with thundering herd protection** — On `kid` mismatch during validation failure, attempt an eager refresh. Use a `tokio::sync::Semaphore(1)` to serialize concurrent refreshes with double-checked locking:

```rust
async fn ensure_kid(&self, kid: &str) -> Result<(), AuthError> {
    // Fast path: already cached
    if self.cache.read().await.has_kid(kid) { return Ok(()); }
    // Serialize refreshes
    let _permit = self.refresh_semaphore.acquire().await?;
    // Recheck after acquiring (another task may have refreshed)
    if self.cache.read().await.has_kid(kid) { return Ok(()); }
    // Fetch and install (stale-while-revalidate on failure)
    match self.fetch_jwks().await {
        Ok(new_keys) => { *self.cache.write().await = new_keys; Ok(()) }
        Err(e) => {
            tracing::warn!(error = %e, "JWKS refresh failed — serving stale keys");
            Err(AuthError::JwksRefreshFailed)
        }
    }
}
```

4. **Signature verification** — Match incoming JWT `kid` header to cached JWKS keys. Verify signature using `jsonwebtoken::decode()`.

5. **Claims validation** — Check `exp` (not expired), `iss` (matches `LAB_OAUTH_ISSUER`), `aud` (matches `LAB_OAUTH_AUDIENCE` per RFC 8707).

6. **Context injection** — Extract `sub` and `scope`/`scp` claims into `axum::Extension<AuthContext>` for downstream handlers.

### Scope enforcement

Two scope levels — mapped directly to `ActionSpec.destructive`:

| Scope | Allows |
|-------|--------|
| `lab:read` | All non-destructive actions (list, get, search, add, update, help, schema) |
| `lab:admin` | Everything, including destructive actions (delete, purge, extract.apply) |

The distinction between "mutating non-destructive" and "destructive" already exists via `ActionSpec.destructive`. A three-level scope (read/write/admin) would require classifying every action as read vs. write, which doesn't map cleanly to the existing action metadata. Two levels maps directly: `destructive == false` → `lab:read` suffices, `destructive == true` → requires `lab:admin`.

Static bearer tokens implicitly have `lab:admin` scope (full access).

### Config

New env vars:

| Var | Purpose | Example |
|-----|---------|---------|
| `LAB_OAUTH_ISSUER` | OIDC issuer URL (must be HTTPS) | `https://accounts.google.com` |
| `LAB_OAUTH_AUDIENCE` | Expected `aud` claim (RFC 8707) | `https://lab.example.com` |
| `LAB_OAUTH_CLIENT_ID` | Optional — extra `azp` claim validation | |
| `LAB_RESOURCE_URL` | Public URL of this lab instance (for metadata + allowed_hosts) | `https://lab.example.com` |

New `config.toml` section:

```toml
[oauth]
issuer = "https://accounts.google.com"
audience = "https://lab.example.com"
resource_url = "https://lab.example.com"
```

### New workspace dependency

```toml
jsonwebtoken = "9"
```

### Verification

- `/.well-known/oauth-protected-resource` returns valid metadata (unauthenticated)
- Request without token gets 401 with `WWW-Authenticate` header
- Valid JWT from configured issuer grants access
- Expired JWT returns 401
- JWT with wrong `aud` returns 401
- Static bearer token still works when `LAB_MCP_HTTP_TOKEN` is set
- Stale JWKS serves during issuer downtime (log warning, don't reject)
- HTTP issuer URL is rejected at startup
- Scope enforcement: `lab:read` token cannot call destructive actions

---

## Phase 2: MCP Client — Connecting to Upstream Servers

### What this is

Lab acts as an MCP **client** connecting to upstream MCP servers. This lets lab proxy upstream tools through its own MCP surface, giving users a single authenticated entry point.

### Architecture

```
Agent/Client → lab (MCP server) → upstream MCP servers (MCP clients)
                    ↓
              lab's own 22 services (dispatch layer)
```

### New files

| File | Purpose |
|------|--------|
| `crates/lab/src/mcp/upstream.rs` | `UpstreamPool` — manages connections to upstream MCP servers |
| `crates/lab/src/mcp/proxy.rs` | Proxy dispatch — routes upstream tool calls through lab's MCP surface |
| `crates/lab/src/dispatch/upstream.rs` | Dispatch entry point for upstream tools |

### `DispatchFn` type change (blocking prerequisite)

**Problem:** The current `DispatchFn` in `mcp/registry.rs:15` is a `fn` pointer:

```rust
pub type DispatchFn =
    fn(String, Value) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>>;
```

A `fn` pointer cannot capture state. Upstream dispatchers need captured state (the upstream client connection, credentials). This is a structural incompatibility.

**Solution:** Change `DispatchFn` to a trait object that supports closures:

```rust
pub type DispatchFn = Arc<
    dyn Fn(String, Value) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>>
        + Send
        + Sync
>;
```

Existing `fn` pointers implement `Fn` but do **not** auto-coerce to `Arc<dyn Fn>`. The `dispatch_fn!` macro must explicitly wrap: `Arc::new(|action, params| Box::pin(async move { $f(&action, params).await }))`. The `RegisteredService` struct keeps `dispatch: DispatchFn`. Test the coercion in a unit test before relying on it across all 22 services.

**Alternatively**, keep `DispatchFn` as-is for built-in services and add a separate `UpstreamDispatcher` trait with its own dispatch path. This avoids changing the existing type but requires upstream tools to be routed differently in `LabMcpServer::call_tool`. The unified `DispatchFn` approach is cleaner.

### `ToolRegistry` mutability for dynamic upstream discovery

**Problem:** `ToolRegistry` is `Arc<ToolRegistry>` — no interior mutability. Phase 2 requires runtime catalog changes when upstream tools are discovered or removed.

**Solution:** Keep the static `Arc<ToolRegistry>` for built-in services. Add a separate `Arc<RwLock<UpstreamCatalog>>` for upstream tools:

```rust
pub struct UpstreamCatalog {
    services: Vec<RegisteredService>,
    last_discovered: std::time::Instant,
}

// In LabMcpServer:
struct LabMcpServer {
    registry: Arc<ToolRegistry>,           // immutable, built-in services
    upstream: Arc<RwLock<UpstreamCatalog>>, // mutable, upstream services
}

// list_tools merges both:
async fn list_tools(&self, ...) -> Result<ListToolsResult, ErrorData> {
    let mut tools: Vec<Tool> = self.registry.services().iter()
        .map(|svc| Tool::new(svc.name, svc.description, ...))
        .collect();
    let upstream = self.upstream.read().await;
    tools.extend(upstream.services.iter()
        .map(|svc| Tool::new(svc.name, svc.description, ...)));
    Ok(ListToolsResult::with_all_items(tools))
}
```

This avoids destabilizing the existing registry path, keeps compile-time feature gates untouched, and makes the built-in vs. upstream distinction structurally visible.

### Config

Upstream servers configured in `config.toml` (connection metadata only — **credentials in env vars**, following lab's established convention):

```toml
[[upstream]]
name = "filesystem"
url = "http://localhost:3001/mcp"
bearer_token_env = "FILESYSTEM_MCP_TOKEN"  # name of env var holding the token

[[upstream]]
name = "github"
url = "http://localhost:3002/mcp"
bearer_token_env = "GITHUB_MCP_TOKEN"

[[upstream]]
name = "memory"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-memory"]
```

**Credential convention:** `bearer_token_env` is the *name* of the env var, not the token itself. The actual secret lives in `~/.labby/.env` as `FILESYSTEM_MCP_TOKEN=abc123`. This preserves lab's config/secret split.

### URL validation for upstream servers

Validate upstream URLs at parse time:
- Reject `file://`, `ftp://`, and other non-HTTP(S) schemes
- Reject `0.0.0.0` and `::` (wildcard addresses)
- Private IP ranges (10.x, 172.16-31.x, 192.168.x) are the expected case in homelab — do not warn about them

### Tool mapping

Each upstream server becomes a lab tool following the one-tool-per-service pattern:

```jsonc
// Upstream "github" server exposes tools: create_issue, list_repos, search_code
// Lab maps these as actions under a single "github" tool:
github({ "action": "create_issue", "params": { "repo": "...", "title": "..." } })
github({ "action": "list_repos" })
github({ "action": "help" })     // lists available upstream actions
github({ "action": "schema", "params": { "action": "create_issue" } })  // proxied schema
```

### Upstream schema proxying

**Problem:** Upstream MCP tools have their own JSON schemas (each tool has its own `inputSchema`). Wrapping them as actions under a single lab tool would lose per-tool schema information, degrading agent performance for tools with complex inputs.

**Solution:** The `schema` built-in action proxies upstream tool schemas:

```rust
// When action == "schema" for an upstream service:
// 1. Look up the requested upstream tool by params.action
// 2. Return its inputSchema (captured during discovery)
// 3. Cache schemas alongside tool names in UpstreamCatalog
```

The `UpstreamCatalog` stores `(tool_name, description, input_schema)` tuples per upstream, not just tool names. The `help` action returns tool names + descriptions. The `schema` action returns the full `inputSchema` for a specific upstream tool.

### Transport support

- **HTTP upstream:** `StreamableHttpClientTransport` from rmcp. Build one transport per upstream at startup, wrap in `Arc`, reuse across sessions. Do not reconstruct per call.
- **Stdio upstream:** `StdioTransport` from rmcp. Spawns a child process. Wrap all child process interactions in `tokio::time::timeout`. On timeout, kill the child (`child.kill()`), log PID + upstream name, mark upstream unhealthy, attempt restart.

### Dynamic discovery

Upstream tool lists can change. The pool re-discovers on a background task:

- Default interval: 5 minutes (configurable via `LAB_UPSTREAM_DISCOVERY_INTERVAL_SECS`)
- **Parallel discovery:** poll all upstreams concurrently (`FuturesUnordered`), not sequentially. One hung upstream must not block others.
- **Per-upstream timeout:** 15 seconds on `list_tools`. Exceeding this marks the upstream unhealthy.
- **Atomic catalog swap:** Build the new `UpstreamCatalog` in a local variable, then swap it into the `Arc<RwLock<UpstreamCatalog>>`. Never block normal dispatch during discovery.
- **On-demand refresh:** `help` action can trigger immediate re-discovery for its upstream.

### Circuit breaker for dead upstreams

An upstream that is down but still registered causes 30-second timeout hangs on every call. The circuit breaker:

1. Track consecutive failures per upstream (counter in `UpstreamCatalog`)
2. After N consecutive failures (default: 3), mark upstream as `unhealthy`
3. Unhealthy upstreams are excluded from the merged catalog (`list_tools` won't show them)
4. Accelerated re-probing for unhealthy upstreams (every 30 seconds instead of 5 minutes)
5. On successful probe, reset failure counter and re-include in catalog
6. Log state transitions at INFO level

### Conflict resolution

If an upstream server name collides with a built-in lab service name (e.g., someone names their upstream "radarr"):
- Built-in lab services **permanently** take precedence
- The upstream is rejected at startup with a clear warning
- Built-in names are reserved — they cannot be overridden by upstream discovery regardless of timing
- The reserved name list is derived from `ToolRegistry.services()` at startup

### Per-request client construction fix (pre-existing issue)

**Problem:** MCP dispatch constructs a new service client (new `reqwest::Client`, new TLS session, fresh env var reads) per tool call via `require_client()`. The API surface solved this with `ServiceClients` in `AppState`. Phase 2 compounds this — each proxied call pays the cost twice.

**Fix:** Thread `Arc<ServiceClients>` from `AppState` into `LabMcpServer` and pass it through to dispatch. The `dispatch_with_client()` function already exists for this — just provide the pre-built client instead of calling `require_client()`.

```rust
struct LabMcpServer {
    registry: Arc<ToolRegistry>,
    upstream: Arc<RwLock<UpstreamCatalog>>,
    clients: Arc<ServiceClients>,  // pre-built, reused across calls
}
```

### Verification

- Upstream HTTP server connects, tools listed under lab's MCP surface
- Upstream stdio server connects via child process spawn
- Tool calls are proxied correctly (action+params → call_tool → envelope)
- `help` action on upstream tool lists all upstream tools with descriptions
- `schema` action returns upstream tool's `inputSchema`
- Name collision produces clear startup warning, built-in wins
- Dead upstream triggers circuit breaker after 3 failures
- Unhealthy upstream excluded from `list_tools`, re-probed at 30s intervals
- One hung upstream during discovery doesn't block others (parallel + timeout)
- Upstream credentials never appear in logs or error messages

---

## Phase 3: MCP Proxy — Full Gateway

### What this is

Lab's MCP server exposes all upstream tools alongside its own services. An agent connecting to lab sees one unified tool catalog: lab's 22 services + N upstream servers, all behind lab's single auth boundary.

### Gateway features

1. **Unified catalog** — `list_tools` returns both lab services and upstream proxied tools
2. **Single auth** — agents authenticate once with lab (OAuth or bearer); lab handles upstream auth independently
3. **Transparent proxying** — upstream tool calls are forwarded with correct params, results returned in lab's envelope format
4. **Error normalization** — upstream errors are wrapped in lab's structured error envelopes
5. **Response size cap** — proxied results bounded to prevent OOM
6. **Resource proxying** — upstream resources optionally exposed through lab

### Error envelope for upstream failures

```jsonc
{
  "kind": "upstream_error",
  "service": "github",
  "action": "create_issue",
  "message": "upstream server returned error",
  "upstream_error": { /* raw upstream error, sanitized */ }
}
```

New kind `upstream_error` added to the canonical vocabulary in `docs/ERRORS.md` and `static_kind()` in `mcp/server.rs`.

### Response size cap

Upstream MCP servers could return arbitrarily large payloads. Lab reads the entire response into memory before forwarding — an unbounded response would cause OOM.

**Mitigation:** Configurable response size cap (default: 10MB). Uses the existing `upstream_error` kind — no new error kind needed:

```jsonc
{
  "kind": "upstream_error",
  "service": "github",
  "action": "export_all",
  "message": "upstream response (52.3 MB) exceeds configured limit (10 MB)"
}
```

Env var: `LAB_UPSTREAM_MAX_RESPONSE_BYTES` (default: `10485760`).

### Upstream response sanitization

Upstream servers are semi-trusted. Their responses are treated as opaque data:
- Upstream tool results are wrapped in lab's envelope as raw `Value` — never deserialized into lab's internal types
- Error messages from upstream are included verbatim but nested under `upstream_error` key — never promoted to lab's top-level `message` field
- Upstream responses that claim to be lab error envelopes (with `kind` field) are not honored — lab always wraps them

### Resource proxying

Upstream resources (`list_resources` / `read_resource`) are proxied:
- Prefixed with upstream name: `github://repo/list` → `lab://upstream/github/repo/list`
- Enabled per upstream with `proxy_resources = true` in config
- Resource list merged into `list_resources` alongside lab's own resources
- Resource reads are proxied with the same timeout and size cap as tool calls

### Catalog merging

`list_tools` concatenates built-in tools from `ToolRegistry` with upstream tools from `UpstreamCatalog` on each call. At 22 built-in + N upstream tools with 1-5 concurrent agents, the Vec allocation is microseconds — no caching needed. If upstream count grows significantly, add caching later.

### Verification

- Agent sees unified tool list (lab + upstream)
- Upstream tool call succeeds end-to-end
- Upstream failure returns structured error with `kind: "upstream_error"`
- Auth is enforced at lab's boundary, not per-upstream
- Response exceeding size cap returns `response_too_large` error
- Upstream resources appear with `lab://upstream/` prefix
- Merged catalog is cached and doesn't allocate per `list_tools` call

---

## Implementation Order

```
Phase 0  ──────────────────────────────────────────
  0.1  Bump rmcp 1.3 → 1.4 + add features in workspace Cargo.toml
       (rmcp 1.4 verified: all feature flags confirmed)
  0.2  Extract LabMcpServer to mcp/server.rs (with test split)
  0.3  Restructure auth middleware: protected sub-router for /v1 + /mcp
  0.4  Mount StreamableHttpService at /mcp in router.rs
  0.5  Configure SessionConfig.keep_alive (rmcp native TTL)
  0.6  Add allowed_hosts config (LAB_MCP_ALLOWED_HOSTS)
  0.7  Add no-auth safety gate for non-localhost binds
  0.8  Verify: stdio + HTTP + /mcp all work, auth scoped correctly

Phase 1  ──────────────────────────────────────────
  1.1  Add jsonwebtoken to workspace deps
  1.2  Add /.well-known/oauth-protected-resource endpoint (unauthenticated)
  1.3  Add WWW-Authenticate header to 401 responses
  1.4  Implement JwksManager with stale-while-revalidate + thundering herd protection
  1.5  Implement JWT validation middleware
  1.6  Layer auth: static bearer OR OAuth JWT (try bearer first)
  1.7  Implement scope extraction + enforcement (read/admin — 2 levels)
  1.8  Config: LAB_OAUTH_ISSUER (HTTPS-only), LAB_OAUTH_AUDIENCE, LAB_RESOURCE_URL
  1.9  Verify: JWT flow, stale JWKS, scope enforcement, backward compat

Phase 2  ──────────────────────────────────────────
  2.1  Change DispatchFn from fn pointer to Arc<dyn Fn> (test coercion first)
  2.2  Add UpstreamCatalog (Arc<RwLock<...>>) alongside static ToolRegistry
  2.3  Thread ServiceClients into LabMcpServer (fix per-request client construction)
  2.4  Implement UpstreamPool: connect to configured servers
  2.5  Discovery: parallel list_tools with per-upstream timeouts
  2.6  Schema proxying: capture + serve upstream inputSchema via schema action
  2.7  Proxy dispatch: action+params → call_tool → envelope
  2.8  Circuit breaker: failure counting, unhealthy exclusion, accelerated re-probe
  2.9  Stdio transport: child process management with timeout + kill
  2.10 Config: [[upstream]] sections, credential convention (bearer_token_env)
  2.11 URL validation: reject non-HTTP schemes
  2.12 Verify: HTTP + stdio upstream, discovery, circuit breaker, name collision

Phase 3  ──────────────────────────────────────────
  3.1  Unified catalog: list_tools merges built-in + upstream (simple concat)
  3.2  Error normalization: upstream_error kind + upstream response sanitization
  3.3  Response size cap: configurable limit, uses upstream_error kind
  3.4  Resource proxying: prefixed URIs, per-upstream opt-in
  3.5  Update docs/ERRORS.md with upstream_error kind
  3.6  End-to-end verification
```

## Key Design Decisions

### Lab is a resource server, not an auth server

The noxa plan had `/auth/login` and `/auth/callback` endpoints. These are wrong. Per MCP 2025-11-25, the MCP server validates tokens — it does not issue them. Removed entirely.

### No new transport variant

Instead of adding `StreamableHttp` as a third transport, `/mcp` is mounted inside the existing HTTP router. When you run `lab serve --transport http`, you get both `/v1/*` (REST API) and `/mcp` (MCP streamable HTTP). This matches RMCP.md: "HTTP MCP is mounted inside the Axum application under a dedicated MCP path such as `/mcp`."

### One tool per upstream server

Upstream tools are not exposed as individual lab tools (which would explode the tool count). Each upstream server is one lab tool. Upstream tools become actions. This preserves lab's one-tool-per-service contract.

### Auth middleware owns auth, not rmcp handlers

RMCP.md is explicit: "bearer token validation, OAuth enforcement, and request scoping belong in Axum middleware and app state." The rmcp `auth` feature is enabled for protocol compatibility but enforcement is in the axum layer.

### LabMcpServer moves to mcp/server.rs

Currently embedded in a CLI command file (`serve.rs`). Needs to be importable by both `serve.rs` (for stdio) and `router.rs` (for HTTP/MCP). Clean separation.

### Separate UpstreamCatalog from ToolRegistry

Built-in services stay in the immutable `Arc<ToolRegistry>`. Upstream services live in `Arc<RwLock<UpstreamCatalog>>`. Merged at query time, cached until re-discovery. This avoids destabilizing the existing registry path.

### DispatchFn changes to support closures

`fn` pointer → `Arc<dyn Fn>`. Existing fn pointers coerce automatically. Upstream dispatchers can now capture connection state.

### Credentials in env vars, not config.toml

Lab's convention: secrets in `~/.labby/.env`, preferences in `config.toml`. Upstream `bearer_token_env` names the env var holding the secret — the actual token never appears in config.toml.

## Dependencies

### New workspace dependencies

```toml
jsonwebtoken = "9"           # JWT validation (Phase 1)
```

### Upgraded workspace dependencies

```toml
rmcp = { version = "1.4", features = [...] }  # Phase 0
```

### No new dependencies needed for

- Phase 0 streamable HTTP (rmcp handles it)
- Phase 2/3 MCP client (rmcp handles it)
- axum/tower (already in workspace)
- reqwest for JWKS fetch (already in workspace)

## Risk Notes

1. **rmcp 1.4 API surface** — The `StreamableHttpService` API is new. Pin to exact minor version. If the API changes in 1.5, the factory closure signature may shift. Verify feature names exist before starting Phase 0.

2. **Session lifecycle** — `LocalSessionManager` is in-memory with native TTL-based eviction (rmcp `SessionConfig.keep_alive`). If lab is behind a load balancer with multiple instances, sessions won't roam. This is fine for single-process homelab deployment. Document the constraint.

3. **JWKS availability** — Homelab auth servers (Authelia, Authentik, Keycloak) reboot frequently for updates. Stale-while-revalidate semantics prevent total auth outage during reboots. Log warnings so operators can diagnose.

4. **Upstream server availability** — If an upstream server is down at startup, its tools won't be discovered. The circuit breaker handles runtime failures. Discovery retries periodically — dead upstreams are not permanently lost.

5. **DispatchFn type change** — Changing from `fn` pointer to `Arc<dyn Fn>` is source-compatible but may affect compile time or code size slightly. The existing `dispatch_fn!` macro absorbs the change.
