//! `GatewayManager` — the shared orchestration object behind every gateway
//! surface (CLI, MCP, HTTP).
//!
//! This file owns the struct definition and its fields; all method bodies live
//! in the `manager/` child modules as additional `impl GatewayManager` blocks:
//!
//! | Module | Responsibilities |
//! |--------|-----------------|
//! | `core` | `new()`, `with_*` builders, `from_config` factory, accessors |
//! | `config_ops` | upstream add/update/remove, service env config, code-mode config |
//! | `pool_lifecycle` | reload + swap-and-drain, `GatewayCatalogSnapshot`/`diff_catalogs` |
//! | `code_mode_runtime` | catalog refresh, render cache, runtime readiness |
//! | `code_mode_resolve` | `resolve_*_tool`, `ToolExecuteSelector` |
//! | `persist` | env-file path + bearer-token persistence |
//! | `imports` | discovery import orchestration + tombstones |
//! | `import_matchers` | pure import/tombstone matching helpers |
//! | `virtual_servers` | virtual-server CRUD + quarantine restore |
//! | `protected_routes` | protected MCP route CRUD + live resolver |
//! | `oauth_resources` | upstream OAuth manager/cache reconciliation |
//! | `views` | `list`/`get`/`status`/`test` and discovery inspection views |

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::{Mutex, RwLock};
use tokio::time::Instant;

use crate::config::LabConfig;
use crate::dispatch::clients::SharedServiceClients;
use crate::dispatch::error::ToolError;
use crate::dispatch::upstream::pool::InProcessConnector;
use crate::oauth::upstream::cache::OauthClientCache;
use crate::oauth::upstream::encryption::EncryptionKey;
use crate::oauth::upstream::manager::UpstreamOauthManager;
use crate::registry::ToolRegistry;

use super::code_mode::CodeModeHistory;
use super::config::write_gateway_config;
use super::protected_routes::ProtectedRouteIndex;
pub use super::runtime::GatewayRuntimeHandle;
use super::types::CatalogChangeNotifier;

mod code_mode_resolve;
mod code_mode_runtime;
mod config_ops;
mod core;
mod import_matchers;
mod imports;
mod oauth_resources;
mod persist;
mod pool_lifecycle;
mod protected_routes;
#[cfg(test)]
mod tests;
mod views;
mod virtual_servers;

// `BatchAddOutcome`, `GatewayCatalogSnapshot`, and `diff_catalogs` keep the
// monolith's public `manager::` paths; they currently have no callers outside
// the manager tree, so the re-exports are allowed to be unused in non-test
// builds (the test suite imports them through these paths).
pub(crate) use self::code_mode_resolve::CallbackToolLookup;
#[allow(unused_imports)]
pub use self::config_ops::BatchAddOutcome;
pub use self::core::{GatewayManagerConfig, GatewayOauthConfig};
pub use self::import_matchers::ImportTombstoneSelector;
pub(crate) use self::import_matchers::{discovered_is_tombstoned, partition_discovered_for_import};
#[allow(unused_imports)]
pub use self::pool_lifecycle::{GatewayCatalogSnapshot, diff_catalogs};

#[derive(Clone)]
pub struct GatewayManager {
    pub(super) path: PathBuf,
    /// Override for the `.env` file path used by config persistence helpers.
    ///
    /// `None` in production — `env_path()` derives the canonical `~/.lab/.env`
    /// location.  Set by the `with_env_path` builder in tests so each test
    /// can write beside its temp `config.toml` without touching the developer's
    /// home directory.
    pub(super) env_path_override: Option<PathBuf>,
    pub(super) runtime: GatewayRuntimeHandle,
    pub(super) config: Arc<RwLock<LabConfig>>,
    pub(super) config_mutation: Arc<Mutex<()>>,
    lazy_pool_init: Arc<Mutex<()>>,
    service_clients: Option<SharedServiceClients>,
    notifier: Option<CatalogChangeNotifier>,
    pub(super) oauth_client_cache: Option<OauthClientCache>,
    pub(super) upstream_oauth_managers: Option<Arc<dashmap::DashMap<String, UpstreamOauthManager>>>,
    builtin_service_registry: Arc<ArcSwap<ToolRegistry>>,
    pub(super) oauth_sqlite: Option<lab_auth::sqlite::SqliteStore>,
    pub(super) oauth_key: Option<EncryptionKey>,
    pub(super) oauth_redirect_uri: Option<Arc<String>>,
    protected_route_index: Arc<RwLock<ProtectedRouteIndex>>,
    code_mode_history: Arc<Mutex<CodeModeHistory>>,
    /// Optional connector for in-process (built-in) service peers.
    /// Propagated to each pool the manager creates so built-in services are
    /// reachable without an external HTTP/stdio connection.
    in_process_connector: Option<InProcessConnector>,
    /// Wall-clock TTL guard for `refresh_code_mode_catalog`. Tracks the
    /// last time a full reprobe completed; back-to-back calls within the
    /// freshness window skip the reprobe and return immediately.
    pub(super) code_mode_refresh_deadline: Arc<Mutex<Option<Instant>>>,
    /// Single-flight guard: only one concurrent `refresh_code_mode_catalog`
    /// runs at a time. Subsequent callers that arrive while a refresh is in
    /// progress wait for it to finish rather than spawning a second reprobe.
    pub(super) code_mode_refresh_inflight: Arc<Mutex<()>>,
    /// Cached rendered Code Mode search catalog, keyed by a fingerprint of
    /// the live healthy tool list. Avoids regenerating `CodeModeCatalogEntry`
    /// structs (including TS `.signature`/`.dts` via `generate_tool_types`),
    /// the serialized JSON blob, and the JS proxy string on every search when
    /// the upstream catalog has not changed between calls.
    pub(super) code_mode_catalog_render_cache:
        Arc<Mutex<Option<crate::dispatch::gateway::code_mode::CatalogRenderCache>>>,
}

impl GatewayManager {
    pub(super) async fn persist_config(&self, cfg: LabConfig) -> Result<(), ToolError> {
        let path = self.path.clone();
        tracing::info!(
            action = "gateway.config.write",
            phase = "start",
            upstream_count = cfg.upstream.len(),
            virtual_server_count = cfg.virtual_servers.len(),
            "gateway reconcile"
        );
        // C1(a): hand the owned `cfg` to the blocking writer and have it return
        // the value back, instead of deep-cloning `LabConfig` a second time just
        // to keep an in-memory copy for the post-write swap. The previous
        // `cfg.clone()` here doubled the per-mutation deep-clone cost.
        //
        // The broader `RwLock<LabConfig>` → `RwLock<Arc<LabConfig>>` change
        // suggested for this finding is deliberately NOT done: `self.config` is
        // read via guard-deref field access in ~10 sibling modules
        // (`runtime.rs`, `oauth_lifecycle.rs`, `virtual_servers.rs`,
        // `code_mode_runtime.rs`, `views.rs`, `imports.rs`, …) that this task is
        // scoped out of, so switching the inner type would ripple far beyond the
        // five owned files. Killing the redundant clone is the safe partial win.
        let cfg = tokio::task::spawn_blocking(move || -> Result<LabConfig, ToolError> {
            write_gateway_config(&path, &cfg)?;
            Ok(cfg)
        })
        .await
        .map_err(|e| ToolError::internal_message(format!("config write task failed: {e}")))??;
        *self.protected_route_index.write().await =
            ProtectedRouteIndex::from_routes(&cfg.protected_mcp_routes);
        *self.config.write().await = cfg;
        tracing::info!(
            action = "gateway.config.write",
            phase = "finish",
            "gateway reconcile"
        );
        Ok(())
    }
}
