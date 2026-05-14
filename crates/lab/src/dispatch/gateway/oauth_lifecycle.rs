use rmcp::transport::AuthorizationManager;
use url::Url;

use crate::config::{
    ToolSearchConfig, UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode,
    UpstreamOauthRegistration,
};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::gateway::oauth::{UpstreamOauthConnectionState, UpstreamOauthStatusView};
use crate::dispatch::redact::redact_url;
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::oauth::upstream::manager::UpstreamOauthManager;
use crate::oauth::upstream::types::{BeginAuthorization, OauthError};

fn tool_error_from_oauth(error: OauthError) -> ToolError {
    ToolError::Sdk {
        sdk_kind: error.kind().to_string(),
        message: error.to_string(),
    }
}

fn should_use_dynamic_registration(upstream: &str, supports_dynamic: bool) -> bool {
    supports_dynamic && upstream != "swag"
}

fn validate_probe_url(raw: &str) -> Result<Url, ToolError> {
    let parsed = Url::parse(raw).map_err(|_| ToolError::InvalidParam {
        message: "invalid upstream URL".to_string(),
        param: "url".to_string(),
    })?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ToolError::InvalidParam {
            message: "upstream OAuth probe URL must not include userinfo".to_string(),
            param: "url".to_string(),
        });
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ToolError::InvalidParam {
            message: "upstream OAuth probe URL must not include query strings or fragments"
                .to_string(),
            param: "url".to_string(),
        });
    }
    Ok(parsed)
}

fn validate_probe_upstream_name(raw: &str) -> Result<String, ToolError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "upstream name must not be empty".to_string(),
            param: "upstream".to_string(),
        });
    }
    if name.len() > 128
        || name
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\' | '?' | '#'))
    {
        return Err(ToolError::InvalidParam {
            message: "upstream name contains unsupported characters".to_string(),
            param: "upstream".to_string(),
        });
    }
    Ok(name.to_string())
}

fn probe_manager_key(parsed: &Url) -> String {
    let host = parsed.host_str().unwrap_or("upstream");
    let mut key = match parsed.port() {
        Some(port) => format!("{host}-{port}"),
        None => host.to_string(),
    };
    let path = parsed.path().trim_matches('/');
    if !path.is_empty() {
        key.push('-');
        key.push_str(path);
    }
    key.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        + &format!(
            "-{:016x}",
            xxhash_rust::xxh3::xxh3_64(parsed.as_str().as_bytes())
        )
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
    ) -> Result<crate::dispatch::gateway::oauth::ProbeResult, ToolError> {
        self.probe_upstream_oauth_for_upstream(url, None).await
    }

    pub async fn probe_upstream_oauth_for_upstream(
        &self,
        url: &str,
        upstream_name: Option<&str>,
    ) -> Result<crate::dispatch::gateway::oauth::ProbeResult, ToolError> {
        let started = std::time::Instant::now();
        let parsed = validate_probe_url(url)?;
        let canonical_url = parsed.as_str().to_string();
        let redacted_url = redact_url(&canonical_url);
        let name = match upstream_name {
            Some(name) => validate_probe_upstream_name(name)?,
            None => {
                let cfg = self.config.read().await;
                cfg.upstream
                    .iter()
                    .find(|upstream| upstream.url.as_deref() == Some(canonical_url.as_str()))
                    .map(|upstream| upstream.name.clone())
                    .unwrap_or_else(|| probe_manager_key(&parsed))
            }
        };
        let name_is_persisted = {
            let cfg = self.config.read().await;
            match cfg.upstream.iter().find(|upstream| upstream.name == name) {
                Some(existing) if existing.url.as_deref() != Some(canonical_url.as_str()) => {
                    return Err(ToolError::InvalidParam {
                        message: format!(
                            "upstream `{name}` is already configured for a different URL"
                        ),
                        param: "upstream".to_string(),
                    });
                }
                Some(_) => true,
                None => false,
            }
        };

        // SSRF validation (synchronous DNS) — must run in spawn_blocking.
        // Also enforces https-only and rejects RFC 1918, loopback, and link-local.
        let url_for_check = canonical_url.clone();
        tokio::task::spawn_blocking(move || {
            crate::dispatch::marketplace::validate_registry_url(&url_for_check)
        })
        .await
        .map_err(|e| ToolError::internal_message(format!("SSRF validation task panicked: {e}")))
        .inspect_err(|_| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "probe",
                url = %redacted_url,
                kind = "ssrf_blocked",
                "upstream oauth probe: SSRF validation task error"
            );
        })??;

        tracing::info!(
            service = "upstream_oauth",
            action = "probe",
            upstream = %name,
            url = %redacted_url,
            "upstream oauth probe: connecting"
        );

        let auth_manager = AuthorizationManager::new(&canonical_url)
            .await
            .map_err(|e| {
                tracing::warn!(
                    service = "upstream_oauth",
                    action = "probe",
                    upstream = %name,
                    url = %redacted_url,
                    kind = "network_error",
                    error = %e,
                    elapsed_ms = started.elapsed().as_millis(),
                    "upstream oauth probe: connection failed"
                );
                ToolError::Sdk {
                    sdk_kind: "network_error".to_string(),
                    message: format!("failed to connect to upstream: {e}"),
                }
            })?;

        let metadata = match auth_manager.discover_metadata().await {
            Ok(m) => {
                tracing::info!(
                    service = "upstream_oauth",
                    action = "probe",
                    upstream = %name,
                    url = %redacted_url,
                    issuer = m.issuer.as_deref().unwrap_or("<none>"),
                    supports_dynamic_registration = m.registration_endpoint.is_some(),
                    scopes = ?m.scopes_supported,
                    elapsed_ms = started.elapsed().as_millis(),
                    "upstream oauth probe: OAuth metadata discovered"
                );
                m
            }
            Err(e) => {
                tracing::info!(
                    service = "upstream_oauth",
                    action = "probe",
                    upstream = %name,
                    url = %redacted_url,
                    reason = %e,
                    elapsed_ms = started.elapsed().as_millis(),
                    "upstream oauth probe: no OAuth metadata found"
                );
                return Ok(crate::dispatch::gateway::oauth::ProbeResult {
                    upstream: name,
                    url: redacted_url.clone(),
                    transient: false,
                    durability: "not_registered_no_oauth_metadata".to_string(),
                    oauth_discovered: false,
                    issuer: None,
                    scopes: None,
                    registration_strategy: None,
                });
            }
        };

        let supports_dynamic = metadata.registration_endpoint.is_some();
        let use_dynamic_registration = should_use_dynamic_registration(&name, supports_dynamic);
        let strategy = if use_dynamic_registration {
            "dynamic"
        } else {
            "client_metadata_document"
        };

        // Check each prerequisite independently so the error names only what's missing.
        if self.oauth_key.is_none()
            || self.oauth_sqlite.is_none()
            || self.oauth_redirect_uri.is_none()
        {
            let missing: Vec<&str> = [
                self.oauth_key
                    .is_none()
                    .then_some("LAB_OAUTH_ENCRYPTION_KEY"),
                // redirect_uri derives from LAB_PUBLIC_URL; missing key is the same root cause
                self.oauth_redirect_uri
                    .is_none()
                    .then_some("LAB_PUBLIC_URL"),
            ]
            .into_iter()
            .flatten()
            .collect();
            let message = format!(
                "upstream OAuth not configured — set {} to enable it",
                missing.join(" and ")
            );
            tracing::warn!(
                service = "upstream_oauth",
                action = "probe",
                upstream = %name,
                kind = "not_configured",
                elapsed_ms = started.elapsed().as_millis(),
                %message,
                "upstream oauth probe: oauth resources not configured"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "not_configured".to_string(),
                message,
            });
        }

        match (
            self.upstream_oauth_managers.as_ref(),
            self.oauth_sqlite.as_ref(),
            self.oauth_key.as_ref(),
            self.oauth_redirect_uri.as_ref(),
        ) {
            (Some(managers), Some(sqlite), Some(key), Some(redirect_uri)) => {
                if let Some(existing) = managers.get(&name) {
                    let existing_url = existing.upstream_config().url.clone();
                    drop(existing);
                    if existing_url.as_deref() != Some(canonical_url.as_str()) {
                        if name_is_persisted {
                            return Err(ToolError::InvalidParam {
                                message: format!(
                                    "upstream `{name}` is already configured for a different URL"
                                ),
                                param: "upstream".to_string(),
                            });
                        }
                        managers.remove(&name);
                        self.evict_upstream_clients(&name);
                        tracing::info!(
                            service = "upstream_oauth",
                            action = "probe",
                            upstream = %name,
                            "upstream oauth probe: replaced stale transient manager"
                        );
                    } else {
                        tracing::info!(
                            service = "upstream_oauth",
                            action = "probe",
                            upstream = %name,
                            elapsed_ms = started.elapsed().as_millis(),
                            "upstream oauth probe: reusing existing manager"
                        );
                    }
                }

                if !managers.contains_key(&name) {
                    let registration = if use_dynamic_registration {
                        UpstreamOauthRegistration::Dynamic
                    } else {
                        // No RFC 7591 dynamic registration — use the Client ID Metadata
                        // Document (CIMD) approach: the lab's own metadata-document URL
                        // acts as the client_id. Derive it from the redirect_uri origin.
                        let metadata_doc_url = Url::parse(redirect_uri.as_str())
                            .ok()
                            .map(|mut u| {
                                u.set_path("/.well-known/oauth-client");
                                u.set_query(None);
                                u.set_fragment(None);
                                u.to_string()
                            })
                            .unwrap_or_default();
                        UpstreamOauthRegistration::ClientMetadataDocument {
                            url: metadata_doc_url,
                        }
                    };
                    let config = UpstreamConfig {
                        enabled: true,
                        name: name.clone(),
                        url: Some(canonical_url.clone()),
                        bearer_token_env: None,
                        command: None,
                        args: vec![],
                        env: std::collections::BTreeMap::new(),
                        proxy_resources: false,
                        proxy_prompts: false,
                        expose_tools: None,
                        expose_resources: None,
                        expose_prompts: None,
                        oauth: Some(UpstreamOauthConfig {
                            mode: UpstreamOauthMode::AuthorizationCodePkce,
                            registration,
                            scopes: metadata.scopes_supported.clone(),
                        }),
                        imported_from: None,
                        tool_search: ToolSearchConfig::default(),
                    };
                    let manager = UpstreamOauthManager::new(
                        sqlite.clone(),
                        key.clone(),
                        config,
                        redirect_uri.as_ref().clone(),
                    );
                    managers.insert(name.clone(), manager);
                    tracing::info!(
                        service = "upstream_oauth",
                        action = "probe",
                        upstream = %name,
                        registration_strategy = strategy,
                        elapsed_ms = started.elapsed().as_millis(),
                        "upstream oauth probe: transient manager registered"
                    );
                }
            }
            _ => {
                tracing::warn!(
                    service = "upstream_oauth",
                    action = "probe",
                    upstream = %name,
                    kind = "not_configured",
                    elapsed_ms = started.elapsed().as_millis(),
                    "upstream oauth probe: oauth resources not configured (LAB_PUBLIC_URL + LAB_OAUTH_ENCRYPTION_KEY required)"
                );
                return Err(ToolError::Sdk {
                    sdk_kind: "not_configured".to_string(),
                    message: "upstream OAuth requires LAB_PUBLIC_URL (https) and LAB_OAUTH_ENCRYPTION_KEY to be set".to_string(),
                });
            }
        }

        Ok(crate::dispatch::gateway::oauth::ProbeResult {
            upstream: name,
            url: redacted_url.clone(),
            transient: true,
            durability: "transient_until_oauth_callback_persists_gateway_config".to_string(),
            oauth_discovered: true,
            issuer: metadata.issuer,
            scopes: metadata.scopes_supported,
            registration_strategy: Some(strategy.to_string()),
        })
    }

    pub fn oauth_sqlite(&self) -> Option<lab_auth::sqlite::SqliteStore> {
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

    pub async fn begin_upstream_authorization(
        &self,
        upstream: &str,
        subject: &str,
    ) -> Result<BeginAuthorization, ToolError> {
        let started = std::time::Instant::now();
        let manager = self.upstream_oauth_manager(upstream).ok_or_else(|| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "start",
                upstream,
                kind = "not_found",
                "upstream oauth start: upstream not found or has no oauth config"
            );
            ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("upstream '{upstream}' not found or has no oauth config"),
            }
        })?;

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
        let manager = self.upstream_oauth_manager(upstream).ok_or_else(|| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "callback",
                upstream,
                kind = "not_found",
                "upstream oauth callback: upstream not found or has no oauth config"
            );
            ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("upstream '{upstream}' not found or has no oauth config"),
            }
        })?;

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
        let manager = self.upstream_oauth_manager(upstream).ok_or_else(|| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "status",
                upstream,
                kind = "not_found",
                "upstream oauth status: upstream not found or has no oauth config"
            );
            ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("upstream '{upstream}' not found or has no oauth config"),
            }
        })?;

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
            let pool = match &self.oauth_client_cache {
                Some(cache) => UpstreamPool::new().with_oauth_client_cache(cache.clone()),
                None => UpstreamPool::new(),
            };
            let upstream_config = manager.upstream_config().clone();
            pool.discover_all_for_subject(&[upstream_config], subject)
                .await;
            if let Some(summary) = pool.cached_upstream_summary(upstream).await {
                discovered_tool_count = summary.discovered_tool_count;
                exposed_tool_count = summary.exposed_tool_count;
            }
            if let Some(error) = pool.upstream_last_error(upstream).await {
                discovery_error = Some(error);
                state = UpstreamOauthConnectionState::DiscoveryFailed;
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
        let manager = self.upstream_oauth_manager(upstream).ok_or_else(|| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "clear",
                upstream,
                kind = "not_found",
                "upstream oauth clear: upstream not found or has no oauth config"
            );
            ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("upstream '{upstream}' not found or has no oauth config"),
            }
        })?;

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

#[cfg(test)]
mod tests {
    use super::{
        probe_manager_key, should_use_dynamic_registration, validate_probe_upstream_name,
        validate_probe_url,
    };

    #[test]
    fn validate_probe_url_rejects_userinfo() {
        let error = validate_probe_url("https://user:secret@example.com/mcp").unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
        assert!(error.to_string().contains("userinfo"));
    }

    #[test]
    fn validate_probe_url_rejects_query_and_fragment() {
        let error =
            validate_probe_url("https://example.com/mcp?token=secret#callback").unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
        assert!(error.to_string().contains("query strings or fragments"));
    }

    #[test]
    fn probe_manager_key_includes_port_and_path() {
        let parsed = validate_probe_url("https://example.com:8443/mcp/v1").unwrap();
        assert!(probe_manager_key(&parsed).starts_with("example.com-8443-mcp-v1-"));
    }

    #[test]
    fn probe_manager_key_distinguishes_colliding_paths() {
        let first = validate_probe_url("https://example.com/mcp-a").unwrap();
        let second = validate_probe_url("https://example.com/mcp/a").unwrap();
        assert_ne!(probe_manager_key(&first), probe_manager_key(&second));
    }

    #[test]
    fn validate_probe_upstream_name_rejects_path_like_values() {
        let error = validate_probe_upstream_name("../plex").unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
    }

    #[test]
    fn swag_uses_client_metadata_document_even_when_dynamic_registration_is_advertised() {
        assert!(!should_use_dynamic_registration("swag", true));
        assert!(should_use_dynamic_registration("github", true));
        assert!(!should_use_dynamic_registration("github", false));
    }
}
