//! Upstream OAuth resource/token management: manager-map reconciliation on
//! reload, client-cache eviction, and authorization polling.

use std::collections::{BTreeMap, HashSet};

use lab_auth::upstream::cache::OauthClientCache;
use lab_auth::upstream::manager::UpstreamOauthManager;
use lab_runtime::error::ToolError;
use lab_runtime::gateway_config::{GatewayConfig, UpstreamConfig};

use super::GatewayManager;

fn upstream_oauth_manager_matches(existing: &UpstreamConfig, desired: &UpstreamConfig) -> bool {
    existing.name == desired.name && existing.url == desired.url && existing.oauth == desired.oauth
}

impl GatewayManager {
    /// Poll `gateway.oauth.status` until the given upstream is authenticated
    /// for `subject`, or until `timeout` elapses (Q-H3).
    ///
    /// Returns `true` when authenticated within the deadline, `false` on
    /// timeout.  Any status-check error is propagated.
    ///
    /// Moving this poll loop from `cli/gateway.rs` into shared dispatch means
    /// every surface (CLI, API, MCP) shares the same orchestration logic.
    pub async fn await_upstream_authorization(
        &self,
        upstream: &str,
        subject: &str,
        timeout: std::time::Duration,
    ) -> Result<bool, ToolError> {
        use tokio::time::{Instant, sleep};
        let deadline = Instant::now() + timeout;
        loop {
            let status = crate::gateway::oauth::status(self, upstream, subject).await?;
            if status.authenticated {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    pub(super) fn reconcile_upstream_oauth_managers(&self, cfg: &GatewayConfig) {
        let oauth_upstreams: BTreeMap<&str, &UpstreamConfig> = cfg
            .upstream
            .iter()
            .filter(|upstream| upstream.oauth.is_some())
            .map(|upstream| (upstream.name.as_str(), upstream))
            .collect();

        // Unconditionally evict cache entries for OAuth upstreams that are no
        // longer in config.  This must run even when upstream_oauth_managers is
        // not initialised — the cache is independent of the manager map.
        if let Some(cache) = &self.oauth_client_cache {
            let known: HashSet<&str> = oauth_upstreams.keys().copied().collect();
            cache.evict_upstreams_not_in(&known);
        }

        let Some(managers) = self.upstream_oauth_managers.as_ref() else {
            return;
        };

        let removed: Vec<String> = managers
            .iter()
            .filter_map(|entry| {
                (!oauth_upstreams.contains_key(entry.key().as_str())).then(|| entry.key().clone())
            })
            .collect();
        for name in removed {
            managers.remove(&name);
            self.evict_upstream_clients(&name);
            tracing::info!(
                upstream = %name,
                "removed upstream oauth manager during gateway reload"
            );
        }

        if oauth_upstreams.is_empty() {
            return;
        }

        let (Some(sqlite), Some(key), Some(redirect_uri)) = (
            self.oauth_sqlite.as_ref(),
            self.oauth_key.as_ref(),
            self.oauth_redirect_uri.as_ref(),
        ) else {
            for name in oauth_upstreams.keys() {
                if !managers.contains_key(*name) {
                    tracing::warn!(
                        upstream = name,
                        "new oauth upstream added via reload but oauth runtime resources are not configured"
                    );
                }
            }
            return;
        };

        for (name, upstream) in oauth_upstreams {
            let should_replace = managers.get(name).is_none_or(|existing| {
                !upstream_oauth_manager_matches(existing.upstream_config(), upstream)
            });
            if !should_replace {
                continue;
            }

            if managers.remove(name).is_some() {
                self.evict_upstream_clients(name);
                tracing::info!(
                    upstream = name,
                    "replaced stale upstream oauth manager during gateway reload"
                );
            } else {
                tracing::info!(
                    upstream = name,
                    "registered new upstream oauth manager during gateway reload"
                );
            }

            managers.insert(
                name.to_string(),
                UpstreamOauthManager::new(
                    sqlite.clone(),
                    key.clone(),
                    upstream.clone(),
                    redirect_uri.as_ref().clone(),
                ),
            );
        }
    }

    // Called only from `#[cfg(test)]` blocks in `cli/gateway.rs`.  Kept on the
    // public surface for API symmetry with the other OAuth resource getters;
    // the allow is intentional — removing it would require adding cfg(test) to
    // a method that conceptually belongs on the production type.
    #[allow(dead_code)]
    #[must_use]
    pub fn oauth_client_cache(&self) -> Option<OauthClientCache> {
        self.oauth_client_cache.clone()
    }

    /// Probe `url` for OAuth support via RFC 8414 AS metadata discovery.
    ///
    /// On success, registers a transient `UpstreamOauthManager` (Dynamic strategy)
    /// keyed by the URL hostname so subsequent `begin_upstream_authorization` calls
    /// work without requiring a static config entry.
    /// Returns the upstream OAuth SQLite store, if configured.
    /// Returns the upstream OAuth callback redirect URI, if configured.
    ///
    /// Used by the `/.well-known/oauth-client` endpoint to build the client
    /// metadata document served to CIMD-supporting authorization servers.
    pub fn upstream_oauth_manager(&self, upstream: &str) -> Option<UpstreamOauthManager> {
        self.upstream_oauth_managers
            .as_ref()
            .and_then(|managers| managers.get(upstream).map(|entry| entry.clone()))
    }

    pub fn evict_upstream_clients(&self, upstream: &str) {
        if let Some(cache) = &self.oauth_client_cache {
            cache.evict_upstream(upstream);
        }
    }
}
