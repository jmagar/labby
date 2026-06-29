use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::gateway::types::GatewayEnrichmentProvider;
use crate::upstream::types::UpstreamRuntimeOwner;
use labby_runtime::gateway_config::{
    CodeModeConfig, CodeModeResultShapePolicy, ProtectedMcpRouteConfig, UpstreamConfig,
    UpstreamOauthConfig,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GatewayRuntimeOwnerParams {
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
pub(crate) struct GatewayNameParams {
    pub name: String,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerParams>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GatewayImportTombstoneParams {
    pub name: String,
    #[serde(default)]
    pub source_client: Option<String>,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub transport_fingerprint: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerParams>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GatewayClientConfigParams {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProtectedRouteNameParams {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProtectedRouteSpecParams {
    pub route: ProtectedMcpRouteConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProtectedRouteUpdateParams {
    pub name: String,
    pub route: ProtectedMcpRouteConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VirtualServerNameParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceConfigGetParams {
    pub service: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceConfigSetParams {
    pub service: String,
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VirtualServerSurfaceParams {
    pub id: String,
    pub surface: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VirtualServerMcpPolicyParams {
    pub id: String,
    pub allowed_actions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GatewayTestParams {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub spec: Option<UpstreamConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GatewayAddParams {
    pub spec: UpstreamConfig,
    #[serde(default)]
    pub bearer_token_value: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerParams>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayEnrichPreviewParams {
    #[serde(default)]
    pub upstreams: Vec<String>,
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub provider: GatewayEnrichmentProvider,
    #[serde(default)]
    pub max_upstreams: Option<usize>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayEnrichApplyParams {
    pub upstream: String,
    pub hint: String,
    pub metadata_hash: String,
}

#[derive(Debug, Clone, Default)]
pub struct GatewayEnrichmentScope {
    pub route_visible_upstreams: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GatewayUpdatePatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub url: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub command: Option<Option<String>>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
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
    pub code_mode: Option<Option<CodeModeConfig>>,
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

#[cfg(test)]
mod tests {
    use super::GatewayUpdatePatch;

    #[test]
    fn gateway_update_patch_can_clear_bearer_token_env() {
        let patch: GatewayUpdatePatch =
            serde_json::from_str(r#"{"bearer_token_env": null}"#).expect("patch");
        assert_eq!(patch.bearer_token_env, Some(None));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GatewayUpdateParams {
    pub name: String,
    pub patch: GatewayUpdatePatch,
    #[serde(default)]
    pub bearer_token_value: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerParams>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GatewayReloadParams {
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerParams>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GatewayStatusParams {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GatewayMcpToggleParams {
    pub name: String,
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
pub(crate) struct GatewayMcpCleanupParams {
    pub name: String,
    #[serde(default)]
    pub aggressive: bool,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GatewayOauthNameParams {
    pub upstream: String,
    #[serde(default)]
    pub subject: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CodeModeSetParams {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub trace_params: Option<bool>,
    #[serde(default)]
    pub result_shape_policy: Option<CodeModeResultShapePolicy>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_response_bytes: Option<usize>,
    #[serde(default)]
    pub max_response_tokens: Option<usize>,
    #[serde(default)]
    pub token_estimate_divisor: Option<u32>,
    #[serde(default)]
    pub max_log_entries: Option<usize>,
    #[serde(default)]
    pub max_log_bytes: Option<usize>,
}

/// Parameters for `gateway.discover` — read-only scan of external MCP configs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GatewayDiscoverParams {
    /// Limit discovery to these client kinds (e.g. `["cursor", "vscode"]`).
    /// Empty means scan all supported clients.
    #[serde(default)]
    pub clients: Vec<String>,
    /// Also return servers whose name already exists in the gateway config.
    #[serde(default)]
    pub include_existing: bool,
}

/// Parameters for `gateway.import` — import discovered servers (disabled by default).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GatewayImportParams {
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
