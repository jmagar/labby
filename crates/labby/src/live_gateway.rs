//! Detect and connect to an already-running `labby serve` daemon.
//!
//! `labby` has three surfaces that can each run as their own process: the CLI
//! (one-shot commands), the MCP stdio transport, and the HTTP daemon. Only
//! the HTTP daemon is meant to be the canonical, long-running gateway --
//! everything else should be a thin client to it whenever one is reachable,
//! rather than spinning up its own independent `GatewayManager` with its own
//! config view, upstream connections, and OAuth state. The WebUI never hits
//! this problem because it's served *by* the live daemon and shares its
//! manager directly; every other surface has to detect the daemon for
//! itself, which is what this module does.
//!
//! An invocation-scoped `CLAUDE_PLUGIN_OPTION_SERVER_URL`, then an operator-set
//! `LABBY_SERVER_URL`, is authoritative and fails closed. Without either one,
//! detection is opportunistic: it tries the local bind address first, then the
//! gateway's configured public URLs (`LABBY_MCP_GATEWAY_URL`,
//! `LABBY_PUBLIC_URL`). Only exhaustion of that bounded candidate walk permits
//! standalone local behavior.

use std::collections::BTreeSet;
use std::future::Future;
use std::time::{Duration, Instant};

use futures::StreamExt;
use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::Value;
use url::{Host, Url};

use crate::config::LabConfig;
use crate::dispatch::error::ToolError;

/// Timeout for the initial reachability probe. This runs on every thin-client
/// startup, so an unreachable host must fail over quickly rather than hang.
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);
/// Bound the complete best-effort candidate walk before standalone fallback.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
/// Bound MCP initialize and short-lived Code Mode calls, not session lifetime.
const MCP_INITIALIZATION_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound remote Code Mode execution independently from MCP initialization.
const CODEMODE_EXECUTION_TIMEOUT: Duration = Duration::from_mins(2);
const MCP_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
/// Bound the discovery catalog before parsing untrusted remote JSON.
const MAX_ACTION_CATALOG_BYTES: usize = 1024 * 1024;
/// Bound the public identity document before parsing untrusted remote JSON.
const MAX_DISCOVERY_DOCUMENT_BYTES: usize = 64 * 1024;
const MAX_OAUTH_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_DISPATCH_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const RESPONSE_BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Bound ordinary thin-client dispatches after the short reachability probe.
/// Long-poll actions opt out explicitly in `dispatch_timeout_for_action`.
const DEFAULT_DISPATCH_TIMEOUT: Duration = Duration::from_secs(30);

/// A reachable, already-running `labby serve` daemon.
#[derive(Clone)]
pub struct LiveGateway {
    base_url: Url,
    explicit: bool,
    source: &'static str,
    token: Option<String>,
    client: reqwest::Client,
    dispatch_timeout: Duration,
    actions: Option<BTreeSet<String>>,
}

#[derive(Clone, Debug)]
enum TargetSet {
    Explicit {
        base_url: Url,
        source: ExplicitSource,
    },
    Opportunistic(Vec<Url>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExplicitSource {
    Plugin,
    Operator,
}

impl ExplicitSource {
    const fn name(self) -> &'static str {
        match self {
            Self::Plugin => "CLAUDE_PLUGIN_OPTION_SERVER_URL",
            Self::Operator => "LABBY_SERVER_URL",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ProbeStage {
    Health,
    Identity,
    Actions,
}

#[derive(Debug)]
struct ProbeFailure {
    stage: ProbeStage,
    status: Option<reqwest::StatusCode>,
    kind: &'static str,
    message: String,
}

/// Pure resolution logic, split out from `candidate_base_urls` so it's
/// testable without mutating process-global env vars (which would race with
/// other tests in the same binary).
fn resolve_target_set_from(
    plugin_url: Option<&str>,
    server_url: Option<&str>,
    host_env: Option<String>,
    port_env: Option<String>,
    config: &LabConfig,
) -> Result<TargetSet, ToolError> {
    for (source, value) in [
        (ExplicitSource::Plugin, plugin_url),
        (ExplicitSource::Operator, server_url),
    ] {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            return Ok(TargetSet::Explicit {
                base_url: normalize_explicit_target(value)?,
                source,
            });
        }
    }

    let host = host_env
        .or_else(|| config.mcp.host.clone())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = port_env
        .and_then(|value| value.parse::<u16>().ok())
        .or(config.mcp.port)
        .unwrap_or(8765);

    let mut candidates = Vec::new();
    push_candidate(&mut candidates, &format!("http://{host}:{port}"));
    let public = config.public_urls();
    for raw in [public.mcp_gateway, public.app].into_iter().flatten() {
        push_candidate(&mut candidates, &raw);
    }
    Ok(TargetSet::Opportunistic(candidates))
}

fn push_candidate(candidates: &mut Vec<Url>, raw: &str) {
    if let Ok(url) = normalize_base_url(raw)
        && !candidates.contains(&url)
    {
        candidates.push(url);
    }
}

fn normalize_explicit_target(raw: &str) -> Result<Url, ToolError> {
    let url = Url::parse(raw.trim()).map_err(|_| invalid_target("invalid URL"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid_target("userinfo is not allowed"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid_target(
            "query strings and fragments are not allowed",
        ));
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_url(&url) => {}
        "http" => {
            return Err(invalid_target(
                "plaintext HTTP is allowed only for loopback",
            ));
        }
        _ => return Err(invalid_target("scheme must be https or loopback http")),
    }
    normalize_base_url(url.as_str()).map_err(|_| invalid_target("invalid base path"))
}

fn normalize_base_url(raw: &str) -> Result<Url, url::ParseError> {
    let mut url = Url::parse(raw.trim())?;
    let path = url.path().trim_end_matches('/');
    let base_path = path.strip_suffix("/mcp").unwrap_or(path);
    let normalized_path = if base_path.is_empty() {
        "/".to_string()
    } else {
        format!("{base_path}/")
    };
    url.set_path(&normalized_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn invalid_target(reason: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("configured Labby server URL is invalid: {reason}"),
    }
}

fn normalize_token(token: Option<String>) -> Option<String> {
    token.filter(|value| !value.is_empty())
}

/// Probe candidate base URLs in order and return a client for the first
/// reachable one.
///
/// `Ok(None)` means only that bounded opportunistic discovery exhausted every
/// candidate, so callers may run standalone for bootstrap compatibility.
/// Invalid or failed explicit targets return `Err` and suppress local fallback.
pub async fn detect(
    config: &LabConfig,
    surface: &'static str,
) -> Result<Option<LiveGateway>, ToolError> {
    let plugin_url = std::env::var("CLAUDE_PLUGIN_OPTION_SERVER_URL").ok();
    let server_url = std::env::var("LABBY_SERVER_URL").ok();
    let explicit_source = explicit_source_from(plugin_url.as_deref(), server_url.as_deref());
    let targets = match resolve_target_set_from(
        plugin_url.as_deref(),
        server_url.as_deref(),
        std::env::var("LABBY_MCP_HTTP_HOST").ok(),
        std::env::var("LABBY_MCP_HTTP_PORT").ok(),
        config,
    ) {
        Ok(targets) => targets,
        Err(error) => {
            tracing::warn!(
                surface,
                service = "gateway",
                action = "remote.detect",
                source = explicit_source
                    .map(ExplicitSource::name)
                    .unwrap_or("unknown"),
                kind = error.kind(),
                fallback_suppressed = true,
                "explicit remote gateway target is invalid"
            );
            return Err(error);
        }
    };
    let token = token_for_target_from(
        &targets,
        std::env::var("CLAUDE_PLUGIN_OPTION_API_TOKEN").ok(),
        std::env::var("LABBY_MCP_HTTP_TOKEN").ok(),
    );
    detect_targets(targets, token, DISCOVERY_TIMEOUT, surface).await
}

fn token_for_target_from(
    targets: &TargetSet,
    plugin_token: Option<String>,
    product_token: Option<String>,
) -> Option<String> {
    let token = match targets {
        TargetSet::Explicit {
            source: ExplicitSource::Plugin,
            ..
        } => plugin_token,
        TargetSet::Explicit {
            source: ExplicitSource::Operator,
            ..
        }
        | TargetSet::Opportunistic(_) => product_token,
    };
    normalize_token(token)
}

fn explicit_source_from(
    plugin_url: Option<&str>,
    server_url: Option<&str>,
) -> Option<ExplicitSource> {
    if plugin_url.is_some_and(|value| !value.trim().is_empty()) {
        Some(ExplicitSource::Plugin)
    } else if server_url.is_some_and(|value| !value.trim().is_empty()) {
        Some(ExplicitSource::Operator)
    } else {
        None
    }
}

async fn detect_targets(
    targets: TargetSet,
    token: Option<String>,
    discovery_timeout: Duration,
    surface: &'static str,
) -> Result<Option<LiveGateway>, ToolError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| ToolError::Sdk {
            sdk_kind: "service_unavailable".to_string(),
            message: format!("remote Labby client initialization failed: {error}"),
        })?;
    match targets {
        TargetSet::Explicit { base_url, source } => {
            let started = Instant::now();
            match probe_target(&client, &base_url, token.as_deref(), true).await {
                Ok(actions) => Ok(Some(LiveGateway::new(
                    base_url,
                    true,
                    source.name(),
                    token,
                    client,
                    actions,
                ))),
                Err(failure) => {
                    let error = probe_error(&base_url, &failure, true);
                    tracing::warn!(
                        surface,
                        service = "gateway",
                        action = "remote.detect",
                        source = source.name(),
                        origin = %sanitized_origin(&base_url),
                        elapsed_ms = started.elapsed().as_millis(),
                        kind = error.kind(),
                        stage = ?failure.stage,
                        status = ?failure.status,
                        fallback_suppressed = true,
                        "explicit remote gateway detection failed"
                    );
                    Err(error)
                }
            }
        }
        TargetSet::Opportunistic(candidates) => {
            let future = async {
                for base_url in candidates {
                    if let Ok(actions) =
                        probe_target(&client, &base_url, token.as_deref(), false).await
                    {
                        return Some(LiveGateway::new(
                            base_url,
                            false,
                            "opportunistic",
                            token.clone(),
                            client.clone(),
                            actions,
                        ));
                    }
                }
                None
            };
            Ok(tokio::time::timeout(discovery_timeout, future)
                .await
                .unwrap_or(None))
        }
    }
}

async fn probe_target(
    client: &reqwest::Client,
    base_url: &Url,
    token: Option<&str>,
    explicit: bool,
) -> Result<Option<BTreeSet<String>>, ProbeFailure> {
    let health = client
        .get(base_url.join("health").expect("validated base URL joins"))
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .map_err(|error| probe_transport_error(ProbeStage::Health, error))?;
    if !health.status().is_success() {
        return Err(probe_status_error(
            ProbeStage::Health,
            health.status(),
            false,
        ));
    }

    if token.is_none() && labby_discovery_identifies_daemon(client, base_url).await {
        return match fetch_actions(client, base_url, None).await {
            Ok(actions) => Ok(Some(actions)),
            Err(_) if !explicit => Ok(None),
            Err(failure) => Err(failure),
        };
    }

    let actions = fetch_actions(client, base_url, token).await?;
    if actions.contains("gateway.reload") {
        Ok(Some(actions))
    } else {
        Err(ProbeFailure {
            stage: ProbeStage::Identity,
            status: None,
            kind: "service_unavailable",
            message: "endpoint is not a compatible Labby daemon".to_string(),
        })
    }
}

async fn fetch_actions(
    client: &reqwest::Client,
    base_url: &Url,
    token: Option<&str>,
) -> Result<BTreeSet<String>, ProbeFailure> {
    let mut request = client
        .get(
            base_url
                .join("v1/gateway/actions")
                .expect("validated base URL joins"),
        )
        .timeout(PROBE_TIMEOUT);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|error| probe_transport_error(ProbeStage::Actions, error))?;
    if !response.status().is_success() {
        return Err(probe_status_error(
            ProbeStage::Actions,
            response.status(),
            token.is_some(),
        ));
    }
    let body = read_action_catalog_body(response).await?;
    let actions = serde_json::from_slice::<Vec<Value>>(&body).map_err(|_| ProbeFailure {
        stage: ProbeStage::Actions,
        status: None,
        kind: "service_unavailable",
        message: "gateway actions response is invalid".to_string(),
    })?;
    let mut names = BTreeSet::new();
    for action in actions {
        let Some(name) = action
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
        else {
            return Err(ProbeFailure {
                stage: ProbeStage::Actions,
                status: None,
                kind: "service_unavailable",
                message: "gateway actions response is invalid".to_string(),
            });
        };
        names.insert(name.to_string());
    }
    Ok(names)
}

async fn read_action_catalog_body(response: reqwest::Response) -> Result<Vec<u8>, ProbeFailure> {
    let too_large = || ProbeFailure {
        stage: ProbeStage::Actions,
        status: None,
        kind: "service_unavailable",
        message: format!(
            "gateway actions response exceeds the {MAX_ACTION_CATALOG_BYTES} byte limit"
        ),
    };
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ACTION_CATALOG_BYTES as u64)
    {
        return Err(too_large());
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ProbeFailure {
            stage: ProbeStage::Actions,
            status: None,
            kind: "service_unavailable",
            message: "gateway actions response could not be read".to_string(),
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_ACTION_CATALOG_BYTES {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn labby_discovery_identifies_daemon(client: &reqwest::Client, base_url: &Url) -> bool {
    let Ok(response) = client
        .get(
            base_url
                .join(".well-known/labby.json")
                .expect("validated base URL joins"),
        )
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(body) = read_bounded_body(response, MAX_DISCOVERY_DOCUMENT_BYTES).await else {
        return false;
    };
    let Ok(discovery) = serde_json::from_slice::<Value>(&body) else {
        return false;
    };
    discovery
        .get("paletteCatalogUrl")
        .and_then(Value::as_str)
        .is_some()
        && discovery
            .get("paletteExecuteUrl")
            .and_then(Value::as_str)
            .is_some()
}

async fn read_bounded_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, ()> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(());
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ())?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn read_tool_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    limit: usize,
    label: &str,
    sdk_kind: &str,
) -> Result<T, ToolError> {
    let body = read_tool_body(response, limit, label, sdk_kind).await?;
    serde_json::from_slice(&body).map_err(|error| ToolError::Sdk {
        sdk_kind: sdk_kind.to_string(),
        message: format!("{label} is invalid: {error}"),
    })
}

async fn read_tool_body(
    response: reqwest::Response,
    limit: usize,
    label: &str,
    sdk_kind: &str,
) -> Result<Vec<u8>, ToolError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(ToolError::Sdk {
            sdk_kind: "response_too_large".to_string(),
            message: format!("{label} exceeds the {limit} byte limit"),
        });
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::time::timeout(RESPONSE_BODY_IDLE_TIMEOUT, stream.next())
            .await
            .map_err(|_| ToolError::Sdk {
                sdk_kind: sdk_kind.to_string(),
                message: format!("{label} body read timed out"),
            })?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|error| ToolError::Sdk {
            sdk_kind: sdk_kind.to_string(),
            message: format!("{label} body could not be read: {error}"),
        })?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(ToolError::Sdk {
                sdk_kind: "response_too_large".to_string(),
                message: format!("{label} exceeds the {limit} byte limit"),
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn probe_transport_error(stage: ProbeStage, error: reqwest::Error) -> ProbeFailure {
    ProbeFailure {
        stage,
        status: None,
        kind: "service_unavailable",
        message: if error.is_timeout() {
            "request timed out".to_string()
        } else {
            "request failed".to_string()
        },
    }
}

fn probe_status_error(
    stage: ProbeStage,
    status: reqwest::StatusCode,
    credentials_supplied: bool,
) -> ProbeFailure {
    let kind = match status.as_u16() {
        401 if credentials_supplied => "auth_failed",
        401 => "auth_required",
        403 => "forbidden",
        _ => "service_unavailable",
    };
    ProbeFailure {
        stage,
        status: Some(status),
        kind,
        message: format!("daemon probe returned HTTP {status}"),
    }
}

fn probe_error(base_url: &Url, failure: &ProbeFailure, explicit: bool) -> ToolError {
    let suppression = if explicit {
        "; local fallback suppressed"
    } else {
        ""
    };
    ToolError::Sdk {
        sdk_kind: failure.kind.to_string(),
        message: format!(
            "configured Labby server {} failed during {:?}: {}{suppression}",
            sanitized_origin(base_url),
            failure.stage,
            failure.message
        ),
    }
}

fn sanitized_origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

#[cfg(test)]
fn candidate_base_urls_from(
    host_env: Option<String>,
    port_env: Option<String>,
    config: &LabConfig,
) -> Vec<String> {
    match resolve_target_set_from(None, None, host_env, port_env, config).unwrap() {
        TargetSet::Opportunistic(urls) => urls
            .into_iter()
            .map(|url| url.as_str().trim_end_matches('/').to_string())
            .collect(),
        TargetSet::Explicit { .. } => unreachable!("no explicit values supplied"),
    }
}

#[cfg(test)]
async fn is_labby_gateway_daemon(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
) -> bool {
    let Ok(base_url) = normalize_base_url(base_url) else {
        return false;
    };
    probe_target(client, &base_url, token, false).await.is_ok()
}

impl LiveGateway {
    fn new(
        base_url: Url,
        explicit: bool,
        source: &'static str,
        token: Option<String>,
        client: reqwest::Client,
        actions: Option<BTreeSet<String>>,
    ) -> Self {
        Self {
            base_url,
            explicit,
            source,
            token,
            client,
            dispatch_timeout: DEFAULT_DISPATCH_TIMEOUT,
            actions,
        }
    }

    #[must_use]
    pub fn allows_local_fallback(&self) -> bool {
        !self.explicit
    }

    #[must_use]
    pub fn source(&self) -> &'static str {
        self.source
    }

    pub async fn verify_resource_lease_actions(&self) -> Result<(), ToolError> {
        const REQUIRED: [&str; 3] = [
            "gateway.oauth.resource_lease.create",
            "gateway.oauth.resource_lease.renew",
            "gateway.oauth.resource_lease.release",
        ];
        let actions = self.action_catalog().await?;
        for action in REQUIRED {
            if !actions.contains(action) {
                return Err(ToolError::Sdk {
                    sdk_kind: "proxy_auth_unavailable".to_string(),
                    message: format!(
                        "live Labby daemon does not support required action `{action}`"
                    ),
                });
            }
        }
        Ok(())
    }

    pub async fn verify_oauth_issuer(
        &self,
        issuer: &Url,
    ) -> Result<labby_auth::jwt::JwksDocument, ToolError> {
        let stable_issuer = issuer.as_str().trim_end_matches('/');
        let metadata_url = format!("{stable_issuer}/.well-known/oauth-authorization-server");
        let response = self
            .client
            .get(&metadata_url)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: format!("OAuth authorization-server metadata is unreachable: {error}"),
            })?;
        if !response.status().is_success() {
            return Err(ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: format!(
                    "OAuth authorization-server metadata returned HTTP {}",
                    response.status()
                ),
            });
        }
        let metadata: Value = read_tool_json(
            response,
            MAX_OAUTH_DOCUMENT_BYTES,
            "OAuth authorization-server metadata",
            "proxy_auth_unavailable",
        )
        .await?;
        if metadata.get("issuer").and_then(Value::as_str) != Some(stable_issuer) {
            return Err(ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message:
                    "OAuth metadata issuer does not exactly match the configured stable issuer"
                        .to_string(),
            });
        }
        let jwks_uri = metadata
            .get("jwks_uri")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: "OAuth metadata does not advertise a JWKS URI".to_string(),
            })?;
        let jwks_url = Url::parse(jwks_uri).map_err(|_| ToolError::Sdk {
            sdk_kind: "proxy_auth_unavailable".to_string(),
            message: "OAuth metadata advertises an invalid JWKS URI".to_string(),
        })?;
        let secure_transport = jwks_url.scheme() == "https"
            || (jwks_url.scheme() == "http" && is_loopback_url(&jwks_url));
        if !secure_transport
            || !jwks_url.username().is_empty()
            || jwks_url.password().is_some()
            || !same_origin(issuer, &jwks_url)
        {
            return Err(ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: "OAuth JWKS URI must use the configured issuer origin without userinfo"
                    .to_string(),
            });
        }
        let response = self
            .client
            .get(jwks_url)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: format!("OAuth JWKS is unreachable: {error}"),
            })?;
        if !response.status().is_success() {
            return Err(ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: format!("OAuth JWKS returned HTTP {}", response.status()),
            });
        }
        read_tool_json(
            response,
            MAX_OAUTH_DOCUMENT_BYTES,
            "OAuth JWKS",
            "proxy_auth_unavailable",
        )
        .await
    }

    pub async fn action_catalog(&self) -> Result<BTreeSet<String>, ToolError> {
        match &self.actions {
            Some(actions) => Ok(actions.clone()),
            None => fetch_actions(&self.client, &self.base_url, self.token.as_deref())
                .await
                .map_err(|failure| probe_error(&self.base_url, &failure, self.explicit)),
        }
    }

    pub async fn create_resource_lease(
        &self,
        resource: &str,
        scopes: Vec<String>,
        ttl: Duration,
        owner: &str,
    ) -> Result<labby_auth::resource_registry::ResourceLease, ToolError> {
        let value = self
            .dispatch_action(
                "gateway.oauth.resource_lease.create",
                serde_json::json!({
                    "resource": resource,
                    "scopes": scopes,
                    "ttl_secs": ttl.as_secs(),
                    "owner": owner,
                }),
            )
            .await?;
        serde_json::from_value(value).map_err(typed_response_error)
    }

    pub async fn renew_resource_lease(
        &self,
        id: &str,
        ttl: Duration,
    ) -> Result<labby_auth::resource_registry::ResourceLease, ToolError> {
        let value = self
            .dispatch_action(
                "gateway.oauth.resource_lease.renew",
                serde_json::json!({"id": id, "ttl_secs": ttl.as_secs()}),
            )
            .await?;
        serde_json::from_value(value).map_err(typed_response_error)
    }

    pub async fn release_resource_lease(
        &self,
        id: &str,
    ) -> Result<labby_gateway::gateway::types::ResourceLeaseReleaseView, ToolError> {
        let value = self
            .dispatch_action(
                "gateway.oauth.resource_lease.release",
                serde_json::json!({"id": id}),
            )
            .await?;
        serde_json::from_value(value).map_err(typed_response_error)
    }

    /// Dispatch `action`/`params` through the daemon's generic gateway
    /// action route (`POST /v1/gateway`) -- the same `{action, params}`
    /// shape MCP and the CLI's own local dispatch already use, so this
    /// needs no per-action endpoint mapping.
    pub async fn dispatch_action(&self, action: &str, params: Value) -> Result<Value, ToolError> {
        let mut request = self
            .client
            .post(
                self.base_url
                    .join("v1/gateway")
                    .expect("validated base URL joins"),
            )
            .json(&serde_json::json!({ "action": action, "params": params }));
        if let Some(timeout) = dispatch_timeout_for_action(action, self.dispatch_timeout) {
            request = request.timeout(timeout);
        }
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if cfg!(debug_assertions)
            && (action.starts_with("gateway.loadout.")
                || action.starts_with("gateway.protected_route."))
            && let Ok(team_id) = std::env::var("LABBY_E2E_TEAM_ID")
        {
            request = request.header("x-labby-team-id", team_id);
        }

        let response = request.send().await.map_err(live_gateway_network_error)?;
        let status = response.status();
        if status.is_success() {
            return read_tool_json(
                response,
                MAX_DISPATCH_RESPONSE_BYTES,
                "live gateway daemon response",
                "decode_error",
            )
            .await;
        }

        let body = read_tool_body(
            response,
            MAX_DISPATCH_RESPONSE_BYTES,
            "live gateway daemon error response",
            "decode_error",
        )
        .await?;
        let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);

        let sdk_kind = body
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("internal_error")
            .to_string();
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("live gateway daemon returned HTTP {status}"));
        Err(ToolError::Sdk { sdk_kind, message })
    }

    /// Execute a Code Mode snippet against the live daemon's actual `codemode`
    /// MCP tool over its already-warm upstream connection pool, instead of a
    /// throwaway caller's own cold connections.
    ///
    /// The generic `{action, params}` route above doesn't apply here -- Code
    /// Mode execution is an MCP tool call, not a gateway action -- so this
    /// speaks the MCP streamable-HTTP protocol directly via a short-lived
    /// connection, the same way `labby-gateway`'s own upstream pool connects
    /// to any other MCP server (see `pool/connect.rs`).
    pub async fn call_codemode_tool(&self, code: &str) -> Result<Value, ToolError> {
        use rmcp::model::CallToolRequestParams;

        let service = self
            .connect_service_with_timeout((), MCP_INITIALIZATION_TIMEOUT)
            .await?;
        let peer = service.peer().clone();

        let mut arguments = serde_json::Map::new();
        arguments.insert("code".to_string(), Value::String(code.to_string()));
        let (call_result, cancel_result) = bounded_codemode_call_and_cleanup(
            peer.call_tool(CallToolRequestParams::new("codemode").with_arguments(arguments)),
            CODEMODE_EXECUTION_TIMEOUT,
            MCP_CLEANUP_TIMEOUT,
            service.cancel(),
        )
        .await;
        if let Err(error) = cancel_result {
            tracing::warn!(
                surface = "cli",
                service = "gateway",
                action = "gateway.code.exec",
                origin = %sanitized_origin(&self.base_url),
                error = %error,
                "remote Code Mode MCP shutdown failed"
            );
        }
        codemode_result_value(call_result?)
    }

    /// Open a long-lived MCP streamable-HTTP connection to the daemon's
    /// `/mcp` endpoint and return the running client service. Callers own the
    /// resulting `Peer<RoleClient>` for as long as they need it (e.g. the
    /// stdio bridge holds one for its entire process lifetime, versus
    /// `call_codemode_tool` above which opens one per call).
    ///
    /// Generic over the `ClientHandler` so callers that need the daemon's
    /// server->client requests (elicitation/sampling/roots) answered --
    /// rather than declined, which is what the unit handler `()` does --
    /// can pass one that forwards them somewhere (see
    /// `crate::mcp::bridge::BridgeClientHandler`).
    pub async fn connect_service<H: rmcp::ClientHandler>(
        &self,
        handler: H,
    ) -> anyhow::Result<RunningService<RoleClient, H>> {
        use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
        use rmcp::transport::streamable_http_client::{
            StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
        };

        let mut transport_config = StreamableHttpClientTransportConfig::with_uri(
            self.base_url
                .join("mcp")
                .expect("validated base URL joins")
                .to_string(),
        );
        transport_config.auth_header = self.token.clone();
        let worker = StreamableHttpClientWorker::new(self.client.clone(), transport_config);
        Ok(handler
            .serve_with_lifecycle(
                worker,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                },
            )
            .await?)
    }

    pub async fn connect_service_bounded<H: rmcp::ClientHandler>(
        &self,
        handler: H,
    ) -> Result<RunningService<RoleClient, H>, ToolError> {
        self.connect_service_with_timeout(handler, MCP_INITIALIZATION_TIMEOUT)
            .await
    }

    async fn connect_service_with_timeout<H: rmcp::ClientHandler>(
        &self,
        handler: H,
        timeout: Duration,
    ) -> Result<RunningService<RoleClient, H>, ToolError> {
        tokio::time::timeout(timeout, self.connect_service(handler))
            .await
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "bridge_transport_error".to_string(),
                message: "remote Labby MCP initialization timed out".to_string(),
            })?
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "bridge_transport_error".to_string(),
                message: format!("remote Labby MCP initialization failed: {error}"),
            })
    }
}

async fn bounded_codemode_call<F, T, E>(future: F, timeout: Duration) -> Result<T, ToolError>
where
    F: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| ToolError::Sdk {
            sdk_kind: "bridge_transport_error".to_string(),
            message: "remote Code Mode call timed out".to_string(),
        })?
        .map_err(|error| ToolError::Sdk {
            sdk_kind: "bridge_transport_error".to_string(),
            message: format!("remote Code Mode call failed: {error}"),
        })
}

async fn bounded_codemode_call_and_cleanup<F, T, E, C, CU, CE>(
    future: F,
    timeout: Duration,
    cleanup_timeout: Duration,
    cleanup: C,
) -> (Result<T, ToolError>, Result<CU, ToolError>)
where
    F: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
    C: Future<Output = Result<CU, CE>>,
    CE: std::fmt::Display,
{
    let result = bounded_codemode_call(future, timeout).await;
    let cleanup_result = match tokio::time::timeout(cleanup_timeout, cleanup).await {
        Err(_) => Err(ToolError::Sdk {
            sdk_kind: "bridge_transport_error".to_string(),
            message: "remote Code Mode MCP shutdown timed out".to_string(),
        }),
        Ok(Err(error)) => Err(ToolError::Sdk {
            sdk_kind: "bridge_transport_error".to_string(),
            message: format!("remote Code Mode MCP shutdown failed: {error}"),
        }),
        Ok(Ok(value)) => Ok(value),
    };
    (result, cleanup_result)
}

fn codemode_result_value(result: rmcp::model::CallToolResult) -> Result<Value, ToolError> {
    let text = result
        .content
        .iter()
        .find_map(|block| block.as_text().map(|text| text.text.clone()))
        .unwrap_or_default();
    let payload = result
        .structured_content
        .unwrap_or_else(|| serde_json::from_str(&text).unwrap_or(Value::String(text)));
    if result.is_error == Some(true) {
        let sdk_kind = payload
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("bridge_transport_error")
            .to_string();
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .map(labby_runtime::redact::redact_secret_like_segments)
            .unwrap_or_else(|| "remote Code Mode tool returned an error".to_string());
        return Err(ToolError::Sdk { sdk_kind, message });
    }
    Ok(payload)
}

fn dispatch_timeout_for_action(action: &str, default: Duration) -> Option<Duration> {
    if labby_gateway::gateway::requires_authoritative_result(action) {
        None
    } else {
        Some(default)
    }
}

fn live_gateway_network_error(error: reqwest::Error) -> ToolError {
    let message = if error.is_timeout() {
        "request to live gateway daemon timed out".to_string()
    } else {
        format!("request to live gateway daemon failed: {error}")
    };
    ToolError::Sdk {
        sdk_kind: "network_error".to_string(),
        message,
    }
}

fn typed_response_error(error: serde_json::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: format!("invalid typed response from live gateway daemon: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // See google.rs::GoogleProvider::new for why this call is needed under
    // "rustls-no-provider" -- idempotent, safe to call repeatedly.
    fn ensure_tls_provider() {
        drop(rustls::crypto::ring::default_provider().install_default());
    }

    fn test_gateway(base_url: String, token: Option<String>) -> LiveGateway {
        ensure_tls_provider();
        LiveGateway {
            base_url: normalize_base_url(&base_url).expect("test URL parses"),
            explicit: false,
            source: "test",
            token,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("test client builds"),
            dispatch_timeout: DEFAULT_DISPATCH_TIMEOUT,
            actions: None,
        }
    }

    async fn detect_opportunistically(
        config: &LabConfig,
    ) -> Result<Option<LiveGateway>, ToolError> {
        let targets = resolve_target_set_from(None, None, None, None, config)?;
        detect_targets(targets, None, DISCOVERY_TIMEOUT, "test").await
    }

    #[test]
    fn local_candidate_prefers_env_over_config_over_default() {
        let mut config = LabConfig::default();
        config.mcp.host = Some("configured.example".to_string());
        config.mcp.port = Some(1234);

        assert_eq!(
            candidate_base_urls_from(None, None, &LabConfig::default()),
            vec!["http://127.0.0.1:8765".to_string()]
        );
        assert_eq!(
            candidate_base_urls_from(None, None, &config),
            vec!["http://configured.example:1234".to_string()]
        );
        assert_eq!(
            candidate_base_urls_from(
                Some("env.example".to_string()),
                Some("9999".to_string()),
                &config
            ),
            vec!["http://env.example:9999".to_string()]
        );
    }

    #[test]
    fn candidates_fall_through_to_configured_public_urls() {
        let mut config = LabConfig::default();
        config.public_urls = Some(crate::config::PublicUrlsConfig {
            app: Some("https://labby.example.com/".to_string()),
            mcp_gateway: Some("https://mcp.example.com".to_string()),
        });

        // Local bind address first (fast path), then the dedicated gateway
        // URL, then the general app URL -- and a trailing slash is trimmed
        // so it composes cleanly with `/health` and `/v1/gateway`.
        assert_eq!(
            candidate_base_urls_from(None, None, &config),
            vec![
                "http://127.0.0.1:8765".to_string(),
                "https://mcp.example.com".to_string(),
                "https://labby.example.com".to_string(),
            ]
        );
    }

    #[test]
    fn plugin_target_wins_and_terminal_mcp_is_normalized() {
        let targets = resolve_target_set_from(
            Some("https://plugin.example/prefix/mcp"),
            Some("https://operator.example"),
            None,
            None,
            &LabConfig::default(),
        )
        .expect("valid explicit target");

        let TargetSet::Explicit { base_url, source } = targets else {
            panic!("expected explicit target");
        };
        assert_eq!(source, ExplicitSource::Plugin);
        assert_eq!(base_url.as_str(), "https://plugin.example/prefix/");
        assert_eq!(
            base_url.join("health").unwrap().as_str(),
            "https://plugin.example/prefix/health"
        );
    }

    #[test]
    fn explicit_target_credentials_are_scoped_to_the_selected_authority() {
        let plugin = resolve_target_set_from(
            Some("https://plugin.example"),
            Some("https://operator.example"),
            None,
            None,
            &LabConfig::default(),
        )
        .unwrap();
        assert_eq!(
            token_for_target_from(
                &plugin,
                Some("plugin-token".to_string()),
                Some("product-admin-token".to_string())
            ),
            Some("plugin-token".to_string())
        );
        assert_eq!(
            token_for_target_from(&plugin, None, Some("product-admin-token".to_string())),
            None,
            "plugin override must never inherit the ambient product token"
        );

        let operator = resolve_target_set_from(
            None,
            Some("https://operator.example"),
            None,
            None,
            &LabConfig::default(),
        )
        .unwrap();
        assert_eq!(
            token_for_target_from(&operator, None, Some("product-admin-token".to_string())),
            Some("product-admin-token".to_string())
        );
    }

    #[test]
    fn probe_status_errors_preserve_authentication_and_authorization_recovery() {
        for (status, credentials_supplied, expected_kind, expected_recovery) in [
            (
                reqwest::StatusCode::UNAUTHORIZED,
                false,
                "auth_required",
                "reauthenticate",
            ),
            (
                reqwest::StatusCode::UNAUTHORIZED,
                true,
                "auth_failed",
                "reauthenticate",
            ),
            (
                reqwest::StatusCode::FORBIDDEN,
                true,
                "forbidden",
                "do_not_retry",
            ),
        ] {
            let failure = probe_status_error(ProbeStage::Actions, status, credentials_supplied);
            let envelope =
                probe_error(&Url::parse("https://example.com/").unwrap(), &failure, true)
                    .to_agent_value();
            assert_eq!(envelope["kind"], expected_kind);
            assert_eq!(envelope["recovery"]["action"], expected_recovery);
            assert_eq!(envelope["recovery"]["same_arguments"], "never");
            assert_eq!(envelope["side_effects"], "none_expected");
        }
    }

    #[test]
    fn explicit_target_validation_matrix() {
        for accepted in [
            "https://example.com",
            "https://example.com/base/mcp/",
            "http://localhost:8765/mcp",
            "http://127.0.0.1:8765",
            "http://[::1]:8765/mcp",
        ] {
            normalize_explicit_target(accepted)
                .unwrap_or_else(|error| panic!("{accepted}: {error}"));
        }

        for rejected in [
            "http://remote.example:8765",
            "ftp://example.com/mcp",
            "https://user:secret@example.com/mcp",
            "https://example.com/mcp?token=secret",
            "https://example.com/mcp#secret",
        ] {
            let error = normalize_explicit_target(rejected).expect_err(rejected);
            assert_eq!(error.kind(), "invalid_param");
            assert!(!error.to_string().contains("secret"));
        }
    }

    #[test]
    fn empty_bearer_token_is_treated_as_absent() {
        assert_eq!(normalize_token(None), None);
        assert_eq!(normalize_token(Some(String::new())), None);
        assert_eq!(
            normalize_token(Some("token".to_string())),
            Some("token".to_string())
        );
    }

    #[tokio::test]
    async fn explicit_tokenless_actions_failure_is_not_swallowed() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/labby.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "apiBaseUrl": server.uri(),
                "paletteCatalogUrl": format!("{}/v1/palette/catalog", server.uri()),
                "paletteExecuteUrl": format!("{}/v1/palette/execute", server.uri()),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let target = TargetSet::Explicit {
            base_url: normalize_base_url(&server.uri()).unwrap(),
            source: ExplicitSource::Operator,
        };
        let error = match detect_targets(target, None, DISCOVERY_TIMEOUT, "test").await {
            Err(error) => error,
            Ok(_) => panic!("explicit actions failure must fail closed"),
        };

        assert_eq!(error.kind(), "auth_required");
        assert!(error.to_string().contains("local fallback suppressed"));
    }

    #[tokio::test]
    async fn opportunistic_discovery_respects_aggregate_deadline() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
            .mount(&server)
            .await;
        let target = TargetSet::Opportunistic(vec![normalize_base_url(&server.uri()).unwrap()]);
        let started = Instant::now();

        let detected = detect_targets(target, None, Duration::from_millis(25), "test")
            .await
            .expect("opportunistic timeout is not an error");

        assert!(detected.is_none());
        // The mock upstream stalls for 2s, so the ceiling only has to stay under
        // that to prove the 25ms detect timeout is what returned control.
        assert!(
            started.elapsed() < Duration::from_millis(1_800),
            "opportunistic detection must obey its own timeout: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn remote_probe_does_not_follow_redirects() {
        ensure_tls_provider();
        let destination = MockServer::start().await;
        let source = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", destination.uri()))
            .mount(&source)
            .await;

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let base_url = normalize_base_url(&source.uri()).unwrap();
        let failure = probe_target(&client, &base_url, Some("sensitive-token"), true)
            .await
            .expect_err("redirect must be rejected");

        assert_eq!(failure.status, Some(reqwest::StatusCode::FOUND));
        assert!(destination.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn authenticated_actions_and_dispatch_do_not_follow_redirects() {
        ensure_tls_provider();
        let destination = MockServer::start().await;
        let source = MockServer::start().await;
        for (method_name, endpoint) in [("GET", "/v1/gateway/actions"), ("POST", "/v1/gateway")] {
            Mock::given(method(method_name))
                .and(path(endpoint))
                .and(header("authorization", "Bearer sensitive-token"))
                .respond_with(
                    ResponseTemplate::new(302)
                        .insert_header("location", format!("{}{endpoint}", destination.uri())),
                )
                .mount(&source)
                .await;
        }

        let mut gateway = test_gateway(source.uri(), Some("sensitive-token".to_string()));
        gateway.explicit = true;
        let catalog_error = gateway
            .action_catalog()
            .await
            .expect_err("actions redirect must fail");
        assert_eq!(catalog_error.kind(), "service_unavailable");
        let dispatch_error = gateway
            .dispatch_action("gateway.list", json!({}))
            .await
            .expect_err("dispatch redirect must fail");
        assert_eq!(dispatch_error.kind(), "internal_error");
        assert!(destination.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mcp_initialization_does_not_follow_redirects() {
        ensure_tls_provider();
        let destination = MockServer::start().await;
        let source = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", "Bearer sensitive-token"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/mcp", destination.uri())),
            )
            .mount(&source)
            .await;

        let gateway = test_gateway(source.uri(), Some("sensitive-token".to_string()));
        let error = gateway
            .connect_service_with_timeout((), Duration::from_secs(1))
            .await
            .expect_err("MCP redirect must fail initialization");

        assert_eq!(error.kind(), "bridge_transport_error");
        assert!(destination.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stalled_mcp_initialization_is_bounded() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            // Far past the elapsed ceiling below. If this stall were inside the
            // ceiling, a regression that ignores the caller's 25ms timeout would
            // still surface the mock's malformed 200 as the same
            // bridge_transport_error kind and pass every assertion here.
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;

        let gateway = test_gateway(server.uri(), None);
        let started = Instant::now();
        let error = gateway
            .connect_service_with_timeout((), Duration::from_millis(25))
            .await
            .expect_err("stalled MCP initialization must time out");

        assert_eq!(error.kind(), "bridge_transport_error");
        let envelope = error.to_agent_value();
        assert_eq!(envelope["origin"], "bridge");
        assert_eq!(envelope["recovery"]["action"], "start_dependency");
        assert_eq!(envelope["recovery"]["same_arguments"], "conditional");
        assert_eq!(envelope["side_effects"], "unknown");
        // Boundedness is proven by the timeout error above: the upstream here
        // never answers within the configured limit. This ceiling only catches a
        // regression that ignores the caller's timeout and falls back to a
        // multi-second default, so it is deliberately loose — a tight budget
        // measured the test machine's scheduler, not the code, and failed under
        // parallel test load.
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn stalled_codemode_call_is_bounded_with_bridge_recovery() {
        let started = Instant::now();
        let cleanup_observed = Arc::new(AtomicBool::new(false));
        let cleanup_flag = cleanup_observed.clone();
        let (result, cleanup_result) = bounded_codemode_call_and_cleanup(
            std::future::pending::<Result<(), std::io::Error>>(),
            Duration::from_millis(25),
            Duration::from_millis(25),
            async move {
                cleanup_flag.store(true, Ordering::SeqCst);
                Ok::<(), std::io::Error>(())
            },
        )
        .await;
        let error = result.expect_err("stalled Code Mode call must time out");

        assert_eq!(error.kind(), "bridge_transport_error");
        let envelope = error.to_agent_value();
        assert_eq!(envelope["origin"], "bridge");
        assert_eq!(envelope["recovery"]["action"], "start_dependency");
        assert_eq!(envelope["recovery"]["same_arguments"], "conditional");
        assert_eq!(envelope["side_effects"], "unknown");
        assert!(cleanup_result.is_ok());
        assert!(cleanup_observed.load(Ordering::SeqCst));
        // Boundedness is proven by the timeout error above: the upstream here
        // never answers within the configured limit. This ceiling only catches a
        // regression that ignores the caller's timeout and falls back to a
        // multi-second default, so it is deliberately loose — a tight budget
        // measured the test machine's scheduler, not the code, and failed under
        // parallel test load.
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn stalled_codemode_cleanup_is_bounded() {
        let started = Instant::now();
        let (result, cleanup_result) = bounded_codemode_call_and_cleanup(
            async { Ok::<_, std::io::Error>(()) },
            Duration::from_millis(25),
            Duration::from_millis(25),
            std::future::pending::<Result<(), std::io::Error>>(),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(
            cleanup_result
                .expect_err("stalled cleanup must time out")
                .kind(),
            "bridge_transport_error"
        );
        // Boundedness is proven by the timeout error above: the upstream here
        // never answers within the configured limit. This ceiling only catches a
        // regression that ignores the caller's timeout and falls back to a
        // multi-second default, so it is deliberately loose — a tight budget
        // measured the test machine's scheduler, not the code, and failed under
        // parallel test load.
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn codemode_completed_error_is_not_reported_as_success() {
        let result = rmcp::model::CallToolResult::structured_error(serde_json::json!({
            "kind": "auth_failed",
            "message": "configured token was rejected"
        }));
        let error = codemode_result_value(result).expect_err("isError must fail");

        assert_eq!(error.kind(), "auth_failed");
        assert_eq!(
            error.to_agent_value()["recovery"]["action"],
            "reauthenticate"
        );
    }

    #[test]
    fn explicit_probe_error_is_sanitized_and_suppresses_fallback() {
        let base_url = Url::parse("https://example.com/").unwrap();
        let error = probe_error(
            &base_url,
            &ProbeFailure {
                stage: ProbeStage::Actions,
                status: Some(reqwest::StatusCode::UNAUTHORIZED),
                kind: "auth_required",
                message: "daemon probe returned HTTP 401 Unauthorized".to_string(),
            },
            true,
        );
        assert_eq!(error.kind(), "auth_required");
        assert!(error.to_string().contains("local fallback suppressed"));
        assert_eq!(sanitized_origin(&base_url), "https://example.com");
    }

    #[tokio::test]
    async fn dispatch_action_returns_success_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
            )
            .mount(&server)
            .await;

        let gateway = test_gateway(server.uri(), Some("test-token".to_string()));
        let result = gateway
            .dispatch_action("gateway.list", serde_json::json!({}))
            .await
            .expect("dispatch should succeed");
        assert_eq!(result, serde_json::json!({ "ok": true }));
    }

    #[tokio::test]
    async fn dispatch_action_maps_error_envelope_to_tool_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "kind": "missing_param",
                "message": "upstream is required",
            })))
            .mount(&server)
            .await;

        let gateway = test_gateway(server.uri(), None);
        let error = gateway
            .dispatch_action("gateway.add", serde_json::json!({}))
            .await
            .expect_err("dispatch should fail");
        assert_eq!(error.kind(), "missing_param");
        assert_eq!(error.user_message(), "upstream is required");
    }

    #[tokio::test]
    async fn dispatch_action_rejects_malformed_success_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&server)
            .await;

        let error = test_gateway(server.uri(), None)
            .dispatch_action("gateway.list", json!({}))
            .await
            .expect_err("malformed success must not become JSON null");

        assert_eq!(error.kind(), "decode_error");
    }

    #[tokio::test]
    async fn dispatch_action_rejects_oversized_response_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![
                b' ';
                MAX_DISPATCH_RESPONSE_BYTES
                    + 1
            ]))
            .mount(&server)
            .await;

        let error = test_gateway(server.uri(), None)
            .dispatch_action("gateway.list", json!({}))
            .await
            .expect_err("oversized dispatch response must be bounded");
        assert_eq!(error.kind(), "response_too_large");
    }

    #[tokio::test]
    async fn dispatch_action_times_out_when_live_daemon_stalls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_json(json!({ "ok": true })),
            )
            .mount(&server)
            .await;

        let mut gateway = test_gateway(server.uri(), None);
        gateway.dispatch_timeout = Duration::from_millis(20);
        let error = gateway
            .dispatch_action("gateway.list", json!({}))
            .await
            .expect_err("ordinary live dispatch must be bounded");

        assert_eq!(error.kind(), "network_error");
        assert!(error.user_message().contains("timed out"));
    }

    #[tokio::test]
    async fn detached_mutation_waits_for_unambiguous_server_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_json(json!({
                "action": "gateway.add",
                "params": {"name": "fixture"}
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(75))
                    .set_body_json(json!({ "committed": true })),
            )
            .mount(&server)
            .await;

        let mut gateway = test_gateway(server.uri(), None);
        gateway.dispatch_timeout = Duration::from_millis(10);
        let result = gateway
            .dispatch_action("gateway.add", json!({"name": "fixture"}))
            .await
            .expect("detached mutation must wait for its authoritative server result");

        assert_eq!(result, json!({ "committed": true }));
    }

    #[test]
    fn long_poll_and_detached_mutations_opt_out_of_default_dispatch_timeout() {
        for action in [
            "gateway.oauth.wait",
            "gateway.code_mode.set",
            "gateway.enrich.apply",
            "gateway.protected_route.add",
            "gateway.protected_route.update",
            "gateway.protected_route.remove",
            "gateway.protected_route.stage_add",
            "gateway.protected_route.stage_update",
            "gateway.protected_route.stage_remove",
            "gateway.loadout.add",
            "gateway.loadout.update",
            "gateway.loadout.patch",
            "gateway.loadout.remove",
            "gateway.loadout.stage_update",
            "gateway.loadout.stage_patch",
            "gateway.loadout.stage_remove",
            "gateway.virtual_server.enable",
            "gateway.virtual_server.disable",
            "gateway.virtual_server.remove",
            "gateway.virtual_server.quarantine.restore",
            "gateway.virtual_server.set_surface",
            "gateway.virtual_server.set_mcp_policy",
            "gateway.service_config.set",
            "gateway.discover",
            "gateway.add",
            "gateway.update",
            "gateway.remove",
            "gateway.import",
            "gateway.import_pending.approve",
            "gateway.import_pending.reject",
            "gateway.import_tombstones.clear",
            "gateway.import_tombstones.restore",
            "gateway.reload",
            "gateway.mcp.enable",
            "gateway.mcp.disable",
            "gateway.mcp.restart",
        ] {
            assert_eq!(
                dispatch_timeout_for_action(action, Duration::from_secs(30)),
                None,
                "{action} must not report a client timeout while server work may commit"
            );
        }
        assert_eq!(
            dispatch_timeout_for_action("gateway.list", Duration::from_secs(30)),
            Some(Duration::from_secs(30))
        );

        let classified = labby_gateway::gateway::ACTIONS
            .iter()
            .filter(|spec| {
                dispatch_timeout_for_action(spec.name, Duration::from_secs(30)).is_none()
            })
            .map(|spec| spec.name)
            .collect::<BTreeSet<_>>();
        let expected = [
            "gateway.oauth.wait",
            "gateway.code_mode.set",
            "gateway.enrich.apply",
            "gateway.protected_route.add",
            "gateway.protected_route.update",
            "gateway.protected_route.remove",
            "gateway.protected_route.stage_add",
            "gateway.protected_route.stage_update",
            "gateway.protected_route.stage_remove",
            "gateway.loadout.add",
            "gateway.loadout.update",
            "gateway.loadout.patch",
            "gateway.loadout.remove",
            "gateway.loadout.stage_update",
            "gateway.loadout.stage_patch",
            "gateway.loadout.stage_remove",
            "gateway.virtual_server.enable",
            "gateway.virtual_server.disable",
            "gateway.virtual_server.remove",
            "gateway.virtual_server.quarantine.restore",
            "gateway.virtual_server.set_surface",
            "gateway.virtual_server.set_mcp_policy",
            "gateway.service_config.set",
            "gateway.discover",
            "gateway.add",
            "gateway.update",
            "gateway.remove",
            "gateway.import",
            "gateway.import_pending.approve",
            "gateway.import_pending.reject",
            "gateway.import_tombstones.clear",
            "gateway.import_tombstones.restore",
            "gateway.reload",
            "gateway.mcp.enable",
            "gateway.mcp.disable",
            "gateway.mcp.restart",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        assert_eq!(classified, expected);
    }

    #[tokio::test]
    async fn detect_returns_none_when_unreachable() {
        // Port 0 never accepts a connection, so this exercises the "not
        // running" fallback path without depending on anything actually
        // listening (or not) on a fixed port.
        ensure_tls_provider();
        let mut config = LabConfig::default();
        config.mcp.host = Some("127.0.0.1".to_string());
        config.mcp.port = Some(0);

        assert!(
            detect_opportunistically(&config)
                .await
                .expect("detection succeeds")
                .is_none()
        );
    }

    #[tokio::test]
    async fn detect_returns_some_when_health_check_and_gateway_actions_succeed() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{ "name": "gateway.reload" }])),
            )
            .mount(&server)
            .await;

        let url = Url::parse(&server.uri()).expect("wiremock uri parses");
        let mut config = LabConfig::default();
        config.mcp.host = Some(url.host_str().expect("wiremock host").to_string());
        config.mcp.port = url.port();

        assert!(
            detect_opportunistically(&config)
                .await
                .expect("detection succeeds")
                .is_some()
        );
    }

    #[tokio::test]
    async fn detect_accepts_labby_discovery_when_no_static_token_is_configured() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/labby.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "apiBaseUrl": server.uri(),
                "paletteCatalogUrl": format!("{}/v1/palette/catalog", server.uri()),
                "paletteExecuteUrl": format!("{}/v1/palette/execute", server.uri()),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        assert!(is_labby_gateway_daemon(&client, &server.uri(), None).await);
    }

    #[tokio::test]
    async fn detect_rejects_discovery_when_static_token_fails_gateway_actions_probe() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/labby.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "apiBaseUrl": server.uri(),
                "paletteCatalogUrl": format!("{}/v1/palette/catalog", server.uri()),
                "paletteExecuteUrl": format!("{}/v1/palette/execute", server.uri()),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .and(header("authorization", "Bearer wrong-token"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        assert!(!is_labby_gateway_daemon(&client, &server.uri(), Some("wrong-token")).await);
    }

    #[tokio::test]
    async fn detect_ignores_healthy_non_labby_server() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": "not_found"
            })))
            .mount(&server)
            .await;

        let url = Url::parse(&server.uri()).expect("wiremock uri parses");
        let mut config = LabConfig::default();
        config.mcp.host = Some(url.host_str().expect("wiremock host").to_string());
        config.mcp.port = url.port();

        assert!(
            detect_opportunistically(&config)
                .await
                .expect("detection succeeds")
                .is_none()
        );
    }

    #[tokio::test]
    async fn detect_falls_through_to_a_public_url_when_local_is_unreachable() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{ "name": "gateway.reload" }])),
            )
            .mount(&server)
            .await;

        // Local bind address (port 0) never accepts a connection; only the
        // configured public URL (standing in for the wiremock server) is
        // actually reachable, matching a caller running on a different
        // machine than the daemon.
        let mut config = LabConfig::default();
        config.mcp.host = Some("127.0.0.1".to_string());
        config.mcp.port = Some(0);
        config.public_urls = Some(crate::config::PublicUrlsConfig {
            app: Some(server.uri()),
            mcp_gateway: None,
        });

        let live = detect_opportunistically(&config)
            .await
            .expect("detection succeeds")
            .expect("should fall through to public url");
        assert_eq!(live.base_url.as_str().trim_end_matches('/'), server.uri());
    }

    #[tokio::test]
    async fn typed_resource_lease_methods_use_generic_gateway_actions() {
        let server = MockServer::start().await;
        let lease = serde_json::json!({
            "id": "opaque-lease-id",
            "resource": "https://proxy.example:53147/mcp",
            "scopes": ["mcp:read", "mcp:write"],
            "expires_at_unix": 4_000_000_000_u64
        });
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_json(json!({
                "action": "gateway.oauth.resource_lease.create",
                "params": {
                    "resource": "https://proxy.example:53147/mcp",
                    "scopes": ["mcp:read", "mcp:write"],
                    "ttl_secs": 120,
                    "owner": "proxy-test"
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(&lease))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_json(json!({
                "action": "gateway.oauth.resource_lease.renew",
                "params": {"id": "opaque-lease-id", "ttl_secs": 240}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(&lease))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_json(json!({
                "action": "gateway.oauth.resource_lease.release",
                "params": {"id": "opaque-lease-id"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"released": true})))
            .mount(&server)
            .await;

        let gateway = test_gateway(server.uri(), None);
        let created = gateway
            .create_resource_lease(
                "https://proxy.example:53147/mcp",
                vec!["mcp:read".to_string(), "mcp:write".to_string()],
                Duration::from_mins(2),
                "proxy-test",
            )
            .await
            .unwrap();
        assert_eq!(created.id, "opaque-lease-id");
        gateway
            .renew_resource_lease(&created.id, Duration::from_mins(4))
            .await
            .unwrap();
        gateway.release_resource_lease(&created.id).await.unwrap();
    }

    #[tokio::test]
    async fn resource_lease_action_support_detection_reads_catalog() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"name": "gateway.reload"},
                {"name": "gateway.oauth.resource_lease.create"}
            ])))
            .mount(&server)
            .await;
        let actions = test_gateway(server.uri(), None)
            .action_catalog()
            .await
            .unwrap();
        assert!(actions.contains("gateway.oauth.resource_lease.create"));
        assert!(!actions.contains("gateway.oauth.resource_lease.release"));
    }

    #[tokio::test]
    async fn oauth_proxy_prerequisites_require_all_lease_actions() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"name": "gateway.oauth.resource_lease.create"},
                {"name": "gateway.oauth.resource_lease.renew"}
            ])))
            .mount(&server)
            .await;
        let error = test_gateway(server.uri(), None)
            .verify_resource_lease_actions()
            .await
            .unwrap_err();
        assert!(error.to_string().contains("release"));
    }

    #[tokio::test]
    async fn capability_catalog_is_fetched_once_and_malformed_json_is_typed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .expect(1)
            .mount(&server)
            .await;

        let error = test_gateway(server.uri(), None)
            .verify_resource_lease_actions()
            .await
            .expect_err("malformed catalog must not look like missing actions");

        assert_eq!(error.kind(), "service_unavailable");
        assert!(error.to_string().contains("actions response is invalid"));
    }

    #[tokio::test]
    async fn capability_catalog_rejects_wrong_shape_and_malformed_entries() {
        for body in [
            json!({}),
            json!([{"description": "missing name"}]),
            json!([{"name": ""}]),
            json!([{"name": "   "}]),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/gateway/actions"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;

            let error = test_gateway(server.uri(), None)
                .action_catalog()
                .await
                .expect_err("invalid catalog shape must be rejected");
            assert_eq!(error.kind(), "service_unavailable");
            assert!(error.to_string().contains("actions response is invalid"));
        }
    }

    #[tokio::test]
    async fn discovery_document_is_bounded() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/labby.json"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![
                b' ';
                MAX_DISCOVERY_DOCUMENT_BYTES
                    + 1
            ]))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        assert!(
            !labby_discovery_identifies_daemon(
                &client,
                &normalize_base_url(&server.uri()).unwrap()
            )
            .await
        );
    }

    #[tokio::test]
    async fn capability_catalog_rejects_oversized_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![
                b' ';
                MAX_ACTION_CATALOG_BYTES
                    + 1
            ]))
            .mount(&server)
            .await;

        let error = test_gateway(server.uri(), None)
            .action_catalog()
            .await
            .expect_err("oversized catalog must be rejected before JSON parsing");
        assert_eq!(error.kind(), "service_unavailable");
        assert!(error.to_string().contains("byte limit"));
    }

    #[tokio::test]
    async fn oauth_proxy_prerequisites_verify_exact_issuer_metadata_and_jwks() {
        let server = MockServer::start().await;
        let issuer = server.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": issuer,
                "jwks_uri": format!("{}/jwks", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"keys": []})))
            .mount(&server)
            .await;

        let jwks = test_gateway(server.uri(), None)
            .verify_oauth_issuer(&Url::parse(&server.uri()).unwrap())
            .await
            .unwrap();
        assert!(jwks.keys.is_empty());
    }

    #[tokio::test]
    async fn oauth_proxy_rejects_cross_origin_jwks_without_requesting_it() {
        let issuer_server = MockServer::start().await;
        let target_server = MockServer::start().await;
        let issuer = issuer_server.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": issuer,
                "jwks_uri": format!("{}/jwks", target_server.uri())
            })))
            .mount(&issuer_server)
            .await;

        let error = test_gateway(issuer_server.uri(), None)
            .verify_oauth_issuer(&Url::parse(&issuer_server.uri()).unwrap())
            .await
            .expect_err("metadata must not delegate JWKS authority cross-origin");

        assert_eq!(error.kind(), "proxy_auth_unavailable");
        assert!(target_server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn oauth_proxy_prerequisites_reject_unreachable_metadata() {
        let server = MockServer::start().await;
        let error = test_gateway(server.uri(), None)
            .verify_oauth_issuer(&Url::parse(&server.uri()).unwrap())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("metadata"));
    }

    #[tokio::test]
    async fn oauth_lease_guard_renews_and_releases_without_exposing_id() {
        use crate::proxy::oauth::{OAuthLeaseGuard, OAuthLeaseTiming};

        let server = MockServer::start().await;
        let lease = json!({
            "id": "lease-secret-id",
            "resource": "https://proxy.example:53147/mcp",
            "scopes": ["mcp:read"],
            "expires_at_unix": 4_000_000_000_u64
        });
        for (action, response) in [
            ("gateway.oauth.resource_lease.create", lease.clone()),
            (
                "gateway.oauth.resource_lease.release",
                json!({"released": true}),
            ),
        ] {
            Mock::given(method("POST"))
                .and(path("/v1/gateway"))
                .and(wiremock::matchers::body_partial_json(
                    json!({"action": action}),
                ))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .mount(&server)
                .await;
        }
        let renewal_observed = Arc::new(tokio::sync::Notify::new());
        let renewal_observed_response = Arc::clone(&renewal_observed);
        let renewed_lease = lease.clone();
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_partial_json(json!({
                "action": "gateway.oauth.resource_lease.renew"
            })))
            .respond_with(move |_request: &wiremock::Request| {
                renewal_observed_response.notify_one();
                ResponseTemplate::new(200).set_body_json(&renewed_lease)
            })
            .mount(&server)
            .await;
        let mut guard = OAuthLeaseGuard::create(
            test_gateway(server.uri(), None),
            "https://proxy.example:53147/mcp",
            vec!["mcp:read".to_string()],
            "owner-fingerprint",
            OAuthLeaseTiming {
                ttl: Duration::from_millis(90),
                renew_interval: Duration::from_millis(20),
                jitter_max: Duration::ZERO,
            },
        )
        .await
        .unwrap();
        assert!(!format!("{guard:?}").contains("lease-secret-id"));
        tokio::time::timeout(Duration::from_secs(1), renewal_observed.notified())
            .await
            .expect("OAuth lease renewal request was not observed");
        guard.release().await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let bodies = requests
            .iter()
            .filter_map(|request| serde_json::from_slice::<Value>(&request.body).ok())
            .collect::<Vec<_>>();
        assert!(
            bodies
                .iter()
                .any(|body| body["action"] == "gateway.oauth.resource_lease.renew")
        );
        assert!(
            bodies
                .iter()
                .any(|body| body["action"] == "gateway.oauth.resource_lease.release")
        );
    }

    #[tokio::test]
    async fn oauth_lease_guard_propagates_renewal_failure_and_still_releases() {
        use crate::proxy::oauth::{OAuthLeaseGuard, OAuthLeaseTiming};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_partial_json(json!({
                "action": "gateway.oauth.resource_lease.create"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "lease-secret-id",
                "resource": "https://proxy.example:53147/mcp",
                "scopes": ["mcp:read"],
                "expires_at_unix": 4_000_000_000_u64
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_partial_json(json!({
                "action": "gateway.oauth.resource_lease.renew"
            })))
            .respond_with(ResponseTemplate::new(503).set_body_json(json!({
                "kind": "daemon_unavailable", "message": "renew failed"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_partial_json(json!({
                "action": "gateway.oauth.resource_lease.release"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"released": true})))
            .mount(&server)
            .await;

        let mut guard = OAuthLeaseGuard::create(
            test_gateway(server.uri(), None),
            "https://proxy.example:53147/mcp",
            vec!["mcp:read".to_string()],
            "owner-fingerprint",
            OAuthLeaseTiming {
                ttl: Duration::from_millis(90),
                renew_interval: Duration::from_millis(10),
                jitter_max: Duration::ZERO,
            },
        )
        .await
        .unwrap();
        let error = guard.wait_for_failure().await.unwrap_err();
        assert!(error.to_string().contains("renewal failed"));
        guard.release().await.unwrap();
    }
}
