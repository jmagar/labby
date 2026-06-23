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

use lab_auth::upstream::cache::OauthClientCache;
use lab_auth::upstream::encryption::EncryptionKey;
use lab_auth::upstream::manager::UpstreamOauthManager;
use lab_runtime::error::ToolError;
use lab_runtime::gateway_config::GatewayConfig;

use crate::upstream::pool::InProcessConnector;

use super::code_mode::{CodeModeHistory, CodeModeSourceStore};
use super::config_store::GatewayConfigStore;
use super::protected_routes::ProtectedRouteIndex;
use super::service_registry::GatewayServiceRegistry;
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
    /// Host-owned persistence + environment seam.
    ///
    /// Owns `config.toml` rendering (with foreign-key preservation), the `.env`
    /// credential file helpers, the process-wide Code Mode flag, and public-URL
    /// resolution — all of which depend on the host's full `LabConfig` and are
    /// shared with non-gateway Labby code, so they cannot live in `lab-gateway`.
    pub(super) store: Arc<dyn GatewayConfigStore>,
    pub(super) runtime: GatewayRuntimeHandle,
    pub(super) config: Arc<RwLock<GatewayConfig>>,
    pub(super) config_mutation: Arc<Mutex<()>>,
    lazy_pool_init: Arc<Mutex<()>>,
    notifier: Option<CatalogChangeNotifier>,
    pub(super) oauth_client_cache: Option<OauthClientCache>,
    pub(super) upstream_oauth_managers: Option<Arc<dashmap::DashMap<String, UpstreamOauthManager>>>,
    builtin_service_registry: Arc<ArcSwap<Arc<dyn GatewayServiceRegistry>>>,
    pub(super) oauth_sqlite: Option<lab_auth::sqlite::SqliteStore>,
    pub(super) oauth_key: Option<EncryptionKey>,
    pub(super) oauth_redirect_uri: Option<Arc<String>>,
    protected_route_index: Arc<RwLock<ProtectedRouteIndex>>,
    code_mode_history: Arc<Mutex<CodeModeHistory>>,
    code_mode_source_store: Arc<Mutex<CodeModeSourceStore>>,
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
    /// Cached rendered Code Mode discovery catalog, keyed by a fingerprint of
    /// the live healthy tool list. Avoids regenerating `CodeModeCatalogEntry`
    /// structs (including TS `.signature`/`.dts` via `generate_tool_types`),
    /// the serialized JSON blob, and the JS proxy string on every lookup when
    /// the upstream catalog has not changed between calls.
    pub(super) code_mode_catalog_render_cache:
        Arc<Mutex<Option<crate::gateway::code_mode::CatalogRenderCache>>>,
    /// Cached snippet metadata for Code Mode discovery. Snippet executable
    /// source is never stored here; `codemode.run()` resolves source lazily.
    pub(super) code_mode_snippet_metadata_cache:
        Arc<Mutex<Option<crate::gateway::code_mode::SnippetMetadataCache>>>,
    /// Shared, long-lived warm-runner pool for Code Mode (Perf H1). Pools the
    /// runner OS process across executions (fresh `javy::Runtime` per run) to
    /// amortize fork/startup. Wrapped in `Arc` so the `Clone` manager shares one
    /// pool; configured from the environment at construction (kill switch:
    /// `LAB_CODE_MODE_POOL_SIZE=0` → spawn-per-execution fallback).
    pub(super) code_mode_runner_pool: Arc<crate::gateway::code_mode::RunnerPool>,
}

impl GatewayManager {
    pub(super) async fn persist_config(&self, cfg: GatewayConfig) -> Result<(), ToolError> {
        tracing::info!(
            action = "gateway.config.write",
            phase = "start",
            upstream_count = cfg.upstream.len(),
            virtual_server_count = cfg.virtual_servers.len(),
            "gateway reconcile"
        );
        // Persistence (TOML render with foreign-key preservation + atomic write)
        // is owned by the host through the `GatewayConfigStore` seam, reusing the
        // existing `write_gateway_config`/`render_gateway_config` toml_edit logic
        // verbatim. The manager keeps the in-memory `GatewayConfig` authoritative
        // for the gateway-owned sections and swaps it in after a successful write.
        self.store.persist(&cfg)?;
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
