use serde::Serialize;

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::oauth::upstream::types::BeginAuthorization;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpstreamOauthStatusView {
    pub authenticated: bool,
    pub upstream: String,
    pub expires_within_5m: bool,
    pub state: UpstreamOauthConnectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token_expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds_until_expiry: Option<i64>,
    #[serde(default)]
    pub refresh_token_present: bool,
    #[serde(default)]
    pub refresh_attempted: bool,
    #[serde(default)]
    pub refreshed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_error: Option<String>,
    #[serde(default)]
    pub discovery_checked: bool,
    #[serde(default)]
    pub discovered_tool_count: usize,
    #[serde(default)]
    pub exposed_tool_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovery_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamOauthConnectionState {
    Connected,
    Expiring,
    Expired,
    RefreshFailed,
    DiscoveryFailed,
    Disconnected,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub upstream: String,
    pub url: String,
    pub transient: bool,
    pub durability: String,
    pub oauth_discovered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_strategy: Option<String>,
}

pub async fn probe(manager: &GatewayManager, url: &str) -> Result<ProbeResult, ToolError> {
    manager.probe_upstream_oauth(url).await
}

pub async fn probe_for_upstream(
    manager: &GatewayManager,
    url: &str,
    upstream: Option<&str>,
) -> Result<ProbeResult, ToolError> {
    manager
        .probe_upstream_oauth_for_upstream(url, upstream)
        .await
}

pub async fn begin_authorization(
    manager: &GatewayManager,
    upstream: &str,
    subject: &str,
) -> Result<BeginAuthorization, ToolError> {
    manager
        .begin_upstream_authorization(upstream, subject)
        .await
}

pub async fn complete_authorization_callback(
    manager: &GatewayManager,
    upstream: &str,
    subject: &str,
    code: &str,
    state: &str,
) -> Result<(), ToolError> {
    manager
        .complete_upstream_authorization_callback(upstream, subject, code, state)
        .await
}

pub async fn status(
    manager: &GatewayManager,
    upstream: &str,
    subject: &str,
) -> Result<UpstreamOauthStatusView, ToolError> {
    manager.upstream_oauth_status(upstream, subject).await
}

pub async fn clear(
    manager: &GatewayManager,
    upstream: &str,
    subject: &str,
) -> Result<(), ToolError> {
    manager.clear_upstream_credentials(upstream, subject).await
}
