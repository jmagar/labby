use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;

use super::server::LabMcpServer;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::mcp::prompts::list_all as list_builtin_prompts;

/// Primary Cloudflare-style Code Mode tool name.
pub(crate) const CODE_MODE_TOOL_NAME: &str = "codemode";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeModeVisibility {
    Raw,
    /// Full gateway broker — advertises the primary `codemode` tool.
    RootSynthetic,
    /// In-process peer mode — same tool surface as RootSynthetic but without a
    /// live gateway_manager.
    InProcessPeer,
}

impl CodeModeVisibility {
    pub(crate) fn hides_raw_tools(self) -> bool {
        !matches!(self, Self::Raw)
    }

    /// Returns true when the mode registers the gateway Code Mode surface.
    pub(crate) fn exposes_synthetic_tools(self) -> bool {
        matches!(self, Self::RootSynthetic | Self::InProcessPeer)
    }

    pub(crate) fn mode_label(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::RootSynthetic => "code_mode_root",
            Self::InProcessPeer => "code_mode_in_process_peer",
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
    #[cfg(feature = "gateway")]
    pub(crate) async fn current_upstream_pool(&self) -> Option<Arc<UpstreamPool>> {
        match &self.gateway_manager {
            Some(manager) => manager.current_pool().await,
            None => None,
        }
    }

    pub(crate) async fn service_visible_on_mcp(&self, service: &str) -> bool {
        if !self.route_scope.allows_service(service) {
            return false;
        }
        if matches!(self.node_role, Some(crate::config::NodeRole::NonMaster)) {
            return false;
        }
        #[cfg(feature = "gateway")]
        match &self.gateway_manager {
            Some(manager) => manager.surface_enabled_for_service(service, "mcp").await,
            None => true,
        }
        #[cfg(not(feature = "gateway"))]
        true
    }

    pub(crate) async fn action_allowed_on_mcp(&self, service: &str, action: &str) -> bool {
        #[cfg(feature = "gateway")]
        match &self.gateway_manager {
            Some(manager) => {
                manager
                    .mcp_action_allowed_for_service(service, action)
                    .await
            }
            None => true,
        }
        #[cfg(not(feature = "gateway"))]
        {
            let _ = (service, action);
            true
        }
    }

    pub(crate) async fn allowed_mcp_actions(&self, service: &str) -> Option<Vec<String>> {
        #[cfg(feature = "gateway")]
        match &self.gateway_manager {
            Some(manager) => manager.allowed_mcp_actions_for_service(service).await,
            None => None,
        }
        #[cfg(not(feature = "gateway"))]
        {
            let _ = service;
            None
        }
    }

    pub(crate) async fn code_mode_visibility(&self) -> CodeModeVisibility {
        #[cfg(feature = "gateway")]
        {
            if !self.route_scope.exposes_code_mode() {
                return CodeModeVisibility::Raw;
            }
            let manager_code_mode_enabled = if let Some(manager) = &self.gateway_manager {
                manager.code_mode_enabled().await
            } else {
                false
            };
            if manager_code_mode_enabled {
                return CodeModeVisibility::RootSynthetic;
            }
            if self.gateway_manager.is_none() && crate::config::process_code_mode_enabled() {
                return CodeModeVisibility::InProcessPeer;
            }
        }
        CodeModeVisibility::Raw
    }

    fn service_visible_by_env_or_gateway(&self, service: &str) -> bool {
        #[cfg(feature = "gateway")]
        let gateway_available = self.gateway_manager.is_some();
        #[cfg(not(feature = "gateway"))]
        let gateway_available = false;
        crate::registry::lab_show_all_enabled()
            || crate::registry::service_visible_with_env(service)
            || gateway_available
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
        #[cfg(feature = "gateway")]
        let show_all_for_gateway = self.gateway_manager.is_some();
        #[cfg(not(feature = "gateway"))]
        let show_all_for_gateway = false;
        let registry = if crate::registry::lab_show_all_enabled() || show_all_for_gateway {
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
        let visibility = self.code_mode_visibility().await;
        let mut tools = BTreeSet::new();
        if visibility.exposes_synthetic_tools() {
            tools.insert(CODE_MODE_TOOL_NAME.to_string());
        } else {
            for svc in self.registry.services() {
                if !visibility.hides_raw_tools() && self.service_visible_on_mcp(svc.name).await {
                    tools.insert(svc.name.to_string());
                }
            }
        }

        #[cfg(feature = "gateway")]
        if !visibility.hides_raw_tools()
            && let Some(pool) = self.current_upstream_pool().await
        {
            for tool in pool
                .healthy_tools_allowed(self.route_scope.allowed_upstreams())
                .await
            {
                tools.insert(tool.tool.name.to_string());
            }
        }

        let mut resources = self.builtin_resource_identifiers().await;
        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            for (upstream_name, uris) in pool.cached_upstream_resource_uris().await {
                if !self.route_scope.allows_upstream(&upstream_name) {
                    continue;
                }
                for uri in uris {
                    resources.insert(format!("lab://upstream/{upstream_name}/{uri}"));
                }
            }
        }

        let builtin_prompt_names = self.builtin_prompt_names();
        let builtin_prompt_refs: Vec<&str> =
            builtin_prompt_names.iter().map(String::as_str).collect();
        let mut prompts: BTreeSet<String> = builtin_prompt_names.iter().cloned().collect();
        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            let owners = pool.cached_prompt_ownership_map().await;
            for prompt_name in pool
                .cached_upstream_prompt_names(&builtin_prompt_refs)
                .await
            {
                if owners
                    .get(&prompt_name)
                    .is_some_and(|upstream| self.route_scope.allows_upstream(upstream))
                {
                    prompts.insert(prompt_name);
                }
            }
        }

        CatalogSnapshot {
            tools,
            resources,
            prompts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Tool name constants (Cloudflare-parity, no aliases) ─────────────────

    #[test]
    fn canonical_tool_name_is_codemode() {
        // PRESENCE: canonical names match expected Cloudflare-parity values
        assert_eq!(
            CODE_MODE_TOOL_NAME, "codemode",
            "primary Code Mode tool name must be 'codemode'"
        );
    }

    #[test]
    fn code_mode_visibility_methods() {
        // RootSynthetic exposes the gateway synthetic tools.
        assert!(CodeModeVisibility::RootSynthetic.exposes_synthetic_tools());
        assert!(CodeModeVisibility::RootSynthetic.hides_raw_tools());
        // InProcessPeer is a synthetic-tools sub-variant.
        assert!(CodeModeVisibility::InProcessPeer.exposes_synthetic_tools());
        assert!(CodeModeVisibility::InProcessPeer.hides_raw_tools());
        // Raw exposes neither and does not hide raw tools.
        assert!(!CodeModeVisibility::Raw.exposes_synthetic_tools());
        assert!(!CodeModeVisibility::Raw.hides_raw_tools());
    }
}
