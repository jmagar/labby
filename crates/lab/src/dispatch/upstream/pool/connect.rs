//! Connection establishment for HTTP and WebSocket upstreams.
//!
//! `connect_upstream` is the transport-dispatching entry point; it delegates to
//! `connect_http_upstream`, `connect_websocket_upstream`, or (in
//! `connect_stdio.rs`) the stdio/in-process connectors. These free functions are
//! `pub(super)` so the pool module and the sibling `connect_stdio` module can
//! call them across the module boundary.

use std::time::Instant;

use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use rmcp::{RoleClient, ServiceExt};

use crate::config::UpstreamConfig;
use crate::oauth::upstream::cache::OauthClientCache;

use super::super::auth::{configured_bearer_token, websocket_authorization_header};
use super::super::http_client;
use super::super::transport::websocket::{
    WebSocketTransportConfig, connect as connect_websocket_transport, parse_ws_url,
};
use super::super::types::{UpstreamRuntimeMetadata, UpstreamRuntimeOwner};
use super::UpstreamConnection;
use super::connect_stdio::connect_stdio_upstream;
use super::helpers::{
    DEFAULT_REQUEST_TIMEOUT, is_websocket_url, max_response_bytes, upstream_target_redacted,
    upstream_transport,
};

pub(super) async fn connect_upstream(
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    let started = Instant::now();
    tracing::debug!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.connect",
        event = "attempt",
        operation = "connection.acquire",
        upstream = %config.name,
        transport = upstream_transport(config),
        target = %upstream_target_redacted(config),
        subject_scoped = subject.is_some(),
        "upstream connection acquire attempt"
    );
    let result = if let Some(ref url) = config.url {
        if is_websocket_url(url) {
            connect_websocket_upstream(url, config).await
        } else {
            connect_http_upstream(url, config, subject, oauth_client_cache).await
        }
    } else if let Some(ref command) = config.command {
        connect_stdio_upstream(command, &config.args, config, runtime_origin, runtime_owner).await
    } else {
        Err(anyhow::anyhow!(
            "upstream {} has neither url nor command",
            config.name
        ))
    };
    match &result {
        Ok((_, tools)) => tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.connect",
            event = "finish",
            operation = "connection.acquire",
            upstream = %config.name,
            transport = upstream_transport(config),
            target = %upstream_target_redacted(config),
            subject_scoped = subject.is_some(),
            tool_count = tools.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "upstream connection acquire finish"
        ),
        Err(error) => tracing::warn!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.connect",
            event = "error",
            operation = "connection.acquire",
            upstream = %config.name,
            transport = upstream_transport(config),
            target = %upstream_target_redacted(config),
            subject_scoped = subject.is_some(),
            kind = "upstream_connect_error",
            error = %error,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream connection acquire error"
        ),
    }
    result
}

pub(super) async fn connect_websocket_upstream(
    url: &str,
    config: &UpstreamConfig,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "websocket",
        action = "upstream.connect.start", target = %upstream_target_redacted(config),
        "upstream connect start",
    );
    if config.oauth.is_some() {
        anyhow::bail!(
            "upstream {} declares oauth, but websocket upstream oauth is not yet supported",
            config.name
        );
    }

    let parsed = parse_ws_url(url).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let authorization = websocket_authorization_header(config);
    let transport = connect_websocket_transport(
        WebSocketTransportConfig::new(parsed.to_string()).with_authorization(authorization),
    );
    let service: rmcp::service::RunningService<RoleClient, ()> = ().serve(transport).await?;
    let peer = service.peer().clone();
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "websocket",
        action = "upstream.connect.finish", tool_count = tools.len(),
        "upstream connect finish",
    );
    Ok((
        UpstreamConnection {
            _client_service: service,
            _server_task: None,
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
        tools,
    ))
}

pub(super) fn stable_jitter_seed(name: &str, attempt: u32) -> u64 {
    let mut hash = 1_469_598_103_934_665_603_u64;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash ^ u64::from(attempt)
}

/// Log the "OAuth upstream not yet capped" warning at most once per
/// upstream name per process. The OAuth path runs on every reprobe/
/// reconnect, so an unconditional WARN floods logs at REPROBE_INTERVAL
/// (30s) × N upstreams cadence. This dedup keeps the gap visible without
/// drowning out real warnings.
fn log_oauth_uncapped_once(upstream_name: &str) {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static LOGGED: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let set = LOGGED.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    let mut guard = match set.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.insert(upstream_name.to_string()) {
        tracing::warn!(
            surface = "dispatch",
            service = "upstream.pool",
            upstream = %upstream_name,
            "oauth http upstream: response body cap not yet applied (follow-up to lab-4z8sx.2)"
        );
    }
}

/// Connect to an HTTP upstream MCP server.
pub(super) async fn connect_http_upstream(
    url: &str,
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "http",
        action = "upstream.connect.start", target = %upstream_target_redacted(config),
        "upstream connect start",
    );
    let transport_config = StreamableHttpClientTransportConfig::with_uri(url);

    // OAuth path: when the upstream declares oauth config, build an AuthClient.
    if config.oauth.is_some() {
        let subject = subject.ok_or_else(|| {
            anyhow::anyhow!(
                "upstream {} requires an authenticated subject; discovery must be request-scoped",
                config.name
            )
        })?;
        let cache = oauth_client_cache.ok_or_else(|| {
            anyhow::anyhow!(
                "upstream {} requires OAuth but no auth client cache is registered",
                config.name
            )
        })?;

        let auth_client = cache
            .get_or_build(config, subject)
            .await
            .map_err(|e| anyhow::anyhow!("oauth_required: {e}"))?;

        // TODO(follow-up to lab-4z8sx.2): the OAuth path does NOT get the
        // BodyCappedHttpClient cap because `OauthClientCache` returns a
        // concrete `AuthClient<reqwest::Client>` and AuthClient is
        // `#[non_exhaustive]` (no way to swap its inner http_client type).
        // Threading `BodyCappedHttpClient` through the cache requires
        // changing the cache to build `AuthClient<BodyCappedHttpClient>` end
        // to end. The non-OAuth path (below) is capped.
        log_oauth_uncapped_once(&config.name);
        let worker = StreamableHttpClientWorker::new((*auth_client).clone(), transport_config);
        let service: rmcp::service::RunningService<RoleClient, ()> = ().serve(worker).await?;
        let peer = service.peer().clone();
        let tools = peer.list_all_tools().await?;
        return Ok((
            UpstreamConnection {
                _client_service: service,
                _server_task: None,
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
            },
            tools,
        ));
    }

    // Non-OAuth path: optionally inject a static bearer token from env.
    let mut transport_config = transport_config;
    if let Some(ref env_name) = config.bearer_token_env {
        if let Some(token) = configured_bearer_token(env_name) {
            transport_config.auth_header = Some(token);
        } else {
            tracing::warn!(
                upstream = %config.name,
                env_var = %env_name,
                "bearer_token_env configured but env var not set"
            );
        }
    }

    let client = reqwest::Client::builder()
        .timeout(DEFAULT_REQUEST_TIMEOUT)
        .build()?;
    // Wrap reqwest::Client in BodyCappedHttpClient so a hostile upstream
    // cannot OOM the gateway via an oversized response. Cap is enforced
    // during streaming (Content-Length + bytes_stream count). For SSE, the
    // cap is per-event so legitimate long-lived subscriptions are not
    // disconnected.
    let capped = http_client::BodyCappedHttpClient::new(client, max_response_bytes());
    let worker = StreamableHttpClientWorker::new(capped, transport_config);
    let service: rmcp::service::RunningService<RoleClient, ()> = ().serve(worker).await?;
    let peer = service.peer().clone();
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "http",
        action = "upstream.connect.finish", tool_count = tools.len(),
        "upstream connect finish",
    );

    Ok((
        UpstreamConnection {
            _client_service: service,
            _server_task: None,
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
        tools,
    ))
}

pub(super) fn runtime_origin_label(
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
) -> Option<String> {
    if let Some(raw) = runtime_owner
        .and_then(|owner| owner.raw.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(raw.to_string());
    }

    if let Some(origin) = runtime_origin
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(origin.to_string());
    }

    for (prefix, session_key) in [
        ("claude-code", "CLAUDE_SESSION_ID"),
        ("codex", "CODEX_SESSION_ID"),
    ] {
        if let Ok(session) = std::env::var(session_key) {
            let trimmed = session.trim();
            if !trimmed.is_empty() {
                return Some(format!("{prefix}:{trimmed}"));
            }
        }
    }

    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        let trimmed = term_program.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    Some("gateway-managed".to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::UpstreamPool;
    use super::*;
    use crate::config::{UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration};

    fn oauth_http_config() -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: "oauth-upstream".into(),
            url: Some("http://127.0.0.1:8080/mcp".into()),
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
                registration: UpstreamOauthRegistration::Preregistered {
                    client_id: "client-id".into(),
                    client_secret_env: None,
                },
                scopes: None,
            }),
            imported_from: None,
            priority: 1.0,
            tool_search: crate::config::ToolSearchConfig::default(),
        }
    }

    #[tokio::test]
    async fn subject_scoped_upstream_requires_authenticated_subject_for_oauth_http_connect() {
        let config = oauth_http_config();
        let error = connect_http_upstream(
            config.url.as_deref().expect("url"),
            &config,
            None,
            Some(&OauthClientCache::new(Arc::new(dashmap::DashMap::new()))),
        )
        .await
        .expect_err("missing subject should fail");

        assert!(
            error
                .to_string()
                .contains("requires an authenticated subject")
        );
    }

    #[tokio::test]
    async fn subject_scoped_upstream_requires_registered_cache_for_oauth_http_connect() {
        let config = oauth_http_config();
        let error = connect_http_upstream(
            config.url.as_deref().expect("url"),
            &config,
            Some("alice"),
            None,
        )
        .await
        .expect_err("missing cache should fail");

        assert!(
            error
                .to_string()
                .contains("no auth client cache is registered")
        );
    }

    #[tokio::test]
    async fn shared_discovery_skips_oauth_http_upstreams() {
        let pool = UpstreamPool::new()
            .with_oauth_client_cache(OauthClientCache::new(Arc::new(dashmap::DashMap::new())));
        pool.discover_all(&[oauth_http_config()]).await;

        assert_eq!(pool.upstream_count().await, 0);
        assert!(pool.upstream_status().await.is_empty());
    }
}
