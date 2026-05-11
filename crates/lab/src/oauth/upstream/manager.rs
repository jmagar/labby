//! Upstream OAuth lifecycle manager.
//!
//! `UpstreamOauthManager` orchestrates the full outbound `authorization_code` + PKCE
//! flow for one configured upstream MCP server.  It is per-upstream (constructed once
//! per `UpstreamConfig` that has an `oauth` block) and is `Clone` / `Send + Sync`.
//!
//! ## Subject
//!
//! All public methods take a `subject: &str` identifying the lab user initiating the
//! flow.  Credentials are stored and refreshed independently per `(upstream, subject)`.
//!
//! ## Two-phase authorization
//!
//! ```text
//! begin_authorization(subject)
//!   ↓  generates PKCE + CSRF, stores state in SQLite, returns redirect URL
//! browser → AS → callback
//!   ↓
//! complete_authorization_callback(subject, code, csrf)
//!   ↓  exchanges code, stores encrypted tokens in SQLite
//! build_auth_client(subject)
//!   ↓  loads stored credentials, proactively refreshes if stale
//! AuthClient<reqwest::Client>  → used by UpstreamPool for MCP calls
//! ```
//!
//! ## AS metadata caching
//!
//! Authorization server metadata is fetched once per upstream (not per-subject) and
//! cached to avoid an HTTP round-trip on every `build_auth_client` call.

use std::sync::Arc;

use lab_auth::sqlite::SqliteStore;
use lab_auth::types::UpstreamOauthCredentialRow;
use rmcp::transport::auth::{AuthorizationMetadata, OAuthClientConfig};
use rmcp::transport::{AuthClient, AuthorizationManager};
use tokio::sync::RwLock;
use tracing::info;

use crate::config::{UpstreamConfig, UpstreamOauthRegistration};
use crate::oauth::upstream::encryption::EncryptionKey;
use crate::oauth::upstream::refresh::RefreshLocks;
use crate::oauth::upstream::store::{SqliteCredentialStore, SqliteStateStore};
use crate::oauth::upstream::types::{BeginAuthorization, OauthError};

const TOKEN_EXPIRY_WARNING_SECS: i64 = 300;
const PROACTIVE_REFRESH_WINDOW_SECS: i64 = 30;

/// Upstream OAuth manager for a single upstream MCP server.
///
/// Cheap to clone — all mutable state is behind `Arc`.
#[derive(Clone)]
pub struct UpstreamOauthManager {
    sqlite: SqliteStore,
    key: EncryptionKey,
    upstream: UpstreamConfig,
    redirect_uri: Arc<String>,
    locks: Arc<RefreshLocks>,
    /// Cached AS metadata (fetched once per upstream, shared across subjects).
    metadata_cache: Arc<RwLock<Option<AuthorizationMetadata>>>,
}

impl UpstreamOauthManager {
    /// Create a new manager for `upstream`.
    ///
    /// `redirect_uri` is the absolute URL of the OAuth callback endpoint that will
    /// receive the authorization code (e.g.
    /// `https://lab.example/v1/upstream-oauth/{name}/callback`).
    pub fn new(
        sqlite: SqliteStore,
        key: EncryptionKey,
        upstream: UpstreamConfig,
        redirect_uri: String,
    ) -> Self {
        Self {
            sqlite,
            key,
            upstream,
            redirect_uri: Arc::new(redirect_uri),
            locks: Arc::new(RefreshLocks::new()),
            metadata_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Return the `UpstreamConfig` this manager was constructed with.
    ///
    /// Used to persist transient (probe-created) managers back into the gateway
    /// LabConfig when authorization completes for the first time.
    pub fn upstream_config(&self) -> &UpstreamConfig {
        &self.upstream
    }

    /// Return `true` if persisted credentials exist for `subject`.
    ///
    /// Does not check whether the credentials are still valid.
    #[allow(dead_code)]
    pub async fn has_credentials(&self, subject: &str) -> Result<bool, OauthError> {
        self.sqlite
            .find_upstream_oauth_credentials(&self.upstream.name, subject)
            .await
            .map(|opt| opt.is_some())
            .map_err(|e| OauthError::Internal(e.to_string()))
    }

    /// Begin the authorization flow.
    ///
    /// Discovers (or uses cached) AS metadata, registers or configures the OAuth
    /// client, generates a PKCE challenge, saves the pending state to SQLite, and
    /// returns the authorization URL to redirect the operator's browser to.
    ///
    /// Enforces S256 PKCE — returns `OauthError::UnsupportedMethod` if the AS does
    /// not advertise S256 in `code_challenge_methods_supported`.
    pub async fn begin_authorization(
        &self,
        subject: &str,
    ) -> Result<BeginAuthorization, OauthError> {
        let started = std::time::Instant::now();
        let oauth_cfg = self.oauth_config()?;
        let upstream_url = self.upstream_url()?;

        let mut manager = AuthorizationManager::new(upstream_url.as_str())
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = "internal_error",
                    error = %e,
                    "upstream oauth: failed to create authorization manager"
                );
                OauthError::Internal(format!("create auth manager: {e}"))
            })?;

        let cred_store = SqliteCredentialStore::new(
            self.sqlite.clone(),
            self.key.clone(),
            &self.upstream.name,
            subject,
        );
        let state_store = SqliteStateStore::new(self.sqlite.clone(), &self.upstream.name, subject);
        manager.set_credential_store(cred_store);
        manager.set_state_store(state_store);

        let metadata = self
            .get_or_discover_metadata(&mut manager)
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = e.kind(),
                    error = %e,
                    "upstream oauth: AS metadata discovery failed"
                );
                e
            })?;

        info!(
            upstream = %self.upstream.name,
            subject,
            issuer = metadata.issuer.as_deref().unwrap_or("<none>"),
            "upstream oauth: AS metadata ready"
        );

        self.verify_s256(&metadata.code_challenge_methods_supported)
            .inspect_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = e.kind(),
                    "upstream oauth: S256 PKCE verification failed"
                );
            })?;
        manager.set_metadata(metadata);

        let scopes: Vec<&str> = oauth_cfg
            .scopes
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(String::as_str)
            .collect();

        let client_cfg = self
            .resolve_client_config(
                &mut manager,
                subject,
                &scopes,
                DynamicClientRegistrationUse::BeginAuthorization,
            )
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = e.kind(),
                    error = %e,
                    "upstream oauth: client config resolution failed"
                );
                e
            })?;

        manager.configure_client(client_cfg).map_err(|e| {
            tracing::warn!(
                upstream = %self.upstream.name,
                subject,
                kind = "internal_error",
                error = %e,
                "upstream oauth: client configuration failed"
            );
            OauthError::Internal(format!("configure client: {e}"))
        })?;

        let authorization_url = manager.get_authorization_url(&scopes).await.map_err(|e| {
            tracing::warn!(
                upstream = %self.upstream.name,
                subject,
                kind = "internal_error",
                error = %e,
                "upstream oauth: authorization URL generation failed"
            );
            OauthError::Internal(format!("get authorization url: {e}"))
        })?;

        let _csrf = extract_state_param(&authorization_url).ok_or_else(|| {
            tracing::warn!(
                upstream = %self.upstream.name,
                subject,
                kind = "internal_error",
                "upstream oauth: authorization URL missing state parameter"
            );
            OauthError::Internal("authorization url missing required state parameter".to_string())
        })?;

        info!(
            upstream = %self.upstream.name,
            subject,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth: authorization started"
        );

        Ok(BeginAuthorization { authorization_url })
    }

    /// Complete the authorization callback.
    ///
    /// Exchanges the authorization code for tokens and persists the encrypted
    /// credentials. Completion is reconstructed from persisted PKCE state rather
    /// than an in-memory pending map, so callbacks remain valid across restarts.
    pub async fn complete_authorization_callback(
        &self,
        subject: &str,
        code: &str,
        csrf_token: &str,
    ) -> Result<(), OauthError> {
        let started = std::time::Instant::now();

        let auth_manager = self
            .configured_authorization_manager(
                subject,
                DynamicClientRegistrationUse::CompleteAuthorization,
            )
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = e.kind(),
                    error = %e,
                    "upstream oauth: failed to build configured authorization manager for token exchange"
                );
                e
            })?;

        auth_manager
            .exchange_code_for_token(code, csrf_token)
            .await
            .map_err(|e| {
                let mapped = map_auth_error(e);
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = mapped.kind(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "upstream oauth: token exchange failed"
                );
                mapped
            })?;

        info!(
            upstream = %self.upstream.name,
            subject,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth: authorization completed, tokens stored"
        );

        Ok(())
    }

    /// Delete all stored credentials for `subject` and evict any cached state.
    pub async fn clear_credentials(&self, subject: &str) -> Result<(), OauthError> {
        self.sqlite
            .delete_upstream_oauth_credentials(&self.upstream.name, subject)
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = "internal_error",
                    error = %e,
                    "upstream oauth: failed to delete credentials from store"
                );
                OauthError::Internal(e.to_string())
            })?;

        self.sqlite
            .delete_dynamic_client_registration(&self.upstream.name, subject)
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = "internal_error",
                    error = %e,
                    "upstream oauth: failed to delete dynamic client registration"
                );
                OauthError::Internal(e.to_string())
            })?;

        info!(
            upstream = %self.upstream.name,
            subject,
            "upstream oauth: credentials and dynamic registration cleared"
        );

        Ok(())
    }

    /// Return an `AuthClient` ready for use, proactively refreshing if near expiry.
    ///
    /// Creates a fresh `AuthorizationManager` backed by stored credentials.  Uses
    /// cached AS metadata to avoid an extra HTTP round-trip.
    ///
    /// Returns `OauthError::NeedsReauth` when no credentials are stored or the
    /// refresh token has been revoked.
    pub async fn build_auth_client(
        &self,
        subject: &str,
    ) -> Result<AuthClient<reqwest::Client>, OauthError> {
        let started = std::time::Instant::now();
        let lock = self.locks.acquire(&self.upstream.name, subject);
        let _guard = lock.lock().await;

        let mut manager = self
            .configured_authorization_manager(
                subject,
                DynamicClientRegistrationUse::StoredCredentials,
            )
            .await
            .inspect_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    provider = %self.oauth_provider_label(),
                    subject,
                    scope = %self.oauth_scope_label(),
                    kind = e.kind(),
                    elapsed_ms = started.elapsed().as_millis(),
                    fallback = "reauthorization_required",
                    "upstream oauth: failed to build auth client manager"
                );
            })?;
        let initialized = manager.initialize_from_store().await.map_err(|e| {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                kind = "internal_error",
                elapsed_ms = started.elapsed().as_millis(),
                fallback = "reauthorization_required",
                "upstream oauth: failed to initialize auth client from credential store"
            );
            OauthError::Internal(format!("initialize from store: {e}"))
        })?;

        if !initialized {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                kind = "oauth_needs_reauth",
                elapsed_ms = started.elapsed().as_millis(),
                fallback = "reauthorization_required",
                "upstream oauth: no stored credentials for auth client"
            );
            return Err(OauthError::NeedsReauth(format!(
                "no stored credentials for upstream '{}' subject '{subject}'",
                self.upstream.name
            )));
        }

        let credential_row = self.credential_row(subject).await?;
        let refresh_state = credential_row
            .as_ref()
            .and_then(|row| TokenRefreshState::from_row(row, now_unix().ok()?));
        if let Some(state) = refresh_state.as_ref() {
            self.log_expiring_token(subject, state, started.elapsed().as_millis());
            self.log_refresh_attempt(subject, state, started.elapsed().as_millis());
        }

        manager.get_access_token().await.map_err(|e| {
            let mapped = map_auth_error(e);
            if refresh_state
                .as_ref()
                .is_some_and(TokenRefreshState::refresh_due)
            {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    provider = %self.oauth_provider_label(),
                    subject,
                    scope = %self.oauth_scope_label(),
                    kind = mapped.kind(),
                    elapsed_ms = started.elapsed().as_millis(),
                    fallback = "reauthorization_required",
                    "upstream oauth: token refresh failed"
                );
            }
            mapped
        })?;

        if refresh_state
            .as_ref()
            .is_some_and(TokenRefreshState::refresh_due)
        {
            tracing::info!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                elapsed_ms = started.elapsed().as_millis(),
                fallback = "none",
                "upstream oauth: token refresh succeeded"
            );
        }

        Ok(AuthClient::new(reqwest::Client::new(), manager))
    }

    /// Force a refresh for stored credentials.
    ///
    /// `AuthorizationManager::get_access_token()` only refreshes inside rmcp's
    /// short refresh buffer. Status checks need an explicit refresh so UI state
    /// cannot report a stale credential row as connected.
    pub async fn refresh_auth_client(&self, subject: &str) -> Result<(), OauthError> {
        let started = std::time::Instant::now();
        let lock = self.locks.acquire(&self.upstream.name, subject);
        let _guard = lock.lock().await;

        let mut manager = self
            .configured_authorization_manager(
                subject,
                DynamicClientRegistrationUse::StoredCredentials,
            )
            .await
            .inspect_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    provider = %self.oauth_provider_label(),
                    subject,
                    scope = %self.oauth_scope_label(),
                    kind = e.kind(),
                    elapsed_ms = started.elapsed().as_millis(),
                    fallback = "reauthorization_required",
                    "upstream oauth: failed to build refresh manager"
                );
            })?;
        let initialized = manager.initialize_from_store().await.map_err(|e| {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                kind = "internal_error",
                elapsed_ms = started.elapsed().as_millis(),
                fallback = "reauthorization_required",
                "upstream oauth: failed to initialize refresh manager from credential store"
            );
            OauthError::Internal(format!("initialize from store: {e}"))
        })?;

        if !initialized {
            return Err(OauthError::NeedsReauth(format!(
                "no stored credentials for upstream '{}' subject '{subject}'",
                self.upstream.name
            )));
        }

        manager.refresh_token().await.map_err(map_auth_error)?;
        tracing::info!(
            upstream = %self.upstream.name,
            provider = %self.oauth_provider_label(),
            subject,
            scope = %self.oauth_scope_label(),
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth: status refresh succeeded"
        );
        Ok(())
    }

    pub async fn credential_row(
        &self,
        subject: &str,
    ) -> Result<Option<UpstreamOauthCredentialRow>, OauthError> {
        self.sqlite
            .find_upstream_oauth_credentials(&self.upstream.name, subject)
            .await
            .map_err(|e| OauthError::Internal(e.to_string()))
    }

    #[allow(dead_code)]
    pub async fn subject_for_state(&self, csrf_token: &str) -> Result<Option<String>, OauthError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| OauthError::Internal(format!("system clock error: {error}")))?
            .as_secs() as i64;
        self.sqlite
            .find_upstream_oauth_state_subject(&self.upstream.name, csrf_token, now)
            .await
            .map_err(|e| OauthError::Internal(e.to_string()))
    }

    // ---- private helpers ----

    async fn configured_authorization_manager(
        &self,
        subject: &str,
        dynamic_registration_use: DynamicClientRegistrationUse,
    ) -> Result<AuthorizationManager, OauthError> {
        let upstream_url = self.upstream_url()?;
        let oauth_cfg = self.oauth_config()?;

        let mut manager = AuthorizationManager::new(upstream_url.as_str())
            .await
            .map_err(|e| OauthError::Internal(format!("create auth manager: {e}")))?;

        let cred_store = SqliteCredentialStore::new(
            self.sqlite.clone(),
            self.key.clone(),
            &self.upstream.name,
            subject,
        );
        let state_store = SqliteStateStore::new(self.sqlite.clone(), &self.upstream.name, subject);
        manager.set_credential_store(cred_store);
        manager.set_state_store(state_store);

        let metadata = self.get_or_discover_metadata(&mut manager).await?;
        self.verify_s256(&metadata.code_challenge_methods_supported)?;
        manager.set_metadata(metadata);

        let scopes: Vec<&str> = oauth_cfg
            .scopes
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(String::as_str)
            .collect();
        let client_cfg = self
            .resolve_client_config(&mut manager, subject, &scopes, dynamic_registration_use)
            .await?;
        manager
            .configure_client(client_cfg)
            .map_err(|e| OauthError::Internal(format!("configure client: {e}")))?;
        Ok(manager)
    }

    fn oauth_config(&self) -> Result<&crate::config::UpstreamOauthConfig, OauthError> {
        self.upstream
            .oauth
            .as_ref()
            .ok_or_else(|| OauthError::Internal("upstream has no oauth config".to_string()))
    }

    fn oauth_scope_label(&self) -> String {
        self.upstream
            .oauth
            .as_ref()
            .and_then(|cfg| cfg.scopes.as_ref())
            .filter(|scopes| !scopes.is_empty())
            .map(|scopes| scopes.join(" "))
            .unwrap_or_else(|| "<none>".to_string())
    }

    fn oauth_provider_label(&self) -> String {
        self.upstream.name.clone()
    }

    fn log_expiring_token(&self, subject: &str, state: &TokenRefreshState, elapsed_ms: u128) {
        if state.seconds_until_expiry <= TOKEN_EXPIRY_WARNING_SECS {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                seconds_until_expiry = state.seconds_until_expiry,
                refresh_token_present = state.refresh_token_present,
                elapsed_ms,
                "upstream oauth: access token nearing expiry"
            );
        }
    }

    fn log_refresh_attempt(&self, subject: &str, state: &TokenRefreshState, elapsed_ms: u128) {
        if !state.refresh_due() {
            return;
        }

        if state.refresh_token_present {
            tracing::info!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                seconds_until_expiry = state.seconds_until_expiry,
                elapsed_ms,
                "upstream oauth: token refresh attempt"
            );
        } else {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                seconds_until_expiry = state.seconds_until_expiry,
                kind = "oauth_needs_reauth",
                elapsed_ms,
                fallback = "reauthorization_required",
                "upstream oauth: access token expired or near expiry without refresh token"
            );
        }
    }

    fn upstream_url(&self) -> Result<Arc<String>, OauthError> {
        let canonical = self
            .upstream
            .canonical_url()
            .ok_or_else(|| OauthError::Internal("upstream has no url".to_string()))?
            .map_err(|e| OauthError::Internal(format!("invalid upstream url: {e}")))?;
        Ok(Arc::new(canonical))
    }

    /// Fetch AS metadata, caching the result for subsequent calls.
    ///
    /// Enforces issuer binding per RFC 8414: `issuer` MUST be present and the
    /// `authorization_endpoint` + `token_endpoint` MUST share its origin. Rejects
    /// silent issuer drift between the first and subsequent discovery calls.
    ///
    /// Uses a single write-lock acquisition to avoid a TOCTOU race between a
    /// read-lock check and a subsequent write-lock cache update.
    async fn get_or_discover_metadata(
        &self,
        manager: &mut AuthorizationManager,
    ) -> Result<AuthorizationMetadata, OauthError> {
        let mut cache = self.metadata_cache.write().await;
        if let Some(meta) = cache.clone() {
            return Ok(meta);
        }

        let mut metadata = manager
            .discover_metadata()
            .await
            .map_err(|e| OauthError::Internal(format!("discover metadata: {e}")))?;

        if self.upstream.name == "swag" {
            metadata.authorization_endpoint = metadata
                .authorization_endpoint
                .replace("/mcp/authorize", "/authorize");
            metadata.token_endpoint = metadata.token_endpoint.replace("/mcp/token", "/token");
            if let Some(reg) = metadata.registration_endpoint.as_ref() {
                metadata.registration_endpoint = Some(reg.replace("/mcp/register", "/register"));
            }
        }

        self.verify_issuer_binding(&metadata)?;

        *cache = Some(metadata.clone());
        Ok(metadata)
    }

    /// RFC 8414 §3 issuer binding: `issuer` must be present, and every
    /// non-jwks endpoint origin (scheme + host + port) must match the
    /// issuer origin. This is stricter than a host-only check: it rejects
    /// endpoints served over a different scheme (e.g. http vs https) or
    /// on a different port, which a host-only comparison would miss.
    fn verify_issuer_binding(&self, metadata: &AuthorizationMetadata) -> Result<(), OauthError> {
        let issuer_raw = metadata.issuer.as_deref().ok_or_else(|| {
            OauthError::IssuerMismatch(format!(
                "AS metadata for upstream '{}' is missing required `issuer` claim",
                self.upstream.name
            ))
        })?;
        // Normalize the issuer: strip trailing slashes for a canonical form.
        let issuer_normalized = issuer_raw.trim_end_matches('/');
        let issuer_origin = url_origin(issuer_normalized).ok_or_else(|| {
            OauthError::IssuerMismatch(format!("issuer `{issuer_raw}` is not a valid URL"))
        })?;
        for (label, endpoint) in [
            (
                "authorization_endpoint",
                Some(metadata.authorization_endpoint.as_str()),
            ),
            ("token_endpoint", Some(metadata.token_endpoint.as_str())),
            (
                "registration_endpoint",
                metadata.registration_endpoint.as_deref(),
            ),
        ] {
            let Some(endpoint) = endpoint else { continue };
            let Some(origin) = url_origin(endpoint) else {
                return Err(OauthError::IssuerMismatch(format!(
                    "{label} `{endpoint}` is not a valid URL"
                )));
            };
            if origin != issuer_origin {
                return Err(OauthError::IssuerMismatch(format!(
                    "{label} origin `{origin}` does not match issuer origin `{issuer_origin}`"
                )));
            }
        }
        Ok(())
    }

    fn verify_s256(&self, methods: &Option<Vec<String>>) -> Result<(), OauthError> {
        match methods {
            Some(methods) if methods.iter().any(|m| m == "S256") => Ok(()),
            Some(methods) => Err(OauthError::UnsupportedMethod(format!(
                "AS does not advertise S256 PKCE; advertised methods: {methods:?}"
            ))),
            None => Err(OauthError::UnsupportedMethod(
                "AS did not advertise code_challenge_methods_supported; S256 is required"
                    .to_string(),
            )),
        }
    }

    async fn resolve_client_config(
        &self,
        manager: &mut AuthorizationManager,
        subject: &str,
        scopes: &[&str],
        dynamic_registration_use: DynamicClientRegistrationUse,
    ) -> Result<OAuthClientConfig, OauthError> {
        let oauth_cfg = self.oauth_config()?;
        match &oauth_cfg.registration {
            UpstreamOauthRegistration::Preregistered {
                client_id,
                client_secret_env,
            } => {
                let secret = match client_secret_env.as_deref() {
                    None => None,
                    Some(var) => {
                        let val = std::env::var(var).unwrap_or_default();
                        if val.is_empty() {
                            return Err(OauthError::Internal(format!(
                                "client_secret_env '{var}' is configured but env var '{var}' is not set or is empty"
                            )));
                        }
                        Some(val)
                    }
                };

                let mut cfg = OAuthClientConfig::new(client_id.clone(), self.redirect_uri.as_str());
                if let Some(s) = secret {
                    cfg = cfg.with_client_secret(s);
                }
                cfg = cfg.with_scopes(scopes.iter().map(|s| s.to_string()).collect());
                Ok(cfg)
            }
            UpstreamOauthRegistration::Dynamic => {
                // Dynamic registration (RFC 7591) has two different lifetimes:
                //   1. Stored credentials are durable and remain authoritative after
                //      a successful token exchange for normal MCP calls.
                //   2. The dynamic registration row is only pending state between
                //      begin_authorization and callback. It survives Lab restarts, but
                //      must not be reused to start a new flow because upstream AS state
                //      can be reset independently, leaving a stale client_id behind.

                match dynamic_registration_use {
                    DynamicClientRegistrationUse::StoredCredentials => {
                        if let Some(row) = self
                            .sqlite
                            .find_upstream_oauth_credentials(&self.upstream.name, subject)
                            .await
                            .map_err(|e| OauthError::Internal(e.to_string()))?
                        {
                            let mut cfg =
                                OAuthClientConfig::new(row.client_id, self.redirect_uri.as_str());
                            cfg = cfg.with_scopes(scopes.iter().map(|s| s.to_string()).collect());
                            return Ok(cfg);
                        }

                        return Err(OauthError::NeedsReauth(format!(
                            "no stored credentials for upstream '{}' subject '{subject}'",
                            self.upstream.name
                        )));
                    }
                    DynamicClientRegistrationUse::CompleteAuthorization => {
                        // Callback/token exchange path: use the client_id created
                        // by the begin_authorization call. This keeps callbacks
                        // valid across Lab process restarts and lets an explicit
                        // reauth flow replace stale stored credentials.
                        if let Some(client_id) = self
                            .sqlite
                            .find_dynamic_client_registration(&self.upstream.name, subject)
                            .await
                            .map_err(|e| OauthError::Internal(e.to_string()))?
                        {
                            let mut cfg =
                                OAuthClientConfig::new(client_id, self.redirect_uri.as_str());
                            cfg = cfg.with_scopes(scopes.iter().map(|s| s.to_string()).collect());
                            return Ok(cfg);
                        }

                        return Err(OauthError::NeedsReauth(format!(
                            "no dynamic client registration for upstream '{}' subject '{subject}'",
                            self.upstream.name
                        )));
                    }
                    DynamicClientRegistrationUse::BeginAuthorization => {}
                }

                // Beginning a new flow: register with the AS every time there are
                // no stored credentials. This self-heals when the upstream AS loses
                // its dynamic-client DB while Lab still has an old pending row.
                let cfg = manager
                    .register_client("lab", self.redirect_uri.as_str(), scopes)
                    .await
                    .map_err(|e| OauthError::Internal(format!("dynamic registration: {e}")))?;

                self.sqlite
                    .save_dynamic_client_registration(&self.upstream.name, subject, &cfg.client_id)
                    .await
                    .map_err(|e| OauthError::Internal(e.to_string()))?;

                // Read back the persisted value to use the DB-canonical client_id.
                let canonical_client_id = self
                    .sqlite
                    .find_dynamic_client_registration(&self.upstream.name, subject)
                    .await
                    .map_err(|e| OauthError::Internal(e.to_string()))?
                    .ok_or_else(|| {
                        OauthError::Internal(
                            "dynamic registration saved but read-back returned nothing".to_string(),
                        )
                    })?;

                let mut canonical_cfg =
                    OAuthClientConfig::new(canonical_client_id, self.redirect_uri.as_str());
                canonical_cfg =
                    canonical_cfg.with_scopes(scopes.iter().map(|s| s.to_string()).collect());
                Ok(canonical_cfg)
            }
            UpstreamOauthRegistration::ClientMetadataDocument { url } => {
                // Client ID Metadata Document (CIMD): the metadata-document URL
                // *is* the client identifier. No registration_endpoint call is
                // issued — the AS fetches the document itself when it first sees
                // the client_id. We construct the OAuth client locally.
                let parsed = url::Url::parse(url).map_err(|e| {
                    OauthError::Internal(format!("invalid client_metadata_document url: {e}"))
                })?;
                if parsed.scheme() != "https" {
                    return Err(OauthError::Internal(format!(
                        "client_metadata_document url must use https, got `{}`",
                        parsed.scheme()
                    )));
                }
                let cfg = OAuthClientConfig::new(url.clone(), self.redirect_uri.as_str())
                    .with_scopes(scopes.iter().map(|s| s.to_string()).collect());
                Ok(cfg)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DynamicClientRegistrationUse {
    BeginAuthorization,
    CompleteAuthorization,
    StoredCredentials,
}

/// Return the normalized origin (scheme + "://" + lowercased host + optional explicit port)
/// of a URL, or `None` if the URL is invalid or has no host.
///
/// This is stricter than a host-only comparison: it rejects URLs that share a host
/// but differ in scheme or port (e.g. http vs https, or :80 vs :8080).
fn url_origin(s: &str) -> Option<String> {
    let u = url::Url::parse(s).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    let scheme = u.scheme();
    match u.port() {
        Some(port) => Some(format!("{scheme}://{host}:{port}")),
        None => Some(format!("{scheme}://{host}")),
    }
}

fn extract_state_param(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
}

struct TokenRefreshState {
    seconds_until_expiry: i64,
    refresh_token_present: bool,
}

impl TokenRefreshState {
    fn from_row(row: &UpstreamOauthCredentialRow, now: i64) -> Option<Self> {
        if row.access_token_expires_at <= 0 {
            return None;
        }
        Some(Self {
            seconds_until_expiry: row.access_token_expires_at.saturating_sub(now),
            refresh_token_present: row.refresh_token_present,
        })
    }

    fn refresh_due(&self) -> bool {
        self.seconds_until_expiry <= PROACTIVE_REFRESH_WINDOW_SECS
    }
}

fn now_unix() -> Result<i64, OauthError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| OauthError::Internal(format!("system clock error: {error}")))
        .map(|duration| duration.as_secs() as i64)
}

fn map_auth_error(e: rmcp::transport::AuthError) -> OauthError {
    match e {
        rmcp::transport::AuthError::AuthorizationRequired => {
            OauthError::NeedsReauth("authorization required".to_string())
        }
        rmcp::transport::AuthError::TokenExchangeFailed(msg) => OauthError::Internal(msg),
        rmcp::transport::AuthError::TokenRefreshFailed(msg) => {
            OauthError::NeedsReauth(format!("token refresh failed: {msg}"))
        }
        other => OauthError::Internal(other.to_string()),
    }
}
