use lab_apis::core::PluginMeta;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceFieldView {
    pub name: String,
    pub description: String,
    pub example: String,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportedServiceView {
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

pub fn supported_services_from_registry(
    registry: &crate::registry::ToolRegistry,
) -> Vec<SupportedServiceView> {
    registry
        .services()
        .iter()
        .filter_map(|service| crate::registry::service_meta(service.name))
        .map(meta_to_view)
        .collect()
}

pub fn service_meta(service: &str) -> Option<&'static PluginMeta> {
    crate::registry::service_meta(service)
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

fn field_view(field: &lab_apis::core::EnvVar) -> ServiceFieldView {
    ServiceFieldView {
        name: field.name.to_string(),
        description: field.description.to_string(),
        example: field.example.to_string(),
        secret: field.secret,
    }
}
