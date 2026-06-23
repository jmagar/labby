use serde::{Deserialize, Serialize};

use labby_runtime::gateway_config::{
    VirtualServerConfig, VirtualServerMcpPolicyConfig, VirtualServerSurfacesConfig,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum VirtualServerSource {
    LabService { service: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VirtualServerRecord {
    pub id: String,
    pub source: VirtualServerSource,
    pub enabled: bool,
    pub surfaces: VirtualServerSurfacesConfig,
    pub mcp_policy: Option<VirtualServerMcpPolicyConfig>,
}

impl From<&VirtualServerConfig> for VirtualServerRecord {
    fn from(value: &VirtualServerConfig) -> Self {
        Self {
            id: value.id.clone(),
            source: VirtualServerSource::LabService {
                service: value.service.clone(),
            },
            enabled: value.enabled,
            surfaces: value.surfaces.clone(),
            mcp_policy: value.mcp_policy.clone(),
        }
    }
}
