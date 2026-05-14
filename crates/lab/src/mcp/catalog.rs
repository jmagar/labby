use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;

use super::server::LabMcpServer;
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::mcp::prompts::list_all as list_builtin_prompts;

pub(crate) const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";
pub(crate) const TOOL_EXECUTE_TOOL_NAME: &str = "tool_execute";
pub(crate) const LEGACY_TOOL_INVOKE_TOOL_NAME: &str = "tool_invoke";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolSearchVisibility {
    Raw,
    RootSynthetic,
    InProcessPeer,
}

impl ToolSearchVisibility {
    pub(crate) fn hides_raw_tools(self) -> bool {
        !matches!(self, Self::Raw)
    }

    pub(crate) fn exposes_synthetic_tools(self) -> bool {
        matches!(self, Self::RootSynthetic)
    }

    pub(crate) fn mode_label(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::RootSynthetic => "tool_search_root",
            Self::InProcessPeer => "tool_search_in_process_peer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogSnapshot {
    pub(crate) tools: BTreeSet<String>,
    pub(crate) resources: BTreeSet<String>,
    pub(crate) prompts: BTreeSet<String>,
}

#[allow(dead_code)]
pub(crate) fn upstream_name_for_uri(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix("lab://upstream/")?;
    let slash_pos = rest.find('/')?;
    Some(&rest[..slash_pos])
}

impl LabMcpServer {
    pub(crate) async fn current_upstream_pool(&self) -> Option<Arc<UpstreamPool>> {
        match &self.gateway_manager {
            Some(manager) => manager.current_pool().await,
            None => None,
        }
    }

    pub(crate) async fn service_visible_on_mcp(&self, service: &str) -> bool {
        if matches!(self.node_role, Some(crate::config::NodeRole::NonMaster)) {
            return false;
        }
        match &self.gateway_manager {
            Some(manager) => manager.surface_enabled_for_service(service, "mcp").await,
            None => true,
        }
    }

    pub(crate) async fn action_allowed_on_mcp(&self, service: &str, action: &str) -> bool {
        match &self.gateway_manager {
            Some(manager) => {
                manager
                    .mcp_action_allowed_for_service(service, action)
                    .await
            }
            None => true,
        }
    }

    pub(crate) async fn allowed_mcp_actions(&self, service: &str) -> Option<Vec<String>> {
        match &self.gateway_manager {
            Some(manager) => manager.allowed_mcp_actions_for_service(service).await,
            None => None,
        }
    }

    pub(crate) async fn tool_search_visibility(&self) -> ToolSearchVisibility {
        let manager_tool_search_enabled = if let Some(manager) = &self.gateway_manager {
            manager.tool_search_enabled().await
        } else {
            false
        };
        if manager_tool_search_enabled {
            return ToolSearchVisibility::RootSynthetic;
        }
        if self.gateway_manager.is_none() && crate::config::process_tool_search_enabled() {
            return ToolSearchVisibility::InProcessPeer;
        }
        ToolSearchVisibility::Raw
    }

    fn service_visible_by_env_or_gateway(&self, service: &str) -> bool {
        crate::registry::lab_show_all_enabled()
            || crate::registry::service_visible_with_env(service)
            || self.gateway_manager.is_some()
    }

    pub(crate) fn builtin_prompt_names(&self) -> Vec<String> {
        list_builtin_prompts()
            .prompts
            .iter()
            .map(|prompt| prompt.name.to_string())
            .collect()
    }

    pub(crate) async fn builtin_resource_identifiers(&self) -> BTreeSet<String> {
        let mut resources = BTreeSet::from(["lab://catalog".to_string()]);
        for svc in self.registry.services() {
            if self.service_visible_on_mcp(svc.name).await {
                resources.insert(format!("lab://{}/actions", svc.name));
            }
        }
        resources
    }

    pub(crate) async fn catalog_json(&self) -> anyhow::Result<Value> {
        let filtered;
        let registry = if crate::registry::lab_show_all_enabled() || self.gateway_manager.is_some()
        {
            &self.registry
        } else {
            filtered = crate::registry::filter_by_configured_env(&self.registry);
            &filtered
        };
        let mut catalog = crate::catalog::build_catalog(registry);
        let mut services = Vec::new();
        for mut service in catalog.services {
            let visible_on_mcp = self.service_visible_on_mcp(&service.name).await;
            if !visible_on_mcp {
                continue;
            }
            if !self.service_visible_by_env_or_gateway(&service.name) {
                continue;
            }
            if let Some(allowed_actions) = self.allowed_mcp_actions(&service.name).await
                && !allowed_actions.is_empty()
            {
                service
                    .actions
                    .retain(|action| allowed_actions.contains(&action.name));
            }
            services.push(service);
        }
        catalog.services = services;
        Ok(serde_json::to_value(catalog)?)
    }

    pub(crate) async fn service_actions_json(&self, service: &str) -> anyhow::Result<Value> {
        if !self.service_visible_on_mcp(service).await {
            anyhow::bail!("unknown service: {service}");
        }
        if !self.service_visible_by_env_or_gateway(service) {
            anyhow::bail!("unknown service: {service}");
        }

        let catalog = crate::catalog::build_catalog(&self.registry);
        let mut entry = catalog
            .services
            .into_iter()
            .find(|entry| entry.name == service)
            .ok_or_else(|| anyhow::anyhow!("unknown service: {service}"))?;

        if let Some(allowed_actions) = self.allowed_mcp_actions(service).await
            && !allowed_actions.is_empty()
        {
            entry
                .actions
                .retain(|action| allowed_actions.contains(&action.name));
        }

        Ok(serde_json::to_value(entry.actions)?)
    }

    pub(crate) async fn snapshot_catalog(&self) -> CatalogSnapshot {
        let visibility = self.tool_search_visibility().await;
        let mut tools = BTreeSet::new();
        if visibility.exposes_synthetic_tools() {
            tools.insert(TOOL_SEARCH_TOOL_NAME.to_string());
            tools.insert(TOOL_EXECUTE_TOOL_NAME.to_string());
        } else {
            for svc in self.registry.services() {
                if !visibility.hides_raw_tools() && self.service_visible_on_mcp(svc.name).await {
                    tools.insert(svc.name.to_string());
                }
            }
        }

        if !visibility.hides_raw_tools()
            && let Some(pool) = self.current_upstream_pool().await
        {
            for tool in pool.healthy_tools().await {
                let name = tool.tool.name.to_string();
                if !tools.contains(&name) {
                    tools.insert(name);
                }
            }
        }

        let mut resources = self.builtin_resource_identifiers().await;
        if let Some(pool) = self.current_upstream_pool().await {
            for (upstream_name, uris) in pool.cached_upstream_resource_uris().await {
                for uri in uris {
                    resources.insert(format!("lab://upstream/{upstream_name}/{uri}"));
                }
            }
        }

        let builtin_prompt_names = self.builtin_prompt_names();
        let builtin_prompt_refs: Vec<&str> =
            builtin_prompt_names.iter().map(String::as_str).collect();
        let mut prompts: BTreeSet<String> = builtin_prompt_names.iter().cloned().collect();
        if let Some(pool) = self.current_upstream_pool().await {
            for prompt_name in pool
                .cached_upstream_prompt_names(&builtin_prompt_refs)
                .await
            {
                prompts.insert(prompt_name);
            }
        }

        CatalogSnapshot {
            tools,
            resources,
            prompts,
        }
    }
}
