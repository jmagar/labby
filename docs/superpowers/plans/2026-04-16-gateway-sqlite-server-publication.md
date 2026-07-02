# Gateway SQLite Server Publication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace TOML-backed gateway persistence with a SQLite-backed managed-server control plane that preserves existing upstream stdio support, keeps virtual-server per-surface policy intact, and allows each managed server to publish either into the shared `/mcp` gateway (`proxy`) or as its own isolated HTTP endpoint at `/mcp/servers/<name>` (`secure`) with explicit per-server authorization.

**Architecture:** Keep gateway business logic in `crates/lab/src/dispatch/gateway/`. Persist a canonical managed-server model in a dedicated gateway SQLite database, not in the auth database. Support three source kinds in the store: `upstream_http`, `upstream_stdio`, and `virtual_service`. Publication remains mode-based: `proxy` participates in the shared aggregate `/mcp`; `secure` is HTTP-only and publishes at `/mcp/servers/<name>`. Preserve existing non-MCP surface controls (`cli`, `api`, `mcp`, `webui`) for virtual servers, and add server-scoped authz so a token for one secure server cannot automatically access every secure server.

**Tech Stack:** Rust (`tokio`, `serde`, `rusqlite`, `axum`, existing `dispatch/gateway`, `mcp/server.rs`, auth middleware in `api/router.rs`), Next.js/React in `apps/gateway-admin`, existing gateway CLI/API/MCP adapters, JSON import/export.

---

## Scope Decisions

- Phase 1 includes SQLite persistence for all currently supported managed servers:
  - upstream HTTP
  - upstream stdio
  - virtual servers
- Phase 1 `secure` publication is HTTP-only.
  - `upstream_http` may be `proxy` or `secure`
  - `virtual_service` may be `proxy` or `secure`
  - `upstream_stdio` remains `proxy`-only and must be rejected if configured as `secure`
- Existing virtual-server surface controls remain part of the persisted model. `publish_mode` does not replace `cli/api/mcp/webui` flags.
- `secure` means two things:
  - isolated catalog and route
  - explicit per-server authz enforcement

## File Structure

### SQLite-backed gateway domain

- Create: `crates/lab/src/dispatch/gateway/store.rs`
  - SQLite schema creation, migrations, CRUD, JSON import/export, and legacy TOML import helpers.
- Create: `crates/lab/src/dispatch/gateway/managed_servers.rs`
  - Canonical persisted server model: source kind, publish mode, surface policy, MCP action policy, external import/export identity.
- Create: `crates/lab/src/dispatch/gateway/publication.rs`
  - Runtime publication planning: aggregate publication, isolated publication, route derivation, and validation.
- Modify: `crates/lab/Cargo.toml`
  - Add `rusqlite.workspace = true` to the `lab` crate.
- Modify: `crates/lab/src/config.rs`
  - Add dedicated gateway DB path/config resolution and deprecate TOML as the write source for gateway state.
- Modify: `crates/lab/src/dispatch/gateway/{manager.rs,dispatch.rs,catalog.rs,params.rs,types.rs,view_models.rs,virtual_servers.rs,service_catalog.rs,config.rs}`
  - Move persistence and reconcile logic to SQLite while preserving current virtual-server semantics.

### MCP publication and authorization

- Create: `crates/lab/src/mcp/published_servers.rs`
  - Separate MCP server handler for isolated published servers.
- Modify: `crates/lab/src/mcp/server.rs`
  - Keep aggregate `/mcp` behavior focused on proxy-mode publication only.
- Modify: `crates/lab/src/api/router.rs`
  - Mount isolated MCP routes and enforce server-scoped authz.
- Modify: `crates/lab/src/api/state.rs`
  - Carry publication/runtime state needed by aggregate and isolated MCP handlers.

### Surface adapters

- Modify: `crates/lab/src/cli/gateway.rs`
  - Add unified server CRUD/import/export commands and compatibility shims for legacy upstream-only commands.
- Modify: `crates/lab/src/api/services/gateway.rs`
  - Expose the new actions over `/v1/gateway`.
- Modify: `crates/lab/src/mcp/services/gateway.rs`
  - Expose the same action contract for MCP management.

### Web UI

- Modify: `apps/gateway-admin/lib/types/gateway.ts`
  - Replace gateway-only types with the unified managed-server model.
- Modify: `apps/gateway-admin/lib/api/gateway-client.ts`
  - Support source kind, publish mode, import/export, and secure-route metadata.
- Modify: `apps/gateway-admin/lib/hooks/use-gateways.ts`
  - Support the unified list/detail/update/import/export flows.
- Modify: `apps/gateway-admin/components/gateway/{gateway-form-dialog.tsx,gateway-list-content.tsx,gateway-detail-content.tsx,gateway-table.tsx,tool-exposure-table.tsx,test-result-panel.tsx}`
  - Add source kind, publish mode, route display, authz preview, and import/export controls.

### Docs and tests

- Modify: `docs/GATEWAY.md`
- Modify: `docs/UPSTREAM.md`
- Modify: `docs/CONFIG.md`
- Modify: `docs/CHANGELOG.md`
- Test: `crates/lab/src/dispatch/gateway/{store.rs,manager.rs,dispatch.rs}`
- Test: `crates/lab/src/api/router.rs`
- Test: `crates/lab/src/cli/gateway.rs`
- Test: `apps/gateway-admin/lib/server/gateway-adapter.test.ts`

## Public Action Contract

Before implementation, keep the public contract explicit:

- New unified actions:
  - `gateway.server.list`
  - `gateway.server.get`
  - `gateway.server.test`
  - `gateway.server.create`
  - `gateway.server.update`
  - `gateway.server.delete`
  - `gateway.server.export`
  - `gateway.server.import`
  - `gateway.server.status`
- Keep existing upstream-only actions for compatibility during migration:
  - `gateway.list`
  - `gateway.get`
  - `gateway.test`
  - `gateway.add`
  - `gateway.update`
  - `gateway.remove`
  - `gateway.reload`
- Compatibility behavior:
  - legacy actions operate only on upstream records
  - new `gateway.server.*` actions operate on all source kinds
  - docs mark legacy upstream-only actions as deprecated once the unified UI/API ships

## Identity and Validation Rules

- Internal DB primary key remains internal and is not the cross-instance import key.
- Export/import identity uses a stable external key:
  - `name`
  - `source_kind`
- `route_path` is derived server-side from validated `name` and `publish_mode`; it is not imported.
- Name validation must be tightened for published endpoints:
  - lowercase canonical form
  - URI-safe
  - no reserved segments
  - reject names that normalize to the same route

## Task 1: Add the dedicated gateway SQLite store and canonical managed-server model

**Files:**
- Create: `crates/lab/src/dispatch/gateway/store.rs`
- Create: `crates/lab/src/dispatch/gateway/managed_servers.rs`
- Modify: `crates/lab/Cargo.toml`
- Modify: `crates/lab/src/config.rs`
- Test: `crates/lab/src/dispatch/gateway/store.rs`

- [ ] **Step 1: Write the failing store tests**

Add tests to `crates/lab/src/dispatch/gateway/store.rs`:

```rust
#[tokio::test]
async fn sqlite_store_round_trips_upstream_http_proxy_server() {
    let store = test_store().await;
    let server = ManagedServerRecord::upstream_http(
        "github",
        "https://github.example.com/mcp",
        PublishMode::Proxy,
    );

    store.upsert_server(&server).await.expect("upsert");
    let rows = store.list_servers().await.expect("list");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "github");
    assert_eq!(rows[0].source_kind, SourceKind::UpstreamHttp);
}

#[tokio::test]
async fn sqlite_store_round_trips_upstream_stdio_proxy_server() {
    let store = test_store().await;
    let server = ManagedServerRecord::upstream_stdio(
        "filesystem",
        "npx",
        vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()],
    );

    store.upsert_server(&server).await.expect("upsert");
    let row = store.get_server_by_name("filesystem").await.expect("get").expect("server");

    assert_eq!(row.source_kind, SourceKind::UpstreamStdio);
    assert_eq!(row.publish_mode, PublishMode::Proxy);
}

#[tokio::test]
async fn sqlite_store_round_trips_virtual_secure_server_with_surface_policy() {
    let store = test_store().await;
    let mut server = ManagedServerRecord::virtual_service("plex", PublishMode::Secure);
    server.surface_policy.mcp = true;
    server.surface_policy.api = false;

    store.upsert_server(&server).await.expect("upsert");
    let row = store.get_server_by_name("plex").await.expect("get").expect("server");

    assert_eq!(row.source_kind, SourceKind::VirtualService);
    assert!(row.surface_policy.mcp);
    assert!(!row.surface_policy.api);
}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab sqlite_store_round_trips_upstream_http_proxy_server -- --exact
cargo test -p lab sqlite_store_round_trips_upstream_stdio_proxy_server -- --exact
cargo test -p lab sqlite_store_round_trips_virtual_secure_server_with_surface_policy -- --exact
```

Expected: FAIL because the store and canonical model do not exist.

- [ ] **Step 3: Add the `rusqlite` dependency to `lab`**

Modify `crates/lab/Cargo.toml`:

```toml
[dependencies]
rusqlite.workspace = true
```

- [ ] **Step 4: Add dedicated gateway DB config**

In `crates/lab/src/config.rs`, add explicit gateway DB settings rather than reusing the auth DB:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayPreferences {
    #[serde(default)]
    pub sqlite_path: Option<PathBuf>,
}

pub fn gateway_sqlite_path(config: &LabConfig) -> anyhow::Result<PathBuf> {
    Ok(config
        .gateway
        .sqlite_path
        .clone()
        .unwrap_or_else(|| default_gateway_sqlite_path()))
}
```

Default to a colocated but separate file such as `~/.config/labby/gateway.db`.

- [ ] **Step 5: Add the canonical managed-server domain**

Create `crates/lab/src/dispatch/gateway/managed_servers.rs` with:

```rust
pub enum SourceKind {
    UpstreamHttp,
    UpstreamStdio,
    VirtualService,
}

pub enum PublishMode {
    Proxy,
    Secure,
}

pub struct SurfacePolicy {
    pub cli: bool,
    pub api: bool,
    pub mcp: bool,
    pub webui: bool,
}

pub struct ManagedServerRecord {
    pub db_id: String,
    pub name: String,
    pub source_kind: SourceKind,
    pub publish_mode: PublishMode,
    pub enabled: bool,
    pub surface_policy: SurfacePolicy,
    pub mcp_allowed_actions: Vec<String>,
    pub upstream_http: Option<ManagedUpstreamHttpSource>,
    pub upstream_stdio: Option<ManagedUpstreamStdioSource>,
    pub virtual_service: Option<ManagedVirtualServiceSource>,
}
```

Keep `db_id` internal. Export/import uses `name + source_kind`.

- [ ] **Step 6: Add the SQLite store with explicit schema creation**

Create `crates/lab/src/dispatch/gateway/store.rs` with tables:

```sql
create table if not exists gateway_servers (
  db_id text primary key,
  name text not null,
  source_kind text not null,
  publish_mode text not null,
  enabled integer not null,
  cli_enabled integer not null,
  api_enabled integer not null,
  mcp_enabled integer not null,
  webui_enabled integer not null,
  mcp_allowed_actions_json text not null,
  created_at text not null,
  updated_at text not null,
  unique(name, source_kind)
);

create table if not exists gateway_upstream_http_sources (
  server_id text primary key references gateway_servers(db_id) on delete cascade,
  url text not null,
  bearer_token_env text,
  proxy_resources integer not null,
  expose_tools_json text
);

create table if not exists gateway_upstream_stdio_sources (
  server_id text primary key references gateway_servers(db_id) on delete cascade,
  command text not null,
  args_json text not null,
  proxy_resources integer not null
);

create table if not exists gateway_virtual_service_sources (
  server_id text primary key references gateway_servers(db_id) on delete cascade,
  service_key text not null
);
```

- [ ] **Step 7: Re-run the targeted tests**

Run:

```bash
cargo test -p lab sqlite_store_round_trips_upstream_http_proxy_server -- --exact
cargo test -p lab sqlite_store_round_trips_upstream_stdio_proxy_server -- --exact
cargo test -p lab sqlite_store_round_trips_virtual_secure_server_with_surface_policy -- --exact
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/Cargo.toml crates/lab/src/config.rs crates/lab/src/dispatch/gateway/store.rs crates/lab/src/dispatch/gateway/managed_servers.rs
git commit -m "feat: add dedicated sqlite gateway store"
```

## Task 2: Migrate the manager to SQLite while preserving existing semantics and legacy reload behavior

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/{manager.rs,config.rs,virtual_servers.rs,view_models.rs}`
- Test: `crates/lab/src/dispatch/gateway/manager.rs`

- [ ] **Step 1: Write the failing manager tests**

Add tests to `crates/lab/src/dispatch/gateway/manager.rs`:

```rust
#[tokio::test]
async fn gateway_manager_lists_upstream_and_virtual_servers_from_sqlite() {
    let manager = test_manager_with_sqlite().await;
    manager
        .store()
        .upsert_server(&ManagedServerRecord::virtual_service("plex", PublishMode::Secure))
        .await
        .expect("seed");
    manager
        .store()
        .upsert_server(&ManagedServerRecord::upstream_stdio("filesystem", "npx", vec![]))
        .await
        .expect("seed");

    let list = manager.list_servers().await.expect("list");
    assert!(list.iter().any(|server| server.name == "plex"));
    assert!(list.iter().any(|server| server.name == "filesystem"));
}

#[tokio::test]
async fn gateway_reload_migrates_legacy_toml_once_then_warns_on_future_manual_edits() {
    let manager = seeded_manager_with_legacy_toml().await;

    manager.reload().await.expect("reload");
    let warnings = manager.status().await.expect("status").warnings;

    assert!(warnings.iter().any(|warning| warning.code == "legacy_gateway_config_present"));
}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab gateway_manager_lists_upstream_and_virtual_servers_from_sqlite -- --exact
cargo test -p lab gateway_reload_migrates_legacy_toml_once_then_warns_on_future_manual_edits -- --exact
```

Expected: FAIL because `GatewayManager` still treats TOML as the primary store.

- [ ] **Step 3: Refactor `GatewayManager` to use the store**

Update `crates/lab/src/dispatch/gateway/manager.rs`:
- replace persisted CRUD over `LabConfig.upstream` and `LabConfig.virtual_servers` with store-backed CRUD
- keep `.env` writes only for canonical service config
- preserve current virtual-server surface gating semantics
- expose store-backed `list_servers`, `get_server`, `test_server`, `create_server`, `update_server`, `delete_server`

- [ ] **Step 4: Add a real legacy migration path**

In `crates/lab/src/dispatch/gateway/config.rs`, keep legacy readers only:
- read `[[upstream]]`
- read legacy `virtual_servers`
- import into SQLite if the store is empty
- mark imported upstreams as:
  - HTTP -> `UpstreamHttp`
  - stdio -> `UpstreamStdio`
  - `publish_mode = proxy`

Do not keep TOML as a write path after import.

- [ ] **Step 5: Redefine `gateway.reload` explicitly**

`gateway.reload` must:
- reload bearer-token env values for persisted records
- rebuild runtime state from the SQLite store
- perform one-time legacy TOML import only when the DB is empty
- emit warnings when legacy gateway TOML remains present after migration

Do not silently continue pretending TOML remains authoritative.

- [ ] **Step 6: Re-run the targeted tests**

Run:

```bash
cargo test -p lab gateway_manager_lists_upstream_and_virtual_servers_from_sqlite -- --exact
cargo test -p lab gateway_reload_migrates_legacy_toml_once_then_warns_on_future_manual_edits -- --exact
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/gateway/manager.rs crates/lab/src/dispatch/gateway/config.rs crates/lab/src/dispatch/gateway/virtual_servers.rs crates/lab/src/dispatch/gateway/view_models.rs
git commit -m "refactor: migrate gateway manager to sqlite persistence"
```

## Task 3: Define aggregate and isolated publication planning with route-safe validation

**Files:**
- Create: `crates/lab/src/dispatch/gateway/publication.rs`
- Modify: `crates/lab/src/dispatch/gateway/{manager.rs,managed_servers.rs}`
- Test: `crates/lab/src/dispatch/gateway/publication.rs`

- [ ] **Step 1: Write the failing publication tests**

Add tests to `crates/lab/src/dispatch/gateway/publication.rs`:

```rust
#[test]
fn secure_publication_rejects_stdio_sources() {
    let server = ManagedServerRecord::upstream_stdio("filesystem", "npx", vec![]);
    let err = validate_publication(&server.with_publish_mode(PublishMode::Secure)).expect_err("validation");
    assert!(err.to_string().contains("secure publication requires http-capable source"));
}

#[test]
fn route_is_derived_from_canonicalized_name() {
    let server = ManagedServerRecord::virtual_service("Plex Main", PublishMode::Secure);
    let route = derive_secure_route(&server).expect("route");
    assert_eq!(route, "/mcp/servers/plex-main");
}

#[test]
fn names_that_canonicalize_to_same_route_are_rejected() {
    assert!(routes_conflict("Plex Main", "plex-main"));
}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab secure_publication_rejects_stdio_sources -- --exact
cargo test -p lab route_is_derived_from_canonicalized_name -- --exact
cargo test -p lab names_that_canonicalize_to_same_route_are_rejected -- --exact
```

Expected: FAIL because publication planning does not exist.

- [ ] **Step 3: Add route derivation and publication validation**

In `crates/lab/src/dispatch/gateway/publication.rs`, add:

```rust
pub struct AggregatePublicationPlan {
    pub proxy_servers: Vec<ManagedServerRecord>,
}

pub struct IsolatedPublicationPlan {
    pub server: ManagedServerRecord,
    pub route_path: String,
    pub required_scope: String,
}
```

Rules:
- `PublishMode::Proxy` -> aggregate only
- `PublishMode::Secure` -> isolated only
- `UpstreamStdio + Secure` -> invalid
- derive route from canonicalized validated name
- derive required auth scope from canonical name, for example `mcp:server:plex-main`

- [ ] **Step 4: Re-run the targeted tests**

Run:

```bash
cargo test -p lab secure_publication_rejects_stdio_sources -- --exact
cargo test -p lab route_is_derived_from_canonicalized_name -- --exact
cargo test -p lab names_that_canonicalize_to_same_route_are_rejected -- --exact
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/gateway/publication.rs crates/lab/src/dispatch/gateway/manager.rs crates/lab/src/dispatch/gateway/managed_servers.rs
git commit -m "feat: add gateway publication planning and route validation"
```

## Task 4: Add a separate isolated MCP server handler with per-server notifier/session ownership

**Files:**
- Create: `crates/lab/src/mcp/published_servers.rs`
- Modify: `crates/lab/src/mcp/server.rs`
- Modify: `crates/lab/src/api/state.rs`
- Test: `crates/lab/src/mcp/published_servers.rs`

- [ ] **Step 1: Write the failing isolated-handler tests**

Add tests to `crates/lab/src/mcp/published_servers.rs`:

```rust
#[tokio::test]
async fn secure_virtual_server_lists_only_its_tool() {
    let handler = test_isolated_virtual_handler("plex").await;
    let tools = handler.list_tools_for_test().await;

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_ref(), "plex");
}

#[tokio::test]
async fn secure_virtual_server_exposes_no_lab_catalog_resource() {
    let handler = test_isolated_virtual_handler("plex").await;
    let resources = handler.list_resources_for_test().await;

    assert!(!resources.iter().any(|resource| resource.uri == "lab://catalog"));
}

#[tokio::test]
async fn isolated_server_has_its_own_peer_notifier() {
    let runtime = test_isolated_runtime("plex").await;
    assert_eq!(runtime.server_name(), "plex");
}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab secure_virtual_server_lists_only_its_tool -- --exact
cargo test -p lab secure_virtual_server_exposes_no_lab_catalog_resource -- --exact
cargo test -p lab isolated_server_has_its_own_peer_notifier -- --exact
```

Expected: FAIL because isolated handlers do not exist.

- [ ] **Step 3: Implement a separate isolated handler**

Create `crates/lab/src/mcp/published_servers.rs` with a dedicated `ServerHandler` for isolated publications instead of trying to lightly branch `LabMcpServer`.

Design:
- aggregate `/mcp` keeps using `LabMcpServer`
- isolated `/mcp/servers/<name>` uses `PublishedServerHandler`
- isolated handler owns:
  - its own peer list
  - its own list-changed notifier
  - only the selected server’s tools/resources/prompts

- [ ] **Step 4: Keep aggregate behavior unchanged**

In `crates/lab/src/mcp/server.rs`, remove responsibility for isolated secure publication. Its job stays:
- built-in services visible on aggregate MCP
- proxy-mode upstream publication
- proxy-mode virtual-service publication

- [ ] **Step 5: Re-run the targeted tests**

Run:

```bash
cargo test -p lab secure_virtual_server_lists_only_its_tool -- --exact
cargo test -p lab secure_virtual_server_exposes_no_lab_catalog_resource -- --exact
cargo test -p lab isolated_server_has_its_own_peer_notifier -- --exact
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/mcp/published_servers.rs crates/lab/src/mcp/server.rs crates/lab/src/api/state.rs
git commit -m "feat: add isolated published server handler"
```

## Task 5: Mount `/mcp/servers/<name>` and enforce server-scoped authorization

**Files:**
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/mcp/published_servers.rs`
- Test: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Write the failing authz route tests**

Add tests to `crates/lab/src/api/router.rs`:

```rust
#[tokio::test]
async fn secure_server_route_requires_matching_server_scope() {
    let app = test_app_with_secure_server("plex-main").await;
    let response = app
        .oneshot(authenticated_request_without_scope("/mcp/servers/plex-main"))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn secure_server_route_accepts_matching_server_scope() {
    let app = test_app_with_secure_server("plex-main").await;
    let response = app
        .oneshot(authenticated_request_with_scope(
            "/mcp/servers/plex-main",
            "mcp:server:plex-main",
        ))
        .await
        .expect("response");

    assert_ne!(response.status(), StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab secure_server_route_requires_matching_server_scope -- --exact
cargo test -p lab secure_server_route_accepts_matching_server_scope -- --exact
```

Expected: FAIL because current auth middleware does not authorize by server.

- [ ] **Step 3: Mount the isolated route**

In `crates/lab/src/api/router.rs`, add:

```rust
.route("/mcp/servers/:name", any(handle_isolated_mcp))
```

- [ ] **Step 4: Enforce server-scoped authz**

Use the existing authenticated `AuthContext`, but add route-level authorization:
- derive required scope from the publication plan
- reject requests without matching scope
- return `403` for authenticated but unauthorized callers

Do not describe this route as secure unless the scope check exists.

- [ ] **Step 5: Re-run the targeted tests**

Run:

```bash
cargo test -p lab secure_server_route_requires_matching_server_scope -- --exact
cargo test -p lab secure_server_route_accepts_matching_server_scope -- --exact
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/api/router.rs crates/lab/src/mcp/published_servers.rs
git commit -m "feat: enforce server-scoped authz for isolated mcp routes"
```

## Task 6: Add unified server CRUD and JSON import/export with safe external identity

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/{catalog.rs,params.rs,dispatch.rs,manager.rs,managed_servers.rs,store.rs,types.rs}`
- Modify: `crates/lab/src/cli/gateway.rs`
- Modify: `crates/lab/src/api/services/gateway.rs`
- Modify: `crates/lab/src/mcp/services/gateway.rs`
- Test: `crates/lab/src/dispatch/gateway/dispatch.rs`
- Test: `crates/lab/src/cli/gateway.rs`

- [ ] **Step 1: Write the failing dispatch tests**

Add tests to `crates/lab/src/dispatch/gateway/dispatch.rs`:

```rust
#[tokio::test]
async fn gateway_server_export_returns_selected_servers_as_json_bundle() {
    let manager = seeded_manager_with_proxy_and_secure_servers().await;
    let value = dispatch_with_manager(
        &manager,
        "gateway.server.export",
        json!({ "names": ["github", "plex"] }),
    )
    .await
    .expect("export");

    assert_eq!(value["servers"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn gateway_server_import_validate_reports_name_source_conflicts_without_writing() {
    let manager = seeded_manager_with_proxy_server("github").await;
    let value = dispatch_with_manager(
        &manager,
        "gateway.server.import",
        json!({
            "mode": "validate",
            "bundle": {
                "servers": [{ "name": "github", "source_kind": "upstream_http", "publish_mode": "proxy" }]
            }
        }),
    )
    .await
    .expect("validate");

    assert_eq!(value["conflicts"].as_array().unwrap().len(), 1);
}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab gateway_server_export_returns_selected_servers_as_json_bundle -- --exact
cargo test -p lab gateway_server_import_validate_reports_name_source_conflicts_without_writing -- --exact
```

Expected: FAIL because the new unified actions do not exist.

- [ ] **Step 3: Add unified export/import params and catalog entries**

In `params.rs`, add:

```rust
pub struct GatewayServerExportParams {
    pub name: Option<String>,
    pub names: Option<Vec<String>>,
    pub all: Option<bool>,
}

pub struct GatewayServerImportParams {
    pub mode: String,
    pub bundle: ManagedServerExportBundle,
}
```

In `catalog.rs`, add:
- `gateway.server.export`
- `gateway.server.import`
- `gateway.server.create`
- `gateway.server.update`
- `gateway.server.delete`

- [ ] **Step 4: Use safe export/import identity**

Use an export bundle shape:

```rust
pub struct ManagedServerExportBundle {
    pub version: u32,
    pub servers: Vec<ManagedServerExportRecord>,
}
```

`ManagedServerExportRecord` must include:
- `name`
- `source_kind`
- `publish_mode`
- `enabled`
- `surface_policy`
- source payload

Do not export internal DB IDs as the primary import key.
Do not import `route_path`; derive it server-side.

- [ ] **Step 5: Wire adapters**

Update:
- `crates/lab/src/cli/gateway.rs`
- `crates/lab/src/api/services/gateway.rs`
- `crates/lab/src/mcp/services/gateway.rs`

CLI examples:

```bash
lab gateway server export --name plex
lab gateway server export --all
lab gateway server import --mode validate --file servers.json
lab gateway server import --mode upsert --file servers.json
```

- [ ] **Step 6: Re-run the targeted tests**

Run:

```bash
cargo test -p lab gateway_server_export_returns_selected_servers_as_json_bundle -- --exact
cargo test -p lab gateway_server_import_validate_reports_name_source_conflicts_without_writing -- --exact
cargo test -p lab gateway_cli_parser_accepts_expected_commands -- --exact
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/gateway/catalog.rs crates/lab/src/dispatch/gateway/params.rs crates/lab/src/dispatch/gateway/dispatch.rs crates/lab/src/dispatch/gateway/manager.rs crates/lab/src/dispatch/gateway/managed_servers.rs crates/lab/src/dispatch/gateway/store.rs crates/lab/src/cli/gateway.rs crates/lab/src/api/services/gateway.rs crates/lab/src/mcp/services/gateway.rs
git commit -m "feat: add unified gateway server import and export"
```

## Task 7: Update server views and gateway-admin for source kind, publish mode, and route/authz metadata

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/view_models.rs`
- Modify: `apps/gateway-admin/lib/types/gateway.ts`
- Modify: `apps/gateway-admin/lib/api/gateway-client.ts`
- Modify: `apps/gateway-admin/lib/hooks/use-gateways.ts`
- Modify: `apps/gateway-admin/components/gateway/{gateway-form-dialog.tsx,gateway-list-content.tsx,gateway-detail-content.tsx,gateway-table.tsx,tool-exposure-table.tsx,test-result-panel.tsx}`
- Test: `apps/gateway-admin/lib/server/gateway-adapter.test.ts`

- [ ] **Step 1: Write the failing UI adapter tests**

Add tests to `apps/gateway-admin/lib/server/gateway-adapter.test.ts`:

```ts
it("maps secure servers with derived isolated route metadata", async () => {
  const view = adaptGatewayServer({
    name: "plex-main",
    publish_mode: "secure",
    source_kind: "virtual_service",
    secure_scope: "mcp:server:plex-main",
  });

  expect(view.publishMode).toBe("secure");
  expect(view.routePath).toBe("/mcp/servers/plex-main");
  expect(view.requiredScope).toBe("mcp:server:plex-main");
});

it("preserves non-mcp surface policy from the backend model", async () => {
  const view = adaptGatewayServer({
    name: "plex-main",
    surface_policy: { cli: true, api: false, mcp: true, webui: false },
  });

  expect(view.surfacePolicy.api).toBe(false);
});
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
pnpm --dir apps/gateway-admin test -- gateway-adapter
```

Expected: FAIL because the adapter does not model publish/authz metadata.

- [ ] **Step 3: Extend the server view model**

In `view_models.rs` and `apps/gateway-admin/lib/types/gateway.ts`, add:
- `source_kind`
- `publish_mode`
- derived `route_path`
- derived `required_scope`
- `surface_policy`

- [ ] **Step 4: Update the UI flows**

Update:
- create/edit forms to choose `proxy` vs `secure`
- display `proxy` vs `secure` badges
- show derived secure route and required scope for secure servers
- preserve non-MCP surface toggles
- expose import/export actions

- [ ] **Step 5: Re-run the targeted tests**

Run:

```bash
pnpm --dir apps/gateway-admin test -- gateway-adapter
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/gateway/view_models.rs apps/gateway-admin/lib/types/gateway.ts apps/gateway-admin/lib/api/gateway-client.ts apps/gateway-admin/lib/hooks/use-gateways.ts apps/gateway-admin/components/gateway
git commit -m "feat: add gateway publication and authz metadata to admin ui"
```

## Task 8: Update docs and expand verification for migration and isolated runtime edge cases

**Files:**
- Modify: `docs/GATEWAY.md`
- Modify: `docs/UPSTREAM.md`
- Modify: `docs/CONFIG.md`
- Modify: `docs/CHANGELOG.md`

- [ ] **Step 1: Document the new persistence and publication model**

Update:
- `docs/GATEWAY.md` with:
  - dedicated gateway SQLite store
  - unified managed-server model
  - `proxy` vs `secure`
  - server-scoped authz for secure endpoints
  - import/export
- `docs/UPSTREAM.md` with:
  - HTTP and stdio persistence in SQLite
  - `secure` limited to HTTP-capable sources
  - stdio remaining `proxy`-only
- `docs/CONFIG.md` with:
  - new gateway DB path
  - legacy TOML import behavior

- [ ] **Step 2: Document migration and reload behavior**

Document:
- startup imports legacy TOML only when the gateway DB is empty
- after migration, `gateway.reload` rebuilds from SQLite and env, not TOML
- legacy TOML presence after migration produces warnings
- manual `config.toml` edits no longer mutate the persisted gateway state

- [ ] **Step 3: Add missing Rust verification targets**

Run:

```bash
cargo test -p lab legacy_gateway_toml_import_is_idempotent -- --exact
cargo test -p lab secure_server_route_requires_matching_server_scope -- --exact
cargo test -p lab secure_virtual_server_exposes_no_lab_catalog_resource -- --exact
cargo test -p lab secure_publication_rejects_stdio_sources -- --exact
cargo test --workspace --all-features --no-fail-fast
cargo check --workspace --all-features
```

Expected: PASS.

- [ ] **Step 4: Run Web UI verification**

Run:

```bash
pnpm --dir apps/gateway-admin test
pnpm --dir apps/gateway-admin build
```

Expected: PASS.

- [ ] **Step 5: Run focused manual verification**

Verify:
1. migrate legacy TOML upstream HTTP, upstream stdio, and virtual servers into SQLite
2. confirm `gateway.reload` refreshes env-backed bearer-token names from SQLite-backed records
3. create upstream HTTP server in `proxy` mode and confirm it appears on `/mcp`
4. create upstream HTTP server in `secure` mode and confirm it appears only on `/mcp/servers/<name>`
5. create virtual server in `secure` mode and confirm it exposes exactly one tool
6. confirm upstream stdio creation in `secure` mode is rejected
7. confirm a token without `mcp:server:<name>` scope gets `403` on that isolated route
8. export one server, many servers, and all servers
9. import with `validate`, `create`, and `upsert`
10. restart `lab` and confirm all server state restores from the gateway DB

- [ ] **Step 6: Commit**

```bash
git add docs/GATEWAY.md docs/UPSTREAM.md docs/CONFIG.md docs/CHANGELOG.md
git commit -m "docs: describe sqlite-backed managed server publication"
```

## Notes for the Implementer

- Use `@test-driven-development` on every backend and frontend behavior change. Do not write new CRUD, publication, or authz code before the failing tests exist.
- Keep adapter logic thin. CLI, MCP, API, and Web UI should call into the shared dispatch contract instead of re-encoding business rules.
- Do not reuse the auth SQLite file for gateway state.
- Do not flatten away existing `cli/api/mcp/webui` surface policy just because publication is now mode-based.
- Do not describe isolated routes as secure unless server-scoped authz is implemented and tested.
- Do not expose shared `lab://catalog` resources on `/mcp/servers/<name>`.
