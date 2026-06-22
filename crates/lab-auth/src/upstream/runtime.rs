use std::sync::Arc;

use anyhow::{Context, Result};
use lab_runtime::gateway_config::UpstreamConfig;

use crate::config::AuthConfig;
use crate::sqlite::SqliteStore;
use crate::upstream::cache::OauthClientCache;
use crate::upstream::encryption::{EncryptionKey, load_key};
use crate::upstream::manager::UpstreamOauthManager;

pub struct UpstreamOauthRuntime {
    pub managers: Arc<dashmap::DashMap<String, UpstreamOauthManager>>,
    pub cache: OauthClientCache,
    pub sqlite: SqliteStore,
    pub key: EncryptionKey,
    pub redirect_uri: String,
}

/// Build the upstream OAuth runtime for the upstreams that declare an `oauth`
/// block.
///
/// Takes the upstream slice directly rather than a whole `LabConfig` so this
/// runtime stays decoupled from the product binary's config type: `lab-auth`
/// reads only the upstream list, never the rest of `LabConfig`.
pub async fn build_upstream_oauth_runtime(
    upstreams: &[UpstreamConfig],
    auth_config: &AuthConfig,
) -> Result<Option<UpstreamOauthRuntime>> {
    let Some(public_url) = auth_config.public_url.as_ref() else {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "oauth.runtime.disabled",
            "upstream oauth runtime disabled because no public url is configured"
        );
        return Ok(None);
    };
    let Ok(encryption_key_raw) = std::env::var("LAB_OAUTH_ENCRYPTION_KEY") else {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "oauth.runtime.disabled",
            "upstream oauth runtime disabled because LAB_OAUTH_ENCRYPTION_KEY is unset"
        );
        return Ok(None);
    };
    anyhow::ensure!(
        public_url.scheme() == "https",
        "LAB_PUBLIC_URL must be absolute https:// when upstream oauth is configured"
    );
    let key = load_key(&encryption_key_raw)
        .map_err(|error| anyhow::anyhow!("invalid LAB_OAUTH_ENCRYPTION_KEY: {error}"))?;
    let sqlite = SqliteStore::open(auth_config.sqlite_path.clone())
        .await
        .context("open sqlite store for upstream oauth")?;
    let redirect_uri = build_upstream_oauth_callback_uri(public_url)?;

    Ok(Some(build_upstream_oauth_runtime_from_parts(
        upstreams,
        sqlite,
        key,
        redirect_uri,
    )))
}

/// Assemble the runtime from pre-loaded parts.
///
/// `upstreams` is the upstream slice (decoupled from `LabConfig`); only the
/// entries with an `oauth` block get a manager.
pub fn build_upstream_oauth_runtime_from_parts(
    upstreams: &[UpstreamConfig],
    sqlite: SqliteStore,
    key: EncryptionKey,
    redirect_uri: String,
) -> UpstreamOauthRuntime {
    let managers = Arc::new(dashmap::DashMap::new());
    for upstream in upstreams.iter().filter(|upstream| upstream.oauth.is_some()) {
        managers.insert(
            upstream.name.clone(),
            UpstreamOauthManager::new(
                sqlite.clone(),
                key.clone(),
                upstream.clone(),
                redirect_uri.clone(),
            ),
        );
    }
    let cache = OauthClientCache::new(Arc::clone(&managers));
    tracing::info!(
        subsystem = "gateway_client",
        phase = "oauth.runtime.ready",
        oauth_upstream_count = managers.len(),
        "upstream oauth runtime initialized"
    );
    UpstreamOauthRuntime {
        managers,
        cache,
        sqlite,
        key,
        redirect_uri,
    }
}

pub fn build_upstream_oauth_callback_uri(public_url: &url::Url) -> Result<String> {
    let mut redirect_uri = public_url.clone();
    let base_path = redirect_uri.path().trim_end_matches('/');
    let next_path = if base_path.is_empty() {
        "/auth/upstream/callback".to_string()
    } else {
        format!("{base_path}/auth/upstream/callback")
    };
    redirect_uri.set_path(&next_path);
    redirect_uri.set_query(None);
    redirect_uri.set_fragment(None);
    Ok(redirect_uri.to_string())
}
