use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DocsProjection {
    pub mcp_help: crate::catalog::Catalog,
    pub service_catalog: Vec<ServiceDoc>,
    pub proxy_config_reference: Vec<ConfigDoc>,
    pub env_reference: Vec<EnvDoc>,
    pub action_catalog: Vec<ActionDoc>,
    pub feature_matrix: FeatureMatrix,
    pub api_routes: Vec<RouteDoc>,
    pub openapi_json: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigDoc {
    pub section: String,
    pub key: String,
    pub toml_path: String,
    pub ty: String,
    pub default: String,
    pub secret: bool,
    pub env_override: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceDoc {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub category: String,
    pub status: String,
    pub feature: Option<String>,
    pub exposure: ServiceExposure,
    pub surfaces: SurfaceAvailability,
    pub default_port: Option<u16>,
    pub docs_url: Option<String>,
    pub supports_multi_instance: bool,
    pub metadata_source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceExposure {
    AlwaysOn,
    FeatureGated,
    RuntimeConditional,
    SdkOnly,
}

#[derive(Debug, Clone, Serialize)]
pub struct SurfaceAvailability {
    pub cli: bool,
    pub mcp: bool,
    pub api: bool,
    pub web_ui: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvDoc {
    pub service: String,
    pub env_var: String,
    pub required: bool,
    pub secret: bool,
    pub description: String,
    pub example: String,
    pub default_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionDoc {
    pub service: String,
    pub action: String,
    pub description: String,
    pub destructive: bool,
    pub requires_admin: bool,
    pub required_scopes: Vec<String>,
    pub authorization_boundary: String,
    pub required_capability: Option<String>,
    pub resource_family: Option<String>,
    pub params: Vec<ParamDoc>,
    pub returns: String,
    pub surface_availability: SurfaceAvailability,
    pub requires_http_subject: bool,
    pub auth_posture: String,
    pub inventory_scope: String,
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParamDoc {
    pub name: String,
    pub ty: String,
    pub required: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureMatrix {
    pub features: Vec<FeatureDoc>,
    pub mismatches: Vec<FeatureMismatch>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureDoc {
    pub crate_name: String,
    pub feature: String,
    pub dependencies: Vec<String>,
    pub included_in_default: bool,
    pub included_in_all: bool,
    pub classification: FeatureClass,
    pub mapped_crate_feature: Option<String>,
    pub exception_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeatureClass {
    ServicePassthrough,
    SdkOnly,
    ProductSlice,
    /// Feature-gated base capability (acp/nodes/stash): member of `all`,
    /// excluded from gateway-only builds, not a standalone product slice.
    BaseCapability,
    BinaryOnly,
    HelperInternal,
    ExtractedCrate,
    AggregateDefault,
    IntentionalException,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureMismatch {
    pub feature: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteDoc {
    pub method: String,
    pub path: String,
    pub aliases: Vec<String>,
    pub surface: String,
    pub handler_group: String,
    pub handler_identity: String,
    pub feature: Option<String>,
    pub runtime_condition: Option<String>,
    pub auth_required: bool,
    pub bearer_only: bool,
    pub bootstrap_proof: bool,
    pub session_cookie_allowed: bool,
    pub csrf_required: bool,
    pub host_validation: bool,
    pub master_only: bool,
    pub cache_posture: String,
    pub failure_disclosure: String,
    pub side_effects: String,
    pub notes: String,
}
