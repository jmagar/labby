//! Pool bootstrap and reload: swap-and-drain reconciliation, catalog snapshot
//! diffing, and quarantine of virtual servers with unregistered services.

use std::collections::BTreeSet;
use std::sync::Arc;

use futures::StreamExt as _;

use tokio::time::Instant;

use crate::config::LabConfig;
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::config::load_gateway_config;
use crate::dispatch::gateway::protected_routes::ProtectedRouteIndex;
use crate::dispatch::gateway::runtime::runtime_origin_tag;
use crate::dispatch::gateway::types::GatewayCatalogDiff;
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;
use crate::registry::ToolRegistry;

use super::GatewayManager;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatewayCatalogSnapshot {
    pub tools: BTreeSet<String>,
    pub resources: BTreeSet<String>,
    pub prompts: BTreeSet<String>,
}

pub fn diff_catalogs(
    before: &GatewayCatalogSnapshot,
    after: &GatewayCatalogSnapshot,
) -> GatewayCatalogDiff {
    GatewayCatalogDiff {
        tools_changed: before.tools != after.tools,
        resources_changed: before.resources != after.resources,
        prompts_changed: before.prompts != after.prompts,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct VirtualServerMigration {
    quarantined: Vec<String>,
}

impl VirtualServerMigration {
    pub(super) fn changed(&self) -> bool {
        !self.quarantined.is_empty()
    }
}

impl GatewayManager {
    pub async fn reload_with_origin(
        &self,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayCatalogDiff, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        self.reload_with_origin_unlocked(origin, owner).await
    }

    pub(super) async fn reload_with_origin_unlocked(
        &self,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayCatalogDiff, ToolError> {
        let started = Instant::now();
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "catalog.refresh.start",
            phase = "config.load.start",
            "gateway reconcile"
        );
        let path = self.path.clone();
        let cfg = tokio::task::spawn_blocking(move || load_gateway_config(&path))
            .await
            .map_err(|e| ToolError::internal_message(format!("config read task failed: {e}")))??;
        let registry = self.builtin_service_registry();
        let (cfg, migration) = quarantine_unregistered_virtual_servers(cfg, &registry);
        if migration.changed() {
            tracing::warn!(
                action = "gateway.config.migrate",
                stale_virtual_server_count = migration.quarantined.len(),
                stale_virtual_servers = ?migration.quarantined,
                "quarantined virtual servers with unregistered backing services"
            );
            self.persist_config(cfg.clone()).await?;
        }
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "catalog.config.loaded",
            phase = "config.load.finish",
            upstream_count = cfg.upstream.len(),
            virtual_server_count = cfg.virtual_servers.len(),
            quarantined_virtual_server_count = cfg.quarantined_virtual_servers.len(),
            "gateway reconcile"
        );
        self.reconcile_upstream_oauth_managers(&cfg);

        // C1(b): diff-aware reload. The full path below tears down the live
        // upstream pool and builds a fresh one, which drops every lazily-spawned
        // stdio child and forces a re-handshake on next use — even when the
        // logical change (e.g. a `protected_mcp_routes` edit, or a no-op reload)
        // left every upstream untouched. When the upstream set, gateway spawn
        // prefs, and code-mode settings are byte-identical between the live
        // in-memory config and the freshly-loaded one, keep the existing pool and
        // only reconcile the cheap in-memory state (protected routes + config
        // swap). Per-upstream *partial* diffing (reconnect only the changed
        // upstreams while preserving the rest) would require carrying live
        // connections across the build-first/swap/drain boundary in
        // `pool/lifecycle.rs` + `pool/ensure.rs`, which are outside this change's
        // scope — so this implements the all-or-nothing safe subset and leaves
        // the partial-reconnect gap documented here.
        let pool_inputs_unchanged = {
            let current = self.config.read().await;
            upstream_runtime_fingerprint(&current) == upstream_runtime_fingerprint(&cfg)
        };
        let existing_pool = self.runtime.current_pool().await;
        if pool_inputs_unchanged && existing_pool.is_some() {
            *self.protected_route_index.write().await =
                ProtectedRouteIndex::from_routes(&cfg.protected_mcp_routes);
            let current_pool = existing_pool;
            *self.config.write().await = cfg;
            let current_cfg = self.config.read().await.clone();
            self.reconcile_runtime_state(&current_cfg, current_pool.as_deref())
                .await?;
            let diff = GatewayCatalogDiff::default();
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.reload",
                event = "catalog.refresh.finish",
                phase = "finish",
                pool_rebuild_skipped = true,
                elapsed_ms = started.elapsed().as_millis(),
                "gateway reconcile (upstream runtime inputs unchanged; live pool preserved)"
            );
            return Ok(diff);
        }

        let old_pool = existing_pool;
        let before = snapshot_from_pool(old_pool.clone()).await;
        let old_pool_present = old_pool.is_some();
        if let Some(old_pool) = old_pool {
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.reload",
                event = "old_pool.drain.start",
                phase = "pool.drain.start",
                "gateway old upstream pool drain start"
            );
            self.runtime.swap(None).await;
            old_pool.drain_for_swap("gateway.reload.before_build").await;
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.reload",
                event = "old_pool.drain.finish",
                phase = "pool.drain.finish",
                "gateway old upstream pool drain finish"
            );
        }
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "pool.seed.start",
            operation = "lazy_runtime_seed",
            phase = "pool.build.start",
            upstream_count = cfg.upstream.len(),
            "gateway reconcile"
        );
        crate::config::set_process_code_mode_enabled(cfg.code_mode.enabled);
        let fresh_pool = {
            let base_pool = self.new_base_pool(cfg.upstream_request_timeout());
            let pool = Arc::new(
                base_pool
                    .with_runtime_origin(runtime_origin_tag(origin))
                    .with_runtime_owner(owner),
            );
            pool.seed_lazy_upstreams(&cfg.upstream).await;
            Some(pool)
        };
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "pool.seed.finish",
            operation = "lazy_runtime_seed",
            phase = "pool.build.finish",
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );

        // Eagerly probe all upstreams so the after-snapshot reflects real tool
        // counts. seed_lazy_upstreams() only creates skeleton entries with empty
        // tool maps; without this the diff always reports tools_changed: ✗ even
        // when new upstreams were added, because both before and after snapshots
        // are empty (discovery is lazy and only triggered on the first list_tools
        // call). Bounded by LAB_UPSTREAM_DISCOVERY_CONCURRENCY (default 3) to
        // match the refresh path in code_mode_runtime.rs.
        if let Some(ref pool) = fresh_pool {
            let concurrency = std::env::var("LAB_UPSTREAM_DISCOVERY_CONCURRENCY")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(3);
            let pool_arc = Arc::clone(pool);
            let enabled: Vec<_> = cfg.upstream.iter().filter(|u| u.enabled).cloned().collect();
            // Step 1: connect all upstreams and discover tools.
            futures::stream::iter(enabled)
                .map(|upstream| {
                    let pool = Arc::clone(&pool_arc);
                    async move {
                        let name = upstream.name.clone();
                        match pool.ensure_tools_for_upstream(&upstream, None, None).await {
                            Ok(true) => tracing::info!(
                                surface = "dispatch",
                                service = "gateway",
                                action = "gateway.reload",
                                event = "upstream.probe.connected",
                                upstream = %name,
                                "upstream probed and connected on reload"
                            ),
                            Ok(false) => tracing::debug!(
                                surface = "dispatch",
                                service = "gateway",
                                action = "gateway.reload",
                                event = "upstream.probe.cached",
                                upstream = %name,
                                "upstream already healthy; probe skipped"
                            ),
                            Err(e) => tracing::warn!(
                                surface = "dispatch",
                                service = "gateway",
                                action = "gateway.reload",
                                event = "upstream.probe.error",
                                upstream = %name,
                                error = %e,
                                "upstream probe failed on reload"
                            ),
                        }
                    }
                })
                .buffer_unordered(concurrency)
                .collect::<Vec<_>>()
                .await;
            // Step 2: list resources for proxy_resources upstreams. This populates
            // entry.resource_uris so read_upstream_ui_resource can reverse-lookup
            // the owner of ui:// URIs (e.g. youtube_search_ui's MCP App widget).
            // Must run after tool discovery since list_upstream_resources only
            // contacts already-connected peers.
            pool_arc.list_upstream_resources().await;
        }

        let after = snapshot_from_pool(fresh_pool.clone()).await;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "pool.swap",
            phase = "pool.swap",
            old_pool_present,
            "gateway reconcile"
        );
        self.runtime.swap(fresh_pool).await;
        *self.protected_route_index.write().await =
            ProtectedRouteIndex::from_routes(&cfg.protected_mcp_routes);
        *self.config.write().await = cfg;
        let current_cfg = self.config.read().await.clone();
        let current_pool = self.runtime.current_pool().await;
        self.reconcile_runtime_state(&current_cfg, current_pool.as_deref())
            .await?;
        let diff = diff_catalogs(&before, &after);
        self.notify_catalog_changes(&diff);
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "catalog.refresh.finish",
            phase = "finish",
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            before_tool_count = before.tools.len(),
            after_tool_count = after.tools.len(),
            before_resource_count = before.resources.len(),
            after_resource_count = after.resources.len(),
            before_prompt_count = before.prompts.len(),
            after_prompt_count = after.prompts.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );
        Ok(diff)
    }

    fn notify_catalog_changes(&self, diff: &GatewayCatalogDiff) {
        if !diff.tools_changed && !diff.resources_changed && !diff.prompts_changed {
            return;
        }

        if let Some(notifier) = &self.notifier {
            notifier.notify_catalog_changes(diff);
        }
    }
}

pub(super) fn quarantine_unregistered_virtual_servers(
    mut cfg: LabConfig,
    registry: &ToolRegistry,
) -> (LabConfig, VirtualServerMigration) {
    let mut migration = VirtualServerMigration::default();
    let mut active = Vec::with_capacity(cfg.virtual_servers.len());

    for virtual_server in std::mem::take(&mut cfg.virtual_servers) {
        if registry.service(&virtual_server.service).is_some() {
            active.push(virtual_server);
            continue;
        }

        migration.quarantined.push(virtual_server.id.clone());
        let already_quarantined = cfg
            .quarantined_virtual_servers
            .iter()
            .any(|existing| existing.id == virtual_server.id);
        if !already_quarantined {
            cfg.quarantined_virtual_servers.push(virtual_server);
        }
    }

    cfg.virtual_servers = active;
    (cfg, migration)
}

/// Fingerprint of the config inputs that determine upstream-pool runtime shape.
///
/// Two configs with the same fingerprint seed the identical pool (same upstream
/// set, same spawn-guard prefs, same code-mode toggle / request timeout), so a
/// reload between them can keep the live pool instead of draining and rebuilding
/// it. Fields that do not affect the pool (protected routes, virtual servers,
/// tombstones, public URLs) are intentionally excluded — they are reconciled
/// separately and must not force a pool rebuild.
fn upstream_runtime_fingerprint(cfg: &LabConfig) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    // Per-upstream fingerprints reuse the canonical helper so this matches the
    // catalog cache's notion of "this upstream changed".
    for upstream in &cfg.upstream {
        hasher.update(
            crate::dispatch::gateway::code_mode::catalog_cache::fingerprint(upstream).as_bytes(),
        );
        hasher.update([0u8]);
    }
    // Pool-shaping config beyond the upstream list.
    hasher.update(serde_json::to_vec(&cfg.gateway).unwrap_or_default());
    hasher.update([0u8]);
    hasher.update(serde_json::to_vec(&cfg.code_mode).unwrap_or_default());
    hasher.update([0u8]);
    hasher.update(cfg.upstream_request_timeout().as_millis().to_le_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn snapshot_from_pool(pool: Option<Arc<UpstreamPool>>) -> GatewayCatalogSnapshot {
    let Some(pool) = pool else {
        return GatewayCatalogSnapshot::default();
    };

    GatewayCatalogSnapshot {
        tools: pool
            .healthy_tools()
            .await
            .into_iter()
            .map(|tool| tool.tool.name.to_string())
            .collect(),
        resources: pool
            .routable_upstream_names(
                crate::dispatch::upstream::types::UpstreamCapability::Resources,
            )
            .await
            .into_iter()
            .collect(),
        prompts: pool
            .routable_upstream_names(crate::dispatch::upstream::types::UpstreamCapability::Prompts)
            .await
            .into_iter()
            .collect(),
    }
}
