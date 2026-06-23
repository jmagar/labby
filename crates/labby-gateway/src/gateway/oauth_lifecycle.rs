use std::sync::Arc;

use crate::gateway::manager::GatewayManager;
use crate::gateway::oauth::{UpstreamOauthConnectionState, UpstreamOauthStatusView};
use labby_auth::upstream::encryption::EncryptionKey;
use labby_auth::upstream::manager::UpstreamOauthManager;
use labby_auth::upstream::types::{BeginAuthorization, OauthError};
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::UpstreamConfig;

pub(crate) mod probe;
#[cfg(test)]
mod tests;

pub(super) fn tool_error_from_oauth(error: OauthError) -> ToolError {
    ToolError::Sdk {
        sdk_kind: error.kind().to_string(),
        message: error.to_string(),
    }
}

/// Decide whether to use RFC 7591 dynamic registration for an upstream.
///
/// The `prefer_client_metadata_document` field on `UpstreamOauthConfig` is the
/// authoritative control:
/// - `Some(true)`  → always use CIMD (Client ID Metadata Document), never dynamic
/// - `Some(false)` → always use dynamic registration when `supports_dynamic` is true
/// - `None` → legacy default: upstreams named `"swag"` use CIMD; all others
///   use dynamic registration when available.
///
/// The `"swag"` name check is intentionally kept as a **documented legacy default**
/// so existing deployments that omit the field continue to work. New upstreams
/// should set `prefer_client_metadata_document` explicitly.
pub(super) fn should_use_dynamic_registration(
    upstream: &str,
    supports_dynamic: bool,
    prefer_cimd: Option<bool>,
) -> bool {
    if !supports_dynamic {
        return false;
    }
    match prefer_cimd {
        Some(true) => false,        // operator explicitly prefers CIMD
        Some(false) => true,        // operator explicitly prefers dynamic registration
        None => upstream != "swag", // legacy: "swag" uses CIMD by default
    }
}

impl GatewayManager {
    pub async fn oauth_upstream_configs(&self) -> Vec<UpstreamConfig> {
        self.config
            .read()
            .await
            .upstream
            .iter()
            .filter(|upstream| upstream.oauth.is_some())
            .cloned()
            .collect()
    }

    pub async fn oauth_upstream_config(&self, upstream_name: &str) -> Option<UpstreamConfig> {
        self.config
            .read()
            .await
            .upstream
            .iter()
            .find(|upstream| upstream.name == upstream_name && upstream.oauth.is_some())
            .cloned()
    }

    pub async fn probe_upstream_oauth(
        &self,
        url: &str,
    ) -> Result<crate::gateway::oauth::ProbeResult, ToolError> {
        self.probe_upstream_oauth_for_upstream(url, None).await
    }

    pub async fn probe_upstream_oauth_for_upstream(
        &self,
        url: &str,
        upstream_name: Option<&str>,
    ) -> Result<crate::gateway::oauth::ProbeResult, ToolError> {
        probe::run(self, url, upstream_name).await
    }

    pub fn oauth_sqlite(&self) -> Option<labby_auth::sqlite::SqliteStore> {
        self.oauth_sqlite.clone()
    }

    pub fn oauth_redirect_uri(&self) -> Option<String> {
        self.oauth_redirect_uri.as_deref().map(|s| s.to_string())
    }

    pub fn evict_subject_client(&self, upstream: &str, subject: &str) {
        if let Some(cache) = &self.oauth_client_cache {
            cache.evict_subject(upstream, subject);
        }
    }

    /// Look up the `UpstreamOauthManager` for `upstream` and return it, or
    /// emit a structured warning and return a `not_found` error.
    ///
    /// This is the single shared preamble used by `begin_upstream_authorization`,
    /// `complete_upstream_authorization_callback`, `upstream_oauth_status`, and
    /// `clear_upstream_credentials`. Extracted to avoid repeating the same 12-line
    /// pattern four times (Q-M5).
    fn require_oauth_manager(
        &self,
        upstream: &str,
        action: &'static str,
    ) -> Result<UpstreamOauthManager, ToolError> {
        self.upstream_oauth_manager(upstream).ok_or_else(|| {
            tracing::warn!(
                service = "upstream_oauth",
                action,
                upstream,
                kind = "not_found",
                "upstream oauth {action}: upstream not found or has no oauth config"
            );
            ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("upstream '{upstream}' not found or has no oauth config"),
            }
        })
    }

    pub async fn begin_upstream_authorization(
        &self,
        upstream: &str,
        subject: &str,
    ) -> Result<BeginAuthorization, ToolError> {
        let started = std::time::Instant::now();
        let manager = self.require_oauth_manager(upstream, "start")?;

        let result = manager.begin_authorization(subject).await.map_err(|e| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "start",
                upstream,
                kind = e.kind(),
                elapsed_ms = started.elapsed().as_millis(),
                "upstream oauth start: begin authorization failed"
            );
            tool_error_from_oauth(e)
        })?;

        tracing::info!(
            service = "upstream_oauth",
            action = "start",
            upstream,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth start: authorization URL generated"
        );
        Ok(result)
    }

    pub async fn complete_upstream_authorization_callback(
        &self,
        upstream: &str,
        subject: &str,
        code: &str,
        state: &str,
    ) -> Result<(), ToolError> {
        let started = std::time::Instant::now();
        let manager = self.require_oauth_manager(upstream, "callback")?;

        manager
            .complete_authorization_callback(subject, code, state)
            .await
            .map_err(|e| {
                tracing::warn!(
                    service = "upstream_oauth",
                    action = "callback",
                    upstream,
                    kind = e.kind(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "upstream oauth callback: token exchange failed"
                );
                tool_error_from_oauth(e)
            })?;

        tracing::info!(
            service = "upstream_oauth",
            action = "callback",
            upstream,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth callback: tokens stored"
        );

        if let Some(oauth_config) = manager.upstream_config().oauth.clone() {
            let _mutation_guard = self.config_mutation.lock().await;
            let mut cfg = self.config.read().await.clone();
            let Some(existing) = cfg.upstream.iter_mut().find(|u| u.name == upstream) else {
                tracing::debug!(
                    service = "upstream_oauth",
                    action = "callback",
                    upstream = %upstream,
                    "upstream oauth callback: no matching gateway in config; skipping oauth persistence"
                );
                return Ok(());
            };
            if existing.oauth.is_none() {
                tracing::info!(
                    service = "upstream_oauth",
                    action = "callback",
                    upstream = %upstream,
                    "upstream oauth callback: persisting oauth config for probe-created manager"
                );
                existing.oauth = Some(oauth_config);
                self.persist_config(cfg).await?;
            }
        }

        Ok(())
    }

    pub async fn upstream_oauth_status(
        &self,
        upstream: &str,
        subject: &str,
    ) -> Result<UpstreamOauthStatusView, ToolError> {
        let started = std::time::Instant::now();
        let manager = self.require_oauth_manager(upstream, "status")?;

        let mut row = manager.credential_row(subject).await.map_err(|e| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "status",
                upstream,
                kind = e.kind(),
                elapsed_ms = started.elapsed().as_millis(),
                "upstream oauth status: credential lookup failed"
            );
            tool_error_from_oauth(e)
        })?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let mut refresh_attempted = false;
        let mut refreshed = false;
        let mut refresh_error_kind = None;
        let mut refresh_error = None;

        if row
            .as_ref()
            .is_some_and(|row| row.access_token_expires_at - now <= 300)
        {
            refresh_attempted = true;
            match manager.refresh_auth_client(subject).await {
                Ok(()) => {
                    refreshed = true;
                    self.evict_subject_client(upstream, subject);
                    if let Err(error) = self
                        .reload_with_origin(Some("upstream-oauth.status.refresh"), None)
                        .await
                    {
                        tracing::warn!(
                            service = "upstream_oauth",
                            action = "status",
                            upstream,
                            subject,
                            kind = error.kind(),
                            elapsed_ms = started.elapsed().as_millis(),
                            "upstream oauth status: refreshed token but gateway rediscovery failed"
                        );
                    }
                    row = manager.credential_row(subject).await.map_err(|e| {
                        tracing::warn!(
                            service = "upstream_oauth",
                            action = "status",
                            upstream,
                            kind = e.kind(),
                            elapsed_ms = started.elapsed().as_millis(),
                            "upstream oauth status: credential lookup after refresh failed"
                        );
                        tool_error_from_oauth(e)
                    })?;
                }
                Err(error) => {
                    refresh_error_kind = Some(error.kind().to_string());
                    refresh_error = Some(error.to_string());
                    tracing::warn!(
                        service = "upstream_oauth",
                        action = "status",
                        upstream,
                        subject,
                        kind = error.kind(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "upstream oauth status: proactive refresh failed"
                    );
                }
            }
        }

        let (access_token_expires_at, seconds_until_expiry, refresh_token_present) = row
            .as_ref()
            .map(|row| {
                (
                    Some(row.access_token_expires_at),
                    Some(row.access_token_expires_at - now),
                    row.refresh_token_present,
                )
            })
            .unwrap_or((None, None, false));
        let expires_within_5m = seconds_until_expiry.is_some_and(|seconds| seconds <= 300);
        let mut state = match (
            row.is_some(),
            refresh_error_kind.is_some(),
            seconds_until_expiry,
        ) {
            (false, _, _) => UpstreamOauthConnectionState::Disconnected,
            (true, true, _) => UpstreamOauthConnectionState::RefreshFailed,
            (true, false, Some(seconds)) if seconds <= 0 => UpstreamOauthConnectionState::Expired,
            (true, false, Some(seconds)) if seconds <= 300 => {
                UpstreamOauthConnectionState::Expiring
            }
            (true, false, _) => UpstreamOauthConnectionState::Connected,
        };
        let authenticated = matches!(
            state,
            UpstreamOauthConnectionState::Connected | UpstreamOauthConnectionState::Expiring
        );
        let mut discovery_checked = false;
        let mut discovered_tool_count = 0;
        let mut exposed_tool_count = 0;
        let mut discovery_error = None;

        if authenticated {
            discovery_checked = true;
            let (request_timeout, relay_timeout) = {
                let cfg = self.config.read().await;
                (cfg.upstream_request_timeout(), cfg.upstream_relay_timeout())
            };
            let pool = self.new_base_pool(request_timeout, relay_timeout);
            let upstream_config = manager.upstream_config().clone();
            pool.discover_all_for_subject(&[upstream_config], subject)
                .await;
            if let Some(summary) = pool.cached_upstream_summary(upstream).await {
                discovered_tool_count = summary.discovered_tool_count;
                exposed_tool_count = summary.exposed_tool_count;
            }
            // Only a TOOL discovery failure marks the upstream as failed —
            // tool routing is gated solely on tool health (see
            // `UpstreamPool::healthy_tools`). A resources/prompts capability
            // probe that errors (e.g. an upstream whose endpoint returns HTTP
            // 400 for `resources/list` instead of a clean "unsupported" reply)
            // must NOT hide tools that discovered fine; surface it as a
            // non-fatal diagnostic and keep the connection authenticated.
            if let Some(error) = pool.upstream_tool_last_error(upstream).await {
                discovery_error = Some(error);
                state = UpstreamOauthConnectionState::DiscoveryFailed;
            } else if let Some(error) = pool.upstream_last_error(upstream).await {
                discovery_error = Some(error);
            }
        }
        let authenticated = matches!(
            state,
            UpstreamOauthConnectionState::Connected | UpstreamOauthConnectionState::Expiring
        );

        tracing::debug!(
            service = "upstream_oauth",
            action = "status",
            upstream,
            authenticated,
            expires_within_5m,
            refresh_attempted,
            refreshed,
            discovery_checked,
            discovered_tool_count,
            exposed_tool_count,
            state = ?state,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth status: checked"
        );
        Ok(UpstreamOauthStatusView {
            authenticated,
            upstream: upstream.to_string(),
            expires_within_5m,
            state,
            access_token_expires_at,
            seconds_until_expiry,
            refresh_token_present,
            refresh_attempted,
            refreshed,
            refresh_error_kind,
            refresh_error,
            discovery_checked,
            discovered_tool_count,
            exposed_tool_count,
            discovery_error,
        })
    }

    pub async fn clear_upstream_credentials(
        &self,
        upstream: &str,
        subject: &str,
    ) -> Result<(), ToolError> {
        let started = std::time::Instant::now();
        let manager = self.require_oauth_manager(upstream, "clear")?;

        manager.clear_credentials(subject).await.map_err(|e| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "clear",
                upstream,
                kind = e.kind(),
                elapsed_ms = started.elapsed().as_millis(),
                "upstream oauth clear: failed to clear credentials"
            );
            tool_error_from_oauth(e)
        })?;

        self.evict_subject_client(upstream, subject);
        tracing::info!(
            service = "upstream_oauth",
            action = "clear",
            upstream,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth clear: credentials cleared and client cache evicted"
        );
        Ok(())
    }
}

/// Unwrapped OAuth runtime resources, borrowed from `GatewayManager`.
///
/// Extracted by `require_oauth_runtime` and passed to probe helpers so they
/// don't have to repeat the 4-tuple `Option` destructuring themselves.
pub(super) struct OauthRuntime<'a> {
    pub managers: &'a dashmap::DashMap<String, UpstreamOauthManager>,
    pub sqlite: &'a labby_auth::sqlite::SqliteStore,
    pub key: &'a EncryptionKey,
    pub redirect_uri: &'a Arc<String>,
}

impl GatewayManager {
    /// Return the OAuth runtime resources, or a structured `not_configured` error.
    ///
    /// Centralises the 4-tuple match used inside `probe_upstream_oauth_for_upstream`
    /// so callers don't need to handle the wildcard arm inline.
    pub(super) fn require_oauth_runtime(&self) -> Result<OauthRuntime<'_>, ToolError> {
        match (
            self.upstream_oauth_managers.as_deref(),
            self.oauth_sqlite.as_ref(),
            self.oauth_key.as_ref(),
            self.oauth_redirect_uri.as_ref(),
        ) {
            (Some(managers), Some(sqlite), Some(key), Some(redirect_uri)) => Ok(OauthRuntime {
                managers,
                sqlite,
                key,
                redirect_uri,
            }),
            _ => {
                tracing::warn!(
                    service = "upstream_oauth",
                    action = "probe",
                    kind = "not_configured",
                    "upstream oauth probe: oauth resources not configured (LAB_PUBLIC_URL + LAB_OAUTH_ENCRYPTION_KEY required)"
                );
                Err(ToolError::Sdk {
                    sdk_kind: "not_configured".to_string(),
                    message: "upstream OAuth requires LAB_PUBLIC_URL (https) and LAB_OAUTH_ENCRYPTION_KEY to be set".to_string(),
                })
            }
        }
    }
}
