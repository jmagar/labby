use labby_primitives::plugin::PluginMeta;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceFieldView {
    pub name: String,
    pub description: String,
    pub example: String,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SupportedServiceView {
    pub key: String,
    pub display_name: String,
    pub category: String,
    pub description: String,
    #[serde(default)]
    pub required_env: Vec<ServiceFieldView>,
    #[serde(default)]
    pub optional_env: Vec<ServiceFieldView>,
    #[serde(default)]
    pub default_port: Option<u16>,
}

pub(crate) fn supported_services_from_registry(
    registry: &dyn crate::gateway::service_registry::GatewayServiceRegistry,
) -> Vec<SupportedServiceView> {
    registry
        .service_names()
        .iter()
        .filter_map(|name| registry.service_meta(name))
        .map(meta_to_view)
        .collect()
}

fn meta_to_view(meta: &'static PluginMeta) -> SupportedServiceView {
    SupportedServiceView {
        key: meta.name.to_string(),
        display_name: meta.display_name.to_string(),
        category: meta.category.as_str().to_string(),
        description: meta.description.to_string(),
        required_env: meta.required_env.iter().map(field_view).collect(),
        optional_env: meta.optional_env.iter().map(field_view).collect(),
        default_port: meta.default_port,
    }
}

fn field_view(field: &labby_primitives::plugin::EnvVar) -> ServiceFieldView {
    ServiceFieldView {
        name: field.name.to_string(),
        description: field.description.to_string(),
        example: field.example.to_string(),
        secret: field.secret,
    }
}
