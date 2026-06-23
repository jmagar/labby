//! Manager-level virtual-server CRUD: enable/disable, surface toggles,
//! quarantine restore, and MCP action policy management.

use crate::gateway::config_mutation::read_env_values;
use crate::gateway::projection::{server_view_from_virtual_server, service_config_view};
use crate::gateway::types::VirtualServerMcpPolicyView;
use crate::gateway::view_models::ServerView;
use crate::upstream::pool::UpstreamCachedSummary;
use lab_runtime::error::ToolError;
use lab_runtime::gateway_config::GatewayConfig;

use super::GatewayManager;

pub(super) fn find_virtual_server<'a>(
    cfg: &'a GatewayConfig,
    id: &str,
) -> Result<&'a lab_runtime::gateway_config::VirtualServerConfig, ToolError> {
    cfg.virtual_servers
        .iter()
        .find(|server| server.id == id)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("virtual server `{id}` not found"),
        })
}

impl GatewayManager {
    pub async fn service_for_virtual_server_id(&self, id: &str) -> Result<String, ToolError> {
        let cfg = self.config.read().await;
        Ok(find_virtual_server(&cfg, id)?.service.clone())
    }

    pub async fn enable_virtual_server(&self, id: &str) -> Result<ServerView, ToolError> {
        self.set_virtual_server_enabled(id, true).await
    }

    pub async fn disable_virtual_server(&self, id: &str) -> Result<ServerView, ToolError> {
        self.set_virtual_server_enabled(id, false).await
    }

    pub async fn remove_virtual_server(&self, id: &str) -> Result<ServerView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let index = cfg
            .virtual_servers
            .iter()
            .position(|server| server.id == id)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("virtual server `{id}` not found"),
            })?;
        let removed = cfg.virtual_servers.remove(index);
        let removed_view = server_view_from_virtual_server(
            &removed,
            UpstreamCachedSummary::default(),
            None,
            None,
            self.builtin_service_registry().as_ref(),
        );

        self.persist_config(cfg).await?;
        Ok(removed_view)
    }

    pub async fn list_quarantined_virtual_servers(&self) -> Result<Vec<ServerView>, ToolError> {
        let cfg = self.config.read().await;
        let registry = self.builtin_service_registry();
        Ok(cfg
            .quarantined_virtual_servers
            .iter()
            .map(|virtual_server| {
                server_view_from_virtual_server(
                    virtual_server,
                    UpstreamCachedSummary::default(),
                    None,
                    None,
                    registry.as_ref(),
                )
            })
            .collect())
    }

    pub async fn restore_quarantined_virtual_server(
        &self,
        id: &str,
    ) -> Result<ServerView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let index = cfg
            .quarantined_virtual_servers
            .iter()
            .position(|server| server.id == id)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("quarantined virtual server `{id}` not found"),
            })?;
        let restored = cfg.quarantined_virtual_servers.remove(index);
        if self.registered_service_meta(&restored.service).is_none() {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_service".to_string(),
                message: format!(
                    "service `{}` is not registered in this lab binary",
                    restored.service
                ),
            });
        }
        if cfg
            .virtual_servers
            .iter()
            .any(|server| server.id == restored.id)
        {
            return Err(ToolError::InvalidParam {
                message: format!("virtual server `{id}` already exists"),
                param: "id".to_string(),
            });
        }

        let restored_view = server_view_from_virtual_server(
            &restored,
            UpstreamCachedSummary::default(),
            None,
            None,
            self.builtin_service_registry().as_ref(),
        );
        cfg.virtual_servers.push(restored);
        self.persist_config(cfg).await?;
        Ok(restored_view)
    }

    pub async fn set_virtual_server_surface(
        &self,
        id: &str,
        surface: &str,
        enabled: bool,
    ) -> Result<ServerView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let virtual_server = cfg
            .virtual_servers
            .iter_mut()
            .find(|server| server.id == id)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("virtual server `{id}` not found"),
            })?;

        match surface {
            "cli" => virtual_server.surfaces.cli = enabled,
            "api" => virtual_server.surfaces.api = enabled,
            "mcp" => virtual_server.surfaces.mcp = enabled,
            "webui" => virtual_server.surfaces.webui = enabled,
            _ => {
                return Err(ToolError::InvalidParam {
                    message: format!("unknown surface `{surface}`"),
                    param: "surface".to_string(),
                });
            }
        }

        self.persist_config(cfg).await?;
        let cfg = self.config.read().await;
        let virtual_server = find_virtual_server(&cfg, id)?;
        Ok(server_view_from_virtual_server(
            virtual_server,
            UpstreamCachedSummary::default(),
            None,
            None,
            self.builtin_service_registry().as_ref(),
        ))
    }

    pub async fn get_virtual_server_mcp_policy(
        &self,
        id: &str,
    ) -> Result<VirtualServerMcpPolicyView, ToolError> {
        let cfg = self.config.read().await;
        let virtual_server = find_virtual_server(&cfg, id)?;
        Ok(VirtualServerMcpPolicyView {
            allowed_actions: virtual_server
                .mcp_policy
                .as_ref()
                .map(|policy| policy.allowed_actions.clone())
                .unwrap_or_default(),
        })
    }

    pub async fn set_virtual_server_mcp_policy(
        &self,
        id: &str,
        allowed_actions: &[String],
    ) -> Result<VirtualServerMcpPolicyView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let virtual_server = cfg
            .virtual_servers
            .iter_mut()
            .find(|server| server.id == id)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("virtual server `{id}` not found"),
            })?;

        virtual_server.mcp_policy = if allowed_actions.is_empty() {
            None
        } else {
            Some(lab_runtime::gateway_config::VirtualServerMcpPolicyConfig {
                allowed_actions: allowed_actions.to_vec(),
            })
        };

        self.persist_config(cfg).await?;
        Ok(VirtualServerMcpPolicyView {
            allowed_actions: allowed_actions.to_vec(),
        })
    }

    async fn set_virtual_server_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<ServerView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let existing_index = cfg
            .virtual_servers
            .iter()
            .position(|server| server.id == id);
        let index = if let Some(index) = existing_index {
            index
        } else {
            let meta = self
                .registered_service_meta(id)
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("virtual server `{id}` not found"),
                })?;
            let values = read_env_values(&self.env_path())?;
            let configured = service_config_view(meta, &values).configured;
            if !configured {
                return Err(ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("virtual server `{id}` not found"),
                });
            }

            cfg.virtual_servers
                .push(lab_runtime::gateway_config::VirtualServerConfig {
                    id: id.to_string(),
                    service: id.to_string(),
                    enabled: false,
                    surfaces: lab_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                    mcp_policy: None,
                });
            cfg.virtual_servers.len() - 1
        };

        let virtual_server = cfg
            .virtual_servers
            .get_mut(index)
            .expect("virtual server index should exist");
        if enabled
            && self
                .registered_service_meta(&virtual_server.service)
                .is_none()
        {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("virtual server `{id}` not found"),
            });
        }
        virtual_server.enabled = enabled;
        if enabled {
            virtual_server.surfaces.mcp = true;
        }

        self.persist_config(cfg).await?;
        let cfg = self.config.read().await;
        let virtual_server = find_virtual_server(&cfg, id)?;
        Ok(server_view_from_virtual_server(
            virtual_server,
            UpstreamCachedSummary::default(),
            None,
            None,
            self.builtin_service_registry().as_ref(),
        ))
    }
}
