//! Probe helpers for `probe_upstream_oauth_for_upstream`.
//!
//! This module decomposes the probe flow into named, single-responsibility
//! helpers so the top-level orchestrator (`run`) stays under ~80 lines and the
//! two near-identical URL-conflict checks are deduplicated (Q-M4).

use url::Url;

use crate::gateway::manager::GatewayManager;
use crate::gateway::oauth::ProbeResult;
use labby_auth::upstream::manager::UpstreamOauthManager;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};
use labby_runtime::redact::redact_url;

use super::{OauthRuntime, should_use_dynamic_registration};

// ── public validators (also used by tests in the parent module) ──────────────

pub(crate) fn validate_probe_url(raw: &str) -> Result<Url, ToolError> {
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

pub(crate) fn validate_probe_upstream_name(raw: &str) -> Result<String, ToolError> {
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

pub(crate) fn probe_manager_key(parsed: &Url) -> String {
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

// ── private helpers ───────────────────────────────────────────────────────────

/// Resolve the probe identity: canonical URL, display URL, upstream name, and
/// whether the name is already persisted in the gateway config.
///
/// Returns `Err` immediately if the named upstream exists under a different URL
/// (first conflict check, deduplicated from the manager-map check below).
async fn resolve_probe_identity(
    manager: &GatewayManager,
    url: &str,
    upstream_name: Option<&str>,
) -> Result<(String, String, String, bool), ToolError> {
    let parsed = validate_probe_url(url)?;
    let canonical_url = parsed.as_str().to_string();
    let redacted_url = redact_url(&canonical_url);

    let name = match upstream_name {
        Some(name) => validate_probe_upstream_name(name)?,
        None => {
            let cfg = manager.config.read().await;
            cfg.upstream
                .iter()
                .find(|u| u.url.as_deref() == Some(canonical_url.as_str()))
                .map(|u| u.name.clone())
                .unwrap_or_else(|| probe_manager_key(&parsed))
        }
    };

    let name_is_persisted = check_name_url_conflict(manager, &name, &canonical_url).await?;
    Ok((canonical_url, redacted_url, name, name_is_persisted))
}

/// Return `true` when `name` is already in the gateway config pointing to
/// `canonical_url`, `false` when the name is not present at all, or `Err` when
/// the name exists but points to a *different* URL (conflict).
async fn check_name_url_conflict(
    manager: &GatewayManager,
    name: &str,
    canonical_url: &str,
) -> Result<bool, ToolError> {
    let cfg = manager.config.read().await;
    match cfg.upstream.iter().find(|u| u.name == name) {
        Some(existing) if existing.url.as_deref() != Some(canonical_url) => {
            Err(ToolError::InvalidParam {
                message: format!("upstream `{name}` is already configured for a different URL"),
                param: "upstream".to_string(),
            })
        }
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

/// Validate that all required OAuth runtime resources are present, and check
/// or report missing env vars with a clear error message.
fn require_oauth_runtime_with_prereq_check<'a>(
    manager: &'a GatewayManager,
    name: &str,
    started: std::time::Instant,
) -> Result<OauthRuntime<'a>, ToolError> {
    // Check each prerequisite independently so the error names only what's missing.
    if manager.oauth_key.is_none()
        || manager.oauth_sqlite.is_none()
        || manager.oauth_redirect_uri.is_none()
    {
        let missing: Vec<&str> = [
            manager
                .oauth_key
                .is_none()
                .then_some("LAB_OAUTH_ENCRYPTION_KEY"),
            manager
                .oauth_redirect_uri
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

    manager.require_oauth_runtime()
}

/// Look up the `prefer_client_metadata_document` override from the persisted
/// upstream config, if any.
async fn resolve_prefer_cimd(manager: &GatewayManager, name: &str) -> Option<bool> {
    let cfg = manager.config.read().await;
    cfg.upstream
        .iter()
        .find(|u| u.name == name)
        .and_then(|u| u.oauth.as_ref())
        .and_then(|o| o.prefer_client_metadata_document)
}

/// Register a new transient `UpstreamOauthManager` for the given upstream, or
/// evict-and-replace a stale one that points to a different URL.
///
/// Deduplicates the URL-conflict check for the manager-map path (matches the
/// same guard already performed on the persisted config in `resolve_probe_identity`,
/// but applied to the in-memory manager map which may have a stale entry).
fn register_transient_manager(
    gm: &GatewayManager,
    runtime: &OauthRuntime<'_>,
    name: &str,
    name_is_persisted: bool,
    canonical_url: &str,
    use_dynamic_registration: bool,
    prefer_cimd: Option<bool>,
    metadata: &rmcp::transport::auth::AuthorizationMetadata,
    strategy: &str,
    started: std::time::Instant,
) -> Result<(), ToolError> {
    if let Some(existing) = runtime.managers.get(name) {
        let existing_url = existing.upstream_config().url.clone();
        drop(existing);
        if existing_url.as_deref() != Some(canonical_url) {
            if name_is_persisted {
                return Err(ToolError::InvalidParam {
                    message: format!("upstream `{name}` is already configured for a different URL"),
                    param: "upstream".to_string(),
                });
            }
            runtime.managers.remove(name);
            gm.evict_upstream_clients(name);
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
            return Ok(());
        }
    }

    // Build and insert a new transient manager.
    let registration = if use_dynamic_registration {
        UpstreamOauthRegistration::Dynamic
    } else {
        // No RFC 7591 dynamic registration — use the Client ID Metadata
        // Document (CIMD) approach: the lab's own metadata-document URL
        // acts as the client_id. Derive it from the redirect_uri origin.
        let metadata_doc_url = Url::parse(runtime.redirect_uri.as_str())
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
        name: name.to_string(),
        url: Some(canonical_url.to_string()),
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
            // Propagate the operator override so that if this transient
            // config is later persisted it retains the explicit setting.
            prefer_client_metadata_document: prefer_cimd,
        }),
        imported_from: None,
        priority: 1.0,
    };

    let new_manager = UpstreamOauthManager::new(
        runtime.sqlite.clone(),
        runtime.key.clone(),
        config,
        runtime.redirect_uri.as_ref().clone(),
    );
    runtime.managers.insert(name.to_string(), new_manager);
    tracing::info!(
        service = "upstream_oauth",
        action = "probe",
        upstream = %name,
        registration_strategy = strategy,
        elapsed_ms = started.elapsed().as_millis(),
        "upstream oauth probe: transient manager registered"
    );
    Ok(())
}

// ── orchestrator ─────────────────────────────────────────────────────────────

/// Top-level probe entry point. Delegates to named helpers and is intentionally
/// kept short so the overall flow is easy to follow.
pub(crate) async fn run(
    manager: &GatewayManager,
    url: &str,
    upstream_name: Option<&str>,
) -> Result<ProbeResult, ToolError> {
    let started = std::time::Instant::now();

    let (canonical_url, redacted_url, name, name_is_persisted) =
        resolve_probe_identity(manager, url, upstream_name).await?;

    crate::security::ssrf::validate_external_https_url(&canonical_url)
        .await
        .inspect_err(|_| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "probe",
                url = %redacted_url,
                kind = "ssrf_blocked",
                "upstream oauth probe: SSRF validation task error"
            );
        })?;

    tracing::info!(
        service = "upstream_oauth",
        action = "probe",
        upstream = %name,
        url = %redacted_url,
        "upstream oauth probe: connecting"
    );

    let auth_manager = rmcp::transport::AuthorizationManager::new(&canonical_url)
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
            return Ok(ProbeResult {
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

    let prefer_cimd = resolve_prefer_cimd(manager, &name).await;
    let supports_dynamic = metadata.registration_endpoint.is_some();
    let use_dynamic_registration =
        should_use_dynamic_registration(&name, supports_dynamic, prefer_cimd);
    let strategy = if use_dynamic_registration {
        "dynamic"
    } else {
        "client_metadata_document"
    };

    let runtime = require_oauth_runtime_with_prereq_check(manager, &name, started)?;

    register_transient_manager(
        manager,
        &runtime,
        &name,
        name_is_persisted,
        &canonical_url,
        use_dynamic_registration,
        prefer_cimd,
        &metadata,
        strategy,
        started,
    )?;

    Ok(ProbeResult {
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
