use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;

use super::server::LabMcpServer;
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::mcp::prompts::list_all as list_builtin_prompts;

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
        let registry = if crate::registry::lab_show_all_enabled() {
            &self.registry
        } else {
            filtered = crate::registry::filter_by_configured_env(&self.registry);
            &filtered
        };
        let mut catalog = crate::catalog::build_catalog(registry);
        let mut services = Vec::new();
        for mut service in catalog.services {
            if !self.service_visible_on_mcp(&service.name).await {
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
        let mut tools = BTreeSet::new();
        for svc in self.registry.services() {
            if self.service_visible_on_mcp(svc.name).await {
                tools.insert(svc.name.to_string());
            }
        }

        if let Some(pool) = self.current_upstream_pool().await {
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
