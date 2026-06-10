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

/// Connect to an upstream MCP server, optionally reusing a caller-supplied
/// `reqwest::Client` for HTTP connections (P-M10).
///
/// When `shared_client` is `Some`, that client is used as the base HTTP
/// transport (non-OAuth) or as the inner client for the OAuth path.  When
/// `None` the function falls back to building a fresh client, preserving the
/// pre-P-M10 behaviour.
pub(super) async fn connect_upstream_with_client(
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
    shared_client: Option<&reqwest::Client>,
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
            connect_http_upstream(url, config, subject, oauth_client_cache, shared_client).await
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

pub(super) async fn connect_upstream(
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    connect_upstream_with_client(
        config,
        subject,
        oauth_client_cache,
        runtime_origin,
        runtime_owner,
        None,
    )
    .await
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

/// Connect to an HTTP upstream MCP server.
///
/// `shared_client` is an optional caller-supplied `reqwest::Client` to reuse
/// for connection-pooling and TLS session reuse (P-M10).  When `None` a fresh
/// client is built.  Both the OAuth and non-OAuth paths wrap the base client in
/// `BodyCappedHttpClient` so the response-size cap (P-H4) is always applied.
pub(super) async fn connect_http_upstream(
    url: &str,
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    shared_client: Option<&reqwest::Client>,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "http",
        action = "upstream.connect.start", target = %upstream_target_redacted(config),
        "upstream connect start",
    );
    let transport_config = StreamableHttpClientTransportConfig::with_uri(url);

    // Resolve base HTTP client: reuse the pool-level shared client when
    // available, otherwise build a fresh one (backward-compatible fallback).
    let base_client = if let Some(c) = shared_client {
        c.clone()
    } else {
        reqwest::Client::builder()
            .timeout(DEFAULT_REQUEST_TIMEOUT)
            .build()?
    };

    // Wrap in BodyCappedHttpClient so both the OAuth and non-OAuth paths
    // enforce the streaming response-size cap (P-H4).
    let capped = http_client::BodyCappedHttpClient::new(base_client, max_response_bytes());

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
            .get_or_build_capped(config, subject, capped)
            .await
            .map_err(|e| anyhow::anyhow!("oauth_required: {e}"))?;

        let worker = StreamableHttpClientWorker::new(auth_client, transport_config);
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

    // `capped` is already built above with the shared/fresh base client.
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
            None,
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
