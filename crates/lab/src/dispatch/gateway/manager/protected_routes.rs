//! Manager-level protected-route management: live resolver lookups plus CRUD
//! that keeps the in-memory route index in sync with persisted config.

use crate::config::{LabConfig, ProtectedMcpRouteConfig};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::config::{
    insert_protected_mcp_route, remove_protected_mcp_route, update_protected_mcp_route,
};

use super::GatewayManager;

impl GatewayManager {
    pub async fn resolve_protected_route(
        &self,
        host: &str,
        path: &str,
    ) -> Option<ProtectedMcpRouteConfig> {
        self.protected_route_index.read().await.resolve(host, path)
    }

    pub async fn resolve_protected_route_metadata(
        &self,
        host: &str,
        path: &str,
    ) -> Option<ProtectedMcpRouteConfig> {
        self.protected_route_index
            .read()
            .await
            .resolve_exact_metadata_path(host, path)
    }

    pub async fn protected_route_list(&self) -> Vec<ProtectedMcpRouteConfig> {
        self.config.read().await.protected_mcp_routes.clone()
    }

    pub async fn protected_route_get(
        &self,
        name: &str,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        self.config
            .read()
            .await
            .protected_mcp_routes
            .iter()
            .find(|route| route.name == name)
            .cloned()
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("protected MCP route `{name}` not found"),
            })
    }

    pub async fn protected_route_add(
        &self,
        route: ProtectedMcpRouteConfig,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        reject_hot_gateway_subset_mutation(&route, "add")?;
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let route = insert_protected_mcp_route(&mut cfg, route)?;
        self.persist_config(cfg).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.add",
            route = %route.name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            enabled = route.enabled,
            scopes = ?route.scopes,
            "protected MCP route added"
        );
        Ok(route)
    }

    pub async fn protected_route_update(
        &self,
        name: &str,
        route: ProtectedMcpRouteConfig,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        if let Some(existing) = cfg
            .protected_mcp_routes
            .iter()
            .find(|route| route.name == name)
        {
            reject_hot_gateway_subset_mutation(existing, "update")?;
        }
        reject_hot_gateway_subset_mutation(&route, "update")?;
        let route = update_protected_mcp_route(&mut cfg, name, route)?;
        self.persist_config(cfg).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.update",
            route = %route.name,
            previous_name = %name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            enabled = route.enabled,
            scopes = ?route.scopes,
            "protected MCP route updated"
        );
        Ok(route)
    }

    pub async fn protected_route_remove(
        &self,
        name: &str,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        if let Some(existing) = cfg
            .protected_mcp_routes
            .iter()
            .find(|route| route.name == name)
        {
            reject_hot_gateway_subset_mutation(existing, "remove")?;
        }
        let route = remove_protected_mcp_route(&mut cfg, name)?;
        self.persist_config(cfg).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.remove",
            route = %route.name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            "protected MCP route removed"
        );
        Ok(route)
    }

    pub async fn protected_route_test(
        &self,
        route: ProtectedMcpRouteConfig,
    ) -> Result<serde_json::Value, ToolError> {
        let mut cfg = LabConfig::default();
        let route = insert_protected_mcp_route(&mut cfg, route)?;
        let resource = route.public_resource();
        let metadata_url = format!(
            "https://{}/.well-known/oauth-protected-resource{}",
            route.public_host, route.public_path
        );
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.test",
            route = %route.name,
            resource = %resource,
            metadata_url = %metadata_url,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            scopes = ?route.scopes,
            "protected MCP route validated"
        );
        Ok(serde_json::json!({
            "ok": true,
            "route": route,
            "resource": resource,
            "metadata_url": metadata_url,
        }))
    }
}

fn reject_hot_gateway_subset_mutation(
    route: &ProtectedMcpRouteConfig,
    operation: &str,
) -> Result<(), ToolError> {
    if !route.is_gateway_subset() {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "restart_required".to_string(),
        message: format!(
            "gateway_subset protected routes are mounted when labby serve starts; edit config and restart before `{operation}` can take effect"
        ),
    })
}
