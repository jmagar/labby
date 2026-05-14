use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::config::{
    ProtectedMcpRouteConfig, ToolSearchConfig, UpstreamConfig, UpstreamOauthConfig,
};
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayRuntimeOwnerParams {
    pub surface: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub raw: Option<String>,
}

impl From<GatewayRuntimeOwnerParams> for UpstreamRuntimeOwner {
    fn from(value: GatewayRuntimeOwnerParams) -> Self {
        Self {
            surface: value.surface,
            subject: value.subject,
            request_id: value.request_id,
            session_id: value.session_id,
            client_name: value.client_name,
            raw: value.raw,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayNameParams {
    pub name: String,
    #[serde(default)]
    pub allow_stdio: bool,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerParams>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayClientConfigParams {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectedRouteNameParams {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectedRouteSpecParams {
    pub route: ProtectedMcpRouteConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectedRouteUpdateParams {
    pub name: String,
    pub route: ProtectedMcpRouteConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualServerNameParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfigGetParams {
    pub service: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfigSetParams {
    pub service: String,
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualServerSurfaceParams {
    pub id: String,
    pub surface: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualServerMcpPolicyParams {
    pub id: String,
    pub allowed_actions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayTestParams {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub spec: Option<UpstreamConfig>,
    #[serde(default)]
    pub allow_stdio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayAddParams {
    pub spec: UpstreamConfig,
    #[serde(default)]
    pub bearer_token_value: Option<String>,
    #[serde(default)]
    pub allow_stdio: bool,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerParams>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayUpdatePatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub url: Option<Option<String>>,
    #[serde(default)]
    pub command: Option<Option<String>>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub bearer_token_env: Option<Option<String>>,
    #[serde(default)]
    pub proxy_resources: Option<bool>,
    #[serde(default)]
    pub proxy_prompts: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub expose_tools: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub expose_resources: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub expose_prompts: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub oauth: Option<Option<UpstreamOauthConfig>>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub tool_search: Option<Option<ToolSearchConfig>>,
}

/// Distinguish absent from null for `Option<Option<T>>` patch fields.
///
/// With plain `#[serde(default)]`, serde_json treats both absent fields and
/// explicit `null` as `None`, making it impossible to clear a field via patch.
/// This deserializer wraps the result in `Some(...)` so:
///
/// - absent → `None` (from `#[serde(default)]`)
/// - `null` → `Some(None)` (clear the field)
/// - `["a"]` → `Some(Some(["a"]))` (set the field)
fn deserialize_nullable<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayUpdateParams {
    pub name: String,
    pub patch: GatewayUpdatePatch,
    #[serde(default)]
    pub bearer_token_value: Option<String>,
    #[serde(default)]
    pub allow_stdio: bool,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerParams>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayReloadParams {
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerParams>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayStatusParams {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayMcpToggleParams {
    pub name: String,
    #[serde(default)]
    pub allow_stdio: bool,
    #[serde(default)]
    pub cleanup: bool,
    #[serde(default)]
    pub aggressive: bool,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerParams>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayMcpCleanupParams {
    pub name: String,
    #[serde(default)]
    pub aggressive: bool,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayOauthNameParams {
    pub upstream: String,
    #[serde(default)]
    pub subject: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSearchSetParams {
    pub enabled: bool,
    #[serde(default)]
    pub top_k_default: Option<usize>,
    #[serde(default)]
    pub max_tools: Option<usize>,
}

/// Parameters for `gateway.discover` — read-only scan of external MCP configs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayDiscoverParams {
    /// Limit discovery to these client kinds (e.g. ["cursor", "vscode"]).
    /// Empty means scan all supported clients.
    #[serde(default)]
    pub clients: Vec<String>,
    /// Also return servers whose name already exists in the gateway config.
    #[serde(default)]
    pub include_existing: bool,
}

/// Parameters for `gateway.import` — import discovered servers (disabled by default).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayImportParams {
    /// Specific server names to import. Mutually exclusive with `all`.
    #[serde(default)]
    pub names: Vec<String>,
    /// Import every discovered server not already in the gateway config.
    #[serde(default)]
    pub all: bool,
    /// Limit discovery to these client kinds. Empty means scan all.
    #[serde(default)]
    pub clients: Vec<String>,
}
