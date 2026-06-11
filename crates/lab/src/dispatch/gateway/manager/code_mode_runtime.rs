//! Code Mode runtime readiness and catalog freshness: upstream warm-up,
//! single-flight catalog reprobe with TTL coalescing, and the rendered-catalog
//! cache used by the `search` surface.

use std::collections::BTreeSet;
use std::sync::Arc;

use futures::StreamExt as _;
use tokio::time::Instant;

use crate::config::{CodeModeConfig, LabConfig};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::gateway::code_mode::CodeModeHistoryEntry;
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::dispatch::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};

use super::GatewayManager;

/// How long a successful full-reprobe result is considered fresh.
/// Back-to-back `refresh_code_mode_catalog` calls within this window
/// return immediately without hitting upstreams again.
const CATALOG_REFRESH_TTL: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone)]
struct CodeModeReprobeFailure {
    upstream: String,
    message: String,
}

fn upstream_allowed(upstream: &str, allowed_upstreams: Option<&BTreeSet<String>>) -> bool {
    allowed_upstreams.is_none_or(|allowed| allowed.contains(upstream))
}

impl GatewayManager {
    pub async fn code_mode_config(&self) -> CodeModeConfig {
        self.config.read().await.code_mode.clone()
    }

    pub async fn record_code_mode_history(&self, entry: CodeModeHistoryEntry) {
        self.code_mode_history.lock().await.push(entry);
    }

    pub async fn code_mode_history_snapshot(&self) -> Vec<CodeModeHistoryEntry> {
        self.code_mode_history.lock().await.snapshot()
    }

    pub async fn code_mode_enabled(&self) -> bool {
        self.config.read().await.code_mode.enabled
    }

    /// Ensure the upstream pool is warm and every enabled upstream has its tool
    /// list connected. Cloudflare-parity: there is no vector/lexical code-mode
    /// index to build — the `search` tool runs the caller's JS over the live
    /// catalog. When `wait_for_refresh` is set, connect upstreams synchronously
    /// so the first cold call sees a populated catalog; otherwise fire-and-forget.
    #[allow(dead_code)]
    pub async fn ensure_search_runtime_ready(
        &self,
        wait_for_refresh: bool,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(), ToolError> {
        self.ensure_search_runtime_ready_allowed(wait_for_refresh, owner, oauth_subject, None)
            .await
    }

    async fn ensure_search_runtime_ready_allowed(
        &self,
        wait_for_refresh: bool,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&BTreeSet<String>>,
    ) -> Result<(), ToolError> {
        let cfg = self.config.read().await.clone();
        if !cfg.code_mode.enabled {
            return Ok(());
        }

        let pool = self.ensure_lazy_upstream_pool(&cfg, owner).await;
        if wait_for_refresh {
            let mut failures = Vec::new();
            for upstream in cfg
                .upstream
                .iter()
                .filter(|u| u.enabled && upstream_allowed(&u.name, allowed_upstreams))
            {
                let subject = upstream.oauth.as_ref().and(oauth_subject);
                if let Err(err) = pool
                    .ensure_tools_for_upstream(upstream, subject, owner)
                    .await
                {
                    failures.push(CodeModeReprobeFailure {
                        upstream: upstream.name.clone(),
                        message: err.to_string(),
                    });
                }
            }
            if !failures.is_empty()
                && pool
                    .healthy_tools_allowed(allowed_upstreams)
                    .await
                    .is_empty()
            {
                let details = failures
                    .iter()
                    .map(|failure| format!("{}: {}", failure.upstream, failure.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(ToolError::Sdk {
                    sdk_kind: "upstream_connect_error".to_string(),
                    message: format!("failed to connect upstreams for code mode: {details}"),
                });
            }
        } else {
            self.spawn_code_mode_upstream_connections(
                pool,
                &cfg,
                owner,
                oauth_subject,
                allowed_upstreams,
            );
        }
        Ok(())
    }

    pub async fn ensure_upstream_tool_runtime_ready(
        &self,
        upstream_name: &str,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(), ToolError> {
        let cfg = self.config.read().await.clone();
        let Some(upstream) = cfg
            .upstream
            .iter()
            .find(|candidate| candidate.name == upstream_name)
        else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_upstream".to_string(),
                message: format!("unknown upstream `{upstream_name}`"),
            });
        };

        let pool = self.ensure_lazy_upstream_pool(&cfg, owner).await;

        let subject = upstream.oauth.as_ref().and(oauth_subject);
        pool.ensure_tools_for_upstream(upstream, subject, owner)
            .await
            .map_err(|err| ToolError::Sdk {
                sdk_kind: "upstream_connect_error".to_string(),
                message: format!("failed to connect upstream `{upstream_name}`: {err}"),
            })?;
        Ok(())
    }

    async fn ensure_lazy_upstream_pool(
        &self,
        cfg: &LabConfig,
        owner: Option<&UpstreamRuntimeOwner>,
    ) -> Arc<UpstreamPool> {
        if let Some(pool) = self.runtime.current_pool().await {
            pool.seed_lazy_upstreams(&cfg.upstream).await;
            return pool;
        }

        let _init_guard = self.lazy_pool_init.lock().await;
        let pool = if let Some(pool) = self.runtime.current_pool().await {
            pool
        } else {
            let mut base_pool = self.new_base_pool(cfg.upstream_request_timeout());
            base_pool = base_pool.with_runtime_owner(Some(owner.cloned().unwrap_or_else(|| {
                UpstreamRuntimeOwner {
                    surface: "dispatch".to_string(),
                    subject: Some(SHARED_GATEWAY_OAUTH_SUBJECT.to_string()),
                    request_id: None,
                    session_id: None,
                    client_name: None,
                    raw: None,
                }
            })));
            let pool = Arc::new(base_pool);
            self.runtime.swap(Some(Arc::clone(&pool))).await;
            pool
        };
        pool.seed_lazy_upstreams(&cfg.upstream).await;
        pool
    }

    pub async fn code_mode_catalog_tools(
        &self,
        allow_cold_connect: bool,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<Vec<UpstreamTool>, ToolError> {
        self.code_mode_catalog_tools_allowed(allow_cold_connect, owner, oauth_subject, None)
            .await
    }

    pub async fn code_mode_catalog_tools_allowed(
        &self,
        allow_cold_connect: bool,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&BTreeSet<String>>,
    ) -> Result<Vec<UpstreamTool>, ToolError> {
        if allow_cold_connect {
            self.refresh_code_mode_catalog_allowed(owner, oauth_subject, allowed_upstreams)
                .await?;
        } else {
            self.ensure_search_runtime_ready_allowed(
                false,
                owner,
                oauth_subject,
                allowed_upstreams,
            )
            .await?;
        }
        let Some(pool) = self.current_pool().await else {
            return Ok(Vec::new());
        };
        Ok(pool.healthy_tools_allowed(allowed_upstreams).await)
    }

    /// One-shot CLI variant of `code_mode_catalog_tools`: serve the codemode
    /// proxy catalog from the on-disk cache, connecting only upstreams whose
    /// cache entry is missing, stale, or fingerprint-mismatched.
    ///
    /// A one-shot `labby gateway code exec` must not connect the full upstream
    /// fleet per invocation just to generate the `codemode.*` proxy. Tool calls
    /// still resolve live (`resolve_code_mode_upstream_tool` ensures the target
    /// upstream), so a stale cache can only mis-shape the proxy — `callTool`
    /// remains the always-fresh escape hatch. Upstreams that fail to probe are
    /// omitted from the proxy and NOT cached, so the next run retries them.
    pub async fn code_mode_catalog_tools_cached(
        &self,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<Vec<UpstreamTool>, ToolError> {
        use crate::dispatch::gateway::code_mode::catalog_cache;

        let cfg = self.config.read().await.clone();
        if !cfg.code_mode.enabled {
            return Ok(Vec::new());
        }

        let cache = catalog_cache::CatalogCache::load();
        let mut tools = Vec::new();
        let mut updates = Vec::new();
        let mut pool = None;
        for upstream in cfg.upstream.iter().filter(|u| u.enabled) {
            let fingerprint = catalog_cache::fingerprint(upstream);
            if let Some(cached) = cache.fresh_tools(&upstream.name, &fingerprint) {
                tools.extend(cached);
                continue;
            }
            let pool = match &pool {
                Some(pool) => Arc::clone(pool),
                None => {
                    let fresh = self.ensure_lazy_upstream_pool(&cfg, owner).await;
                    pool = Some(Arc::clone(&fresh));
                    fresh
                }
            };
            let subject = upstream.oauth.as_ref().and(oauth_subject);
            match pool
                .ensure_tools_for_upstream(upstream, subject, owner)
                .await
            {
                Ok(_) => {
                    let live = pool.healthy_tools_for_upstream(&upstream.name).await;
                    updates.push(catalog_cache::CatalogCacheUpdate {
                        upstream_name: upstream.name.clone(),
                        fingerprint,
                        tools: live.clone(),
                    });
                    tools.extend(live);
                }
                Err(error) => {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.catalog_cache",
                        upstream = %upstream.name,
                        error = %error,
                        "upstream connect failed; omitting from codemode proxy (not cached)"
                    );
                }
            }
        }
        catalog_cache::merge_and_store(updates);
        Ok(tools)
    }

    /// Refresh the transient Code Mode catalog from live upstream metadata.
    ///
    /// This is intentionally a manager-level policy: Code Mode needs a fresh
    /// per-call catalog, while `UpstreamPool` only owns the connect/reprobe
    /// mechanics. Reprobe uses existing live peers when possible and reconnects
    /// when needed, so partial-but-healthy catalogs do not mask tool-list growth.
    ///
    /// **P-H1 improvements:**
    /// - Single-flight + TTL coalescing: while one refresh is in flight, a
    ///   concurrent caller that arrives within `CATALOG_REFRESH_TTL` of the last
    ///   completed refresh skips its own reprobe and rides on the in-flight one.
    ///   This bounds the cost of bursty back-to-back `search` calls **without**
    ///   ever masking tool-list growth for a lone caller: an isolated
    ///   `allow_cold_connect = true` call always reprobes, because reprobe is the
    ///   system's growth-detection mechanism (see the read-only catalog expansion
    ///   test). The TTL only suppresses *redundant concurrent* work, never the
    ///   single-caller freshness contract.
    /// - Parallel reprobe: all enabled upstreams are probed concurrently, bounded by
    ///   `upstream_discovery_concurrency()` (default 3, env `LAB_UPSTREAM_DISCOVERY_CONCURRENCY`).
    #[allow(dead_code)]
    pub async fn refresh_code_mode_catalog(
        &self,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(), ToolError> {
        self.refresh_code_mode_catalog_allowed(owner, oauth_subject, None)
            .await
    }

    async fn refresh_code_mode_catalog_allowed(
        &self,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&BTreeSet<String>>,
    ) -> Result<(), ToolError> {
        let cfg = self.config.read().await.clone();
        if !cfg.code_mode.enabled {
            return Ok(());
        }

        // --- Single-flight + TTL coalescing ---
        // try_lock succeeds only when no other refresh is in progress. If a
        // concurrent caller already holds the lock AND the last refresh
        // completed within the freshness window, coalesce onto the in-flight
        // refresh rather than queueing a redundant reprobe. Crucially this only
        // fires under genuine concurrency: a lone caller always acquires the
        // lock and reprobes, so tool-list growth is never masked.
        let _inflight_guard = match self.code_mode_refresh_inflight.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                let within_ttl = {
                    let deadline_guard = self.code_mode_refresh_deadline.lock().await;
                    deadline_guard.is_some_and(|deadline| Instant::now() < deadline)
                };
                if within_ttl {
                    tracing::debug!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.refresh_catalog",
                        "concurrent refresh in flight within TTL, coalescing"
                    );
                    return Ok(());
                }
                // Concurrent refresh in flight but TTL expired — wait for the
                // lock so this caller still observes a fresh catalog.
                self.code_mode_refresh_inflight.lock().await
            }
        };

        let pool = self.ensure_lazy_upstream_pool(&cfg, owner).await;
        // Mirror `pool/helpers.rs::upstream_discovery_concurrency()` — the
        // function is pub(crate) inside a private module so we read the env var
        // directly rather than reaching through an inaccessible module path.
        let concurrency = std::env::var("LAB_UPSTREAM_DISCOVERY_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(3);

        // Clone context for async move blocks.
        let owner_cloned = owner.cloned();
        let oauth_subject_cloned = oauth_subject.map(ToOwned::to_owned);
        let pool_arc = Arc::clone(&pool);

        // Parallel reprobe — all enabled upstreams concurrently, bounded by concurrency.
        let enabled_upstreams: Vec<_> = cfg
            .upstream
            .iter()
            .filter(|u| u.enabled && upstream_allowed(&u.name, allowed_upstreams))
            .cloned()
            .collect();

        let results: Vec<_> = futures::stream::iter(enabled_upstreams.into_iter())
            .map(|upstream| {
                let pool = Arc::clone(&pool_arc);
                let owner = owner_cloned.clone();
                let oauth_subject = oauth_subject_cloned.clone();
                async move {
                    let subject = upstream.oauth.as_ref().and(oauth_subject.as_deref());
                    let outcome = pool
                        .reprobe_tools_for_upstream_as(&upstream, subject, owner.as_ref())
                        .await;
                    (upstream, outcome)
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        let mut failures = Vec::new();
        let mut cache_updates = Vec::new();
        for (upstream, outcome) in results {
            match outcome {
                Ok(_) => {
                    // Keep the one-shot CLI catalog cache warm from the
                    // long-lived surface so `gateway code exec` rarely has to
                    // cold-connect upstreams for proxy generation.
                    cache_updates.push(
                        crate::dispatch::gateway::code_mode::catalog_cache::CatalogCacheUpdate {
                            upstream_name: upstream.name.clone(),
                            fingerprint:
                                crate::dispatch::gateway::code_mode::catalog_cache::fingerprint(
                                    &upstream,
                                ),
                            tools: pool.healthy_tools_for_upstream(&upstream.name).await,
                        },
                    );
                }
                Err(err) => {
                    failures.push(CodeModeReprobeFailure {
                        upstream: upstream.name.clone(),
                        message: err.to_string(),
                    });
                }
            }
        }
        crate::dispatch::gateway::code_mode::catalog_cache::merge_and_store(cache_updates);

        if !failures.is_empty()
            && pool
                .healthy_tools_allowed(allowed_upstreams)
                .await
                .is_empty()
        {
            let details = failures
                .iter()
                .map(|failure| format!("{}: {}", failure.upstream, failure.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_connect_error".to_string(),
                message: format!("failed to refresh Code Mode catalog: {details}"),
            });
        }

        // Stamp the TTL deadline so a *concurrent* caller that arrives while a
        // later refresh is in flight can coalesce within the freshness window.
        {
            let mut deadline_guard = self.code_mode_refresh_deadline.lock().await;
            *deadline_guard = Some(Instant::now() + CATALOG_REFRESH_TTL);
        }

        Ok(())
    }

    /// Store a freshly rendered catalog in the manager-level render cache.
    ///
    /// Called by `code_search_catalog` after a cache miss so subsequent searches
    /// within the same healthy-tool fingerprint skip `generate_tool_types` per entry.
    pub async fn store_catalog_render_cache(
        &self,
        cache: crate::dispatch::gateway::code_mode::CatalogRenderCache,
    ) {
        let mut guard = self.code_mode_catalog_render_cache.lock().await;
        *guard = Some(cache);
    }

    /// Return the cached catalog render if the fingerprint still matches.
    ///
    /// Returns `Some((entries, catalog_json, serialized_size))` on a hit,
    /// `None` on a miss (caller must rebuild and call `store_catalog_render_cache`).
    pub async fn cached_catalog_render(
        &self,
        fingerprint: &str,
    ) -> Option<(
        Vec<crate::dispatch::gateway::code_mode::CodeModeCatalogEntry>,
        String,
        usize,
    )> {
        let guard = self.code_mode_catalog_render_cache.lock().await;
        guard.as_ref().and_then(|cache| {
            if cache.fingerprint == fingerprint {
                Some((
                    cache.entries.clone(),
                    cache.catalog_json.clone(),
                    cache.serialized_size,
                ))
            } else {
                None
            }
        })
    }

    /// Fire-and-forget: spawn per-upstream connection tasks for exclusive code mode.
    ///
    /// Unlike `refresh_code_mode_indexes_if_stale` this does NOT build vector
    /// search indexes.  It only ensures each enabled upstream has its tool list
    /// in the pool so `healthy_tools()` is non-empty.
    fn spawn_code_mode_upstream_connections(
        &self,
        pool: Arc<UpstreamPool>,
        cfg: &LabConfig,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&BTreeSet<String>>,
    ) {
        let owner = owner.cloned();
        let oauth_subject = oauth_subject.map(ToOwned::to_owned);
        for upstream in cfg
            .upstream
            .iter()
            .filter(|u| u.enabled && upstream_allowed(&u.name, allowed_upstreams))
        {
            let pool = Arc::clone(&pool);
            let upstream = upstream.clone();
            let owner = owner.clone();
            let oauth_subject = oauth_subject.clone();
            tokio::spawn(async move {
                // `ensure_tools_for_upstream` skips the upstream internally
                // when it already has healthy tools.
                let subject = upstream.oauth.as_ref().and(oauth_subject.as_deref());
                if let Err(err) = pool
                    .ensure_tools_for_upstream(&upstream, subject, owner.as_ref())
                    .await
                {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.warm_upstream",
                        upstream = %upstream.name,
                        error = %err,
                        "code_mode upstream connection failed during warm-up"
                    );
                } else {
                    tracing::debug!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.warm_upstream",
                        upstream = %upstream.name,
                        "code_mode upstream connected"
                    );
                }
            });
        }
    }
}
