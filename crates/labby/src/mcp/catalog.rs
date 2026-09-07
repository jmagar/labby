use std::collections::BTreeSet;
use std::sync::Arc;

use rmcp::RoleServer;
use rmcp::model::Tool;
use rmcp::service::RequestContext;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::server::LabMcpServer;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::pool::UpstreamPool;
#[cfg(feature = "gateway")]
use crate::mcp::context::{
    auth_context_from_extensions, code_mode_read_scope_allowed, oauth_upstream_subject_for_request,
    tool_execute_scope_allowed,
};
#[cfg(feature = "gateway")]
use crate::mcp::handlers_resources::admin_app_resources_visible;
use crate::mcp::peer_contract::{PeerCatalogAudience, PeerContract};
#[cfg(all(test, feature = "proxy-testkit"))]
use crate::mcp::prompts::list_all as list_builtin_prompts;

/// Primary Code Mode tool. It has no static UI but can return a nested upstream MCP App.
pub(crate) const CODE_MODE_TOOL_NAME: &str = "codemode";
/// Read-only Code Mode entry point. The broker enforces upstream annotations.
pub(crate) const CODE_MODE_READ_TOOL_NAME: &str = "codemode_read";
/// Explicit Code Mode MCP App entry point.
pub(crate) const CODE_MODE_UI_TOOL_NAME: &str = "codemode_ui";
/// Text-only management tool for the Lab-owned MCP App surface.
pub(crate) const MCP_APP_TOOL_NAME: &str = "mcp_app";
/// Shared Code Mode MCP App state for one running gateway. Every downstream MCP
/// session receives a clone, while independent gateways and tests remain isolated.
pub(crate) use labby_runtime::CodeModeAppState;

/// Lab-owned server process log viewer tool name.
pub(crate) const SERVER_LOGS_TOOL_NAME: &str = "server_logs";
/// Lab-owned MCP App entry point for adding a gateway upstream.
pub(crate) const ADD_SERVER_TOOL_NAME: &str = "add_server";
/// Lab-owned MCP App entry point for live gateway upstream status.
pub(crate) const GATEWAY_STATUS_TOOL_NAME: &str = "gateway_status";
/// Lab-owned MCP App entry point for schema-backed runtime settings.
pub(crate) const SETTINGS_TOOL_NAME: &str = "settings";

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

#[cfg(all(test, feature = "proxy-testkit"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogSnapshot {
    pub(crate) tools: BTreeSet<String>,
    pub(crate) resources: BTreeSet<String>,
    pub(crate) prompts: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolCatalogSnapshot {
    pub(crate) tools: BTreeSet<String>,
    /// SHA-256 of the canonical, post-filter descriptor set presented to this
    /// peer. The digest includes every serialized Tool field, including schemas,
    /// annotations, and `_meta`, and excludes gateway runtime state by construction.
    pub(crate) contract_hash: [u8; 32],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CatalogChangeSet {
    pub(crate) tools_changed: bool,
    pub(crate) resources_changed: bool,
    pub(crate) prompts_changed: bool,
}

impl ToolCatalogSnapshot {
    #[must_use]
    pub(crate) fn from_descriptors(descriptors: &[Tool]) -> Self {
        let tools = descriptors
            .iter()
            .map(|tool| tool.name.as_ref().to_string())
            .collect();
        Self {
            tools,
            contract_hash: descriptor_contract_hash(descriptors),
        }
    }

    #[cfg(test)]
    #[must_use]
    #[allow(clippy::disallowed_methods)] // test helper constructs bare Tool values directly
    pub(crate) fn from_names(tools: BTreeSet<String>) -> Self {
        let descriptors = tools
            .iter()
            .map(|name| Tool::new(name.clone(), "", Arc::new(serde_json::Map::new())))
            .collect::<Vec<_>>();
        Self::from_descriptors(&descriptors)
    }

    pub(crate) fn changes_since(&self, before: &Self) -> CatalogChangeSet {
        CatalogChangeSet {
            tools_changed: before.contract_hash != self.contract_hash,
            resources_changed: false,
            prompts_changed: false,
        }
    }
}

fn descriptor_contract_hash(descriptors: &[Tool]) -> [u8; 32] {
    let mut canonical = descriptors
        .iter()
        .map(|tool| {
            let value = serde_json::to_value(tool).unwrap_or(Value::Null);
            let value = canonicalize_json(value);
            let bytes = serde_json::to_vec(&value).unwrap_or_default();
            (tool.name.as_ref().to_string(), bytes)
        })
        .collect::<Vec<_>>();
    canonical.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

    let mut hasher = Sha256::new();
    for (name, descriptor) in canonical {
        hash_len_prefixed(&mut hasher, name.as_bytes());
        hash_len_prefixed(&mut hasher, &descriptor);
    }
    hasher.finalize().into()
}

fn hash_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let mut canonical = serde_json::Map::new();
            for (key, value) in entries {
                canonical.insert(key, canonicalize_json(value));
            }
            Value::Object(canonical)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        scalar => scalar,
    }
}

#[allow(dead_code)]
pub(crate) fn upstream_name_for_uri(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix("lab://upstream/")?;
    let slash_pos = rest.find('/')?;
    Some(&rest[..slash_pos])
}

impl LabMcpServer {
    /// This session's visible-contract inputs, in the form the notification
    /// fanout can hold onto and re-evaluate later. See `peer_contract.rs`.
    pub(crate) fn peer_contract(&self) -> PeerContract {
        PeerContract {
            registry: Arc::clone(&self.registry),
            #[cfg(feature = "gateway")]
            gateway_manager: self.gateway_manager.clone(),
            route_scope: self.route_scope.clone(),
            code_mode_app_state: self.code_mode_app_state.clone(),
            audience: PeerCatalogAudience::default(),
        }
    }

    pub(crate) fn peer_contract_for_request(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> PeerContract {
        #[cfg(feature = "gateway")]
        let audience = {
            let auth = auth_context_from_extensions(&context.extensions);
            PeerCatalogAudience {
                code_mode_read_allowed: code_mode_read_scope_allowed(auth),
                code_mode_execute_allowed: tool_execute_scope_allowed(auth),
                admin_apps_visible: admin_app_resources_visible(auth),
                #[cfg(feature = "skills")]
                skill_library_management_visible: self
                    .skill_library_http_management_visible(context),
                #[cfg(not(feature = "skills"))]
                skill_library_management_visible: false,
                #[cfg(feature = "skills")]
                skill_library_app_visible: self.skill_library_http_management_visible(context)
                    && code_mode_read_scope_allowed(auth),
                #[cfg(not(feature = "skills"))]
                skill_library_app_visible: false,
                oauth_subject: oauth_upstream_subject_for_request(
                    auth,
                    self.request_subject(context),
                )
                .map(std::borrow::Cow::into_owned),
                project_listing: crate::mcp::peer_contract::ProjectPeerListing::from_extensions(
                    &context.extensions,
                ),
            }
        };
        #[cfg(not(feature = "gateway"))]
        let audience = PeerCatalogAudience::default();

        PeerContract {
            registry: Arc::clone(&self.registry),
            #[cfg(feature = "gateway")]
            gateway_manager: self.gateway_manager.clone(),
            route_scope: self.route_scope.clone(),
            code_mode_app_state: self.code_mode_app_state.clone(),
            audience,
        }
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn current_upstream_pool(&self) -> Option<Arc<UpstreamPool>> {
        self.peer_contract().current_upstream_pool().await
    }

    // FR-2a: these four delegate to the shared audience-free gates in
    // `peer_contract.rs` over borrowed fields — no `self.peer_contract()`
    // construction (that clones a deep `McpRouteScope` per call).

    pub(crate) async fn service_visible_on_mcp(&self, service: &str) -> bool {
        if self.registry.service(service).is_some() && !self.registry.supports_mcp_dispatch(service)
        {
            return false;
        }
        #[cfg(feature = "gateway")]
        {
            crate::mcp::peer_contract::mcp_service_visible(
                &self.route_scope,
                self.gateway_manager.as_deref(),
                service,
            )
            .await
        }
        #[cfg(not(feature = "gateway"))]
        {
            crate::mcp::peer_contract::route_allows_mcp_service(&self.route_scope, service)
        }
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn mcp_apps_config(&self) -> labby_runtime::gateway_config::McpAppsConfig {
        crate::mcp::peer_contract::mcp_apps_config(self.gateway_manager.as_deref()).await
    }

    /// Authoritative Code Mode app visibility for runtime access checks.
    ///
    /// Manager-backed servers read the published configuration directly so a
    /// config mutation cannot race the mirrored session atomic. In-process
    /// servers without a manager retain the shared atomic as their authority.
    pub(crate) async fn code_mode_app_enabled_on_mcp(&self) -> bool {
        #[cfg(feature = "gateway")]
        if let Some(manager) = self.gateway_manager.as_ref() {
            return manager.code_mode_config().await.mcp_ui_enabled;
        }
        self.code_mode_app_state.is_enabled()
    }

    pub(crate) async fn action_allowed_on_mcp(&self, service: &str, action: &str) -> bool {
        #[cfg(feature = "gateway")]
        {
            crate::mcp::peer_contract::mcp_action_allowed(
                self.gateway_manager.as_deref(),
                service,
                action,
            )
            .await
        }
        #[cfg(not(feature = "gateway"))]
        {
            let _ = (service, action);
            true
        }
    }

    #[cfg(feature = "gateway")]
    /// Whether the current route can safely advertise and execute Add Server.
    pub(crate) async fn add_server_app_available_on_mcp(&self) -> bool {
        let apps = self.mcp_apps_config().await;
        self.add_server_app_available_on_mcp_with(apps).await
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn add_server_app_available_on_mcp_with(
        &self,
        apps: labby_runtime::gateway_config::McpAppsConfig,
    ) -> bool {
        crate::mcp::peer_contract::add_server_app_available(
            &self.route_scope,
            self.gateway_manager.as_deref(),
            &self.registry,
            apps,
        )
        .await
    }

    #[cfg(feature = "gateway")]
    /// Whether the current route can safely advertise live gateway status.
    pub(crate) async fn gateway_status_app_available_on_mcp(&self) -> bool {
        let apps = self.mcp_apps_config().await;
        self.gateway_status_app_available_on_mcp_with(apps).await
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn gateway_status_app_available_on_mcp_with(
        &self,
        apps: labby_runtime::gateway_config::McpAppsConfig,
    ) -> bool {
        crate::mcp::peer_contract::gateway_status_app_available(
            &self.route_scope,
            self.gateway_manager.as_deref(),
            &self.registry,
            apps,
        )
        .await
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
        self.peer_contract().code_mode_visibility().await
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

    #[cfg(all(test, feature = "proxy-testkit"))]
    pub(crate) fn builtin_prompt_names(&self) -> Vec<String> {
        list_builtin_prompts()
            .prompts
            .iter()
            .map(|prompt| prompt.name.to_string())
            .collect()
    }

    #[cfg(all(test, feature = "proxy-testkit"))]
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

    #[cfg(all(test, feature = "proxy-testkit"))]
    pub(crate) async fn snapshot_catalog(&self) -> CatalogSnapshot {
        let visibility = self.code_mode_visibility().await;
        let mut tools = BTreeSet::new();
        if visibility.exposes_synthetic_tools() {
            tools.insert(CODE_MODE_READ_TOOL_NAME.to_string());
            tools.insert(CODE_MODE_TOOL_NAME.to_string());
            tools.insert(MCP_APP_TOOL_NAME.to_string());
            if self.code_mode_app_enabled_on_mcp().await {
                tools.insert(CODE_MODE_UI_TOOL_NAME.to_string());
            }
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

    /// Full client-visible tool contract for trusted/local test paths.
    #[cfg(all(test, feature = "proxy-testkit"))]
    pub(crate) async fn snapshot_tool_catalog(&self) -> ToolCatalogSnapshot {
        self.peer_contract().visible_contract().await
    }

    /// Full client-visible tool contract for one authenticated request.
    pub(crate) async fn snapshot_tool_catalog_for_request(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> ToolCatalogSnapshot {
        self.peer_contract_for_request(context)
            .visible_contract()
            .await
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use super::*;

    // ── Tool name constants (Cloudflare-parity, no aliases) ─────────────────

    #[test]
    fn canonical_code_mode_tool_names_are_stable() {
        assert_eq!(CODE_MODE_TOOL_NAME, "codemode");
        assert_eq!(CODE_MODE_READ_TOOL_NAME, "codemode_read");
        assert_eq!(CODE_MODE_UI_TOOL_NAME, "codemode_ui");
        assert_eq!(MCP_APP_TOOL_NAME, "mcp_app");
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

    fn schema(properties: Value) -> Arc<serde_json::Map<String, Value>> {
        Arc::new(
            serde_json::json!({ "type": "object", "properties": properties })
                .as_object()
                .expect("schema object")
                .clone(),
        )
    }

    fn descriptor(description: &str) -> Tool {
        Tool::new(
            "alpha",
            description.to_string(),
            schema(serde_json::json!({ "value": { "type": "string" } })),
        )
    }

    #[test]
    fn descriptor_only_change_reports_tool_change() {
        let before = ToolCatalogSnapshot::from_descriptors(&[descriptor("before")]);
        let after = ToolCatalogSnapshot::from_descriptors(&[descriptor("after")]);

        assert_eq!(before.tools, after.tools);
        assert_eq!(
            after.changes_since(&before),
            CatalogChangeSet {
                tools_changed: true,
                resources_changed: false,
                prompts_changed: false,
            }
        );
    }

    #[test]
    fn schema_annotation_and_meta_changes_affect_contract_hash() {
        let base = descriptor("same");
        let schema_changed = Tool::new(
            "alpha",
            "same",
            schema(serde_json::json!({ "value": { "type": "integer" } })),
        );
        let annotation_changed = descriptor("same")
            .with_annotations(rmcp::model::ToolAnnotations::new().read_only(true));
        let mut meta = serde_json::Map::new();
        meta.insert(
            "ui/resourceUri".to_string(),
            Value::String("ui://alpha".to_string()),
        );
        let meta_changed = descriptor("same").with_meta(rmcp::model::MetaObject(meta));

        let base = ToolCatalogSnapshot::from_descriptors(&[base]);
        for changed in [schema_changed, annotation_changed, meta_changed] {
            let changed = ToolCatalogSnapshot::from_descriptors(&[changed]);
            assert_ne!(base.contract_hash, changed.contract_hash);
        }
    }

    #[test]
    fn descriptor_and_json_object_order_do_not_affect_contract_hash() {
        let left = Tool::new(
            "alpha",
            "same",
            schema(serde_json::json!({
                "a": { "type": "string" },
                "b": { "type": "integer" }
            })),
        );
        let right = Tool::new(
            "beta",
            "same",
            schema(serde_json::json!({ "value": { "type": "boolean" } })),
        );
        let reordered_alpha = Tool::new(
            "alpha",
            "same",
            schema(serde_json::json!({
                "b": { "type": "integer" },
                "a": { "type": "string" }
            })),
        );

        let first = ToolCatalogSnapshot::from_descriptors(&[left, right.clone()]);
        let second = ToolCatalogSnapshot::from_descriptors(&[right, reordered_alpha]);
        assert_eq!(first.contract_hash, second.contract_hash);
        assert!(!second.changes_since(&first).tools_changed);
    }

    #[test]
    fn tool_name_changes_remain_available_for_diagnostics() {
        let before = ToolCatalogSnapshot::from_names(BTreeSet::from(["a".to_string()]));
        let after =
            ToolCatalogSnapshot::from_names(BTreeSet::from(["a".to_string(), "b".to_string()]));
        assert!(after.changes_since(&before).tools_changed);
        assert_eq!(
            after
                .tools
                .difference(&before.tools)
                .cloned()
                .collect::<Vec<_>>(),
            ["b"]
        );
    }
}
