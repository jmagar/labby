//! `list_tools` handler body + gateway meta-tool input-schema construction.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.4`) as an inherent
//! `impl LabMcpServer` method. The `ServerHandler` trait impl in
//! `server.rs` keeps a one-line delegator.
//!
//! The Code Mode tool description has exactly one definition; this module
//! imports it for `list_tools`.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};
use std::time::Instant;
#[cfg(feature = "gateway")]
use std::time::SystemTime;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::MetaObject;
use rmcp::model::{ListToolsResult, PaginatedRequestParams};
use rmcp::service::RequestContext;
use serde_json::Value;

#[cfg(feature = "gateway")]
use crate::dispatch::upstream::pool::MAX_UPSTREAM_TOOLS;
#[cfg(feature = "gateway")]
use crate::mcp::bound_access::{
    ProjectDiscoveryShadow, ProjectExecutionBinding, project_execution_binding,
};
#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::CodeModeUpstreamDescription;
#[cfg(feature = "gateway")]
use crate::mcp::catalog::{
    ADD_SERVER_TOOL_NAME, CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME, CODE_MODE_UI_TOOL_NAME,
    GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME, SETTINGS_TOOL_NAME,
};
use crate::mcp::catalog::{SERVER_LOGS_TOOL_NAME, ToolCatalogSnapshot};
#[cfg(feature = "gateway")]
use crate::mcp::context::oauth_upstream_subject_for_request;
#[cfg(feature = "gateway")]
use crate::mcp::context::tool_execute_scope_allowed;
#[cfg(any(feature = "gateway", feature = "skills"))]
use crate::mcp::context::{auth_context_from_extensions, code_mode_read_scope_allowed};
#[cfg(feature = "gateway")]
use crate::mcp::handlers_resources::{
    add_server_app_resource_uri_for_tool, add_server_app_skybridge_uri_for_tool,
    code_mode_app_resource_uri_for_tool, code_mode_app_skybridge_uri_for_tool,
    gateway_status_app_resource_uri_for_tool, gateway_status_app_skybridge_uri_for_tool,
    mcp_apps_app_resource_uri_for_tool, mcp_apps_app_skybridge_uri_for_tool,
    settings_app_resource_uri_for_tool, settings_app_skybridge_uri_for_tool,
};
use crate::mcp::handlers_resources::{
    admin_app_resources_visible, server_logs_app_resource_uri_for_tool,
    server_logs_app_skybridge_uri_for_tool,
};
#[cfg(feature = "skills")]
use crate::mcp::handlers_resources::{
    skill_library_app_resource_uri_for_tool, skill_library_app_skybridge_uri_for_tool,
};
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};
use crate::mcp::pagination::{PageCollector, error_kind as pagination_error_kind};
use crate::mcp::permanent_tools::SkillLibraryDescriptorMode;
use crate::mcp::server::LabMcpServer;

/// Remove MCP App bindings whose backing resources are not readable on the
/// current route. Keep unrelated metadata intact.
pub(crate) fn strip_resource_backed_ui_meta(meta: &mut Option<MetaObject>) {
    let should_clear = if let Some(meta) = meta.as_mut() {
        meta.0.remove("ui");
        meta.0.remove("openai/outputTemplate");
        meta.0.is_empty()
    } else {
        false
    };
    if should_clear {
        *meta = None;
    }
}

impl LabMcpServer {
    async fn unavailable_project_tool_list(
        &self,
        context: &RequestContext<RoleServer>,
        start: Instant,
    ) -> ListToolsResult {
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_tools",
            elapsed_ms,
            project_binding = "unavailable",
            page_tool_count = 0,
            has_next_cursor = false,
            "tool list unavailable for Project transport"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "list_tools",
            elapsed_ms,
            DispatchLogOutcome::Failure {
                level: LoggingLevel::Warning,
                kind: "access_context_unavailable",
            },
        )
        .await;
        ListToolsResult::with_all_items(Vec::new())
            .with_ttl_ms(0)
            .with_cache_scope(rmcp::model::CacheScope::Private)
    }

    pub(crate) async fn list_tools_impl(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_tools",
            subject,
            "dispatch start"
        );
        #[cfg(feature = "gateway")]
        let project_shadow = match project_execution_binding(&context.extensions, SystemTime::now())
        {
            ProjectExecutionBinding::Legacy => ProjectDiscoveryShadow::Legacy,
            ProjectExecutionBinding::Unavailable => {
                return Ok(self.unavailable_project_tool_list(&context, start).await);
            }
            ProjectExecutionBinding::Bound { transport, .. } => {
                ProjectDiscoveryShadow::Bound(transport)
            }
        };
        #[cfg(feature = "gateway")]
        let project_cursor_binding = match &project_shadow {
            ProjectDiscoveryShadow::Legacy => None,
            ProjectDiscoveryShadow::Unavailable => unreachable!("unavailable returned above"),
            ProjectDiscoveryShadow::Bound(_) => {
                let Some(binding) = project_shadow.cursor_binding_fingerprint(SystemTime::now())
                else {
                    return Ok(self.unavailable_project_tool_list(&context, start).await);
                };
                Some(binding)
            }
        };
        let page_collector = match PageCollector::new(request) {
            Ok(collector) => collector,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let kind = pagination_error_kind(&error);
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_tools",
                    subject,
                    elapsed_ms,
                    kind,
                    "tool list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_tools",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind,
                    },
                )
                .await;
                return Err(error);
            }
        };
        let mut descriptors = Vec::new();
        let mut advertised_names = HashSet::new();
        let mut builtin_tool_count = 0usize;
        let mut upstream_tool_count = 0usize;
        let mut subject_scoped_tool_count = 0usize;
        let mut gateway_tool_count = 0usize;
        let upstream_ui_tool_count = 0usize;
        let mut suppressed_builtin_tool_count = 0usize;
        #[cfg(feature = "gateway")]
        let mut project_shadow_checked_tool_count = 0usize;
        #[cfg(not(feature = "gateway"))]
        let project_shadow_checked_tool_count = 0usize;
        #[cfg(feature = "gateway")]
        let mut project_shadow_would_suppress_tool_count = 0usize;
        #[cfg(not(feature = "gateway"))]
        let project_shadow_would_suppress_tool_count = 0usize;
        let mut pool_present = false;
        let mut catalog_upstream_count = 0usize;
        let mut upstream_tool_error_count = 0usize;
        let mut open_upstream_count = 0usize;
        // FU-2 (issue #210, lab-ecxfl): one PeerContract for the whole listing.
        // The three consumers below (visibility, Code Mode upstream
        // descriptions, upstream pool) are audience-independent, so hoisting
        // is behavior-neutral. The clone cost is only real on ProtectedSubset
        // routes — `Root` is a unit variant.
        let peer_contract = self.peer_contract();
        let visibility = peer_contract.code_mode_visibility().await;
        let manager_code_mode_enabled = visibility.exposes_synthetic_tools();
        let process_code_mode_enabled = crate::config::process_code_mode_enabled();
        let hide_raw_tools = visibility.hides_raw_tools();
        let visibility_mode = visibility.mode_label();
        #[cfg(feature = "gateway")]
        if !hide_raw_tools {
            peer_contract.ensure_protected_subset_tools().await;
        }
        #[cfg(feature = "gateway")]
        let auth = auth_context_from_extensions(&context.extensions);
        #[cfg(feature = "gateway")]
        let (code_mode_app_enabled, mcp_apps_config) =
            crate::mcp::peer_contract::mcp_app_visibility_snapshot(
                self.gateway_manager.as_deref(),
                &self.code_mode_app_state,
            )
            .await;
        let server_logs_app_visible = {
            #[cfg(feature = "gateway")]
            {
                mcp_apps_config.server_logs && admin_app_resources_visible(auth)
            }
            #[cfg(not(feature = "gateway"))]
            {
                true
            }
        };
        #[cfg(feature = "gateway")]
        let add_server_app_visible = admin_app_resources_visible(auth)
            && self
                .add_server_app_available_on_mcp_with(mcp_apps_config)
                .await;
        #[cfg(feature = "gateway")]
        let gateway_status_app_visible = admin_app_resources_visible(auth)
            && self
                .gateway_status_app_available_on_mcp_with(mcp_apps_config)
                .await;
        #[cfg(feature = "gateway")]
        let settings_app_visible = mcp_apps_config.settings
            && admin_app_resources_visible(auth)
            && self.route_scope.allows_service("setup")
            && self.service_visible_on_mcp("setup").await;
        let mut builtin_names = HashSet::new();
        #[cfg(feature = "skills")]
        let skill_library_allowed_actions = self.allowed_mcp_actions("artifacts").await;
        #[cfg(feature = "skills")]
        let skill_library_mode = if self.skill_library_http_management_visible(&context) {
            let skills_auth = auth_context_from_extensions(&context.extensions);
            SkillLibraryDescriptorMode::Management {
                app_visible: code_mode_read_scope_allowed(skills_auth)
                    && self.route_scope.exposes_resources(),
                allowed_actions: skill_library_allowed_actions.as_deref(),
            }
        } else {
            SkillLibraryDescriptorMode::Hidden
        };
        #[cfg(not(feature = "skills"))]
        let skill_library_mode = SkillLibraryDescriptorMode::Hidden;
        for svc in self.registry.services() {
            // `service_visible_on_mcp` already checks `route_scope.allows_service`.
            if self.service_visible_on_mcp(svc.name).await {
                #[cfg(feature = "gateway")]
                if matches!(&project_shadow, ProjectDiscoveryShadow::Bound(_)) {
                    project_shadow_checked_tool_count += 1;
                    if project_shadow.allows_builtin_service_descriptor(svc, SystemTime::now())
                        != Some(true)
                    {
                        project_shadow_would_suppress_tool_count += 1;
                        continue;
                    }
                }
                builtin_names.insert(svc.name.to_string());
                if hide_raw_tools && svc.name != SERVER_LOGS_TOOL_NAME {
                    suppressed_builtin_tool_count += 1;
                } else {
                    advertised_names.insert(svc.name.to_string());
                    descriptors.push(self.registry.permanent_tools().builtin_service_tool(
                        svc,
                        server_logs_app_visible,
                        skill_library_mode,
                    ));
                    builtin_tool_count += 1;
                }
            }
        }
        // Assemble and deduplicate the complete visible contract before pagination. Offset
        // cursors are only safe when every catalog rebuild produces the same global order.
        #[cfg(feature = "gateway")]
        if visibility.exposes_synthetic_tools()
            && (!matches!(&project_shadow, ProjectDiscoveryShadow::Bound(_))
                || project_shadow.allows_code_mode_tools(SystemTime::now()) == Some(true))
            && (code_mode_read_scope_allowed(auth) || tool_execute_scope_allowed(auth))
        {
            // ── Gateway Code Mode tool. It takes `{ code, upstreams?, tools? }`
            // and exposes in-sandbox discovery through `codemode.search()` /
            // `codemode.describe()`.
            // See mcp/CLAUDE.md for the exception rationale and
            // dispatch/gateway/dispatch.rs guard.
            let code_mode_upstreams =
                crate::mcp::peer_contract::project_code_mode_description_upstreams(
                    matches!(&project_shadow, ProjectDiscoveryShadow::Bound(_)),
                    peer_contract.code_mode_upstreams_for_description().await,
                );
            if code_mode_read_scope_allowed(auth) {
                descriptors.push(
                    self.registry
                        .permanent_tools()
                        .code_mode_read_descriptor(&code_mode_upstreams),
                );
                advertised_names.insert(CODE_MODE_READ_TOOL_NAME.to_string());
                gateway_tool_count += 1;
            }

            if tool_execute_scope_allowed(auth) {
                let descriptor = self
                    .registry
                    .permanent_tools()
                    .code_mode_descriptor(&code_mode_upstreams);
                tracing::info!(
                    surface = "mcp",
                    service = labby_codemode::SERVICE,
                    action = "tool.describe",
                    description_bytes =
                        descriptor.description.as_deref().map(str::len).unwrap_or(0),
                    "registered primary Code Mode description"
                );
                descriptors.push(descriptor);
                advertised_names.insert(CODE_MODE_TOOL_NAME.to_string());
                gateway_tool_count += 1;

                if code_mode_app_enabled {
                    let codemode_resource_uri =
                        code_mode_app_resource_uri_for_tool(CODE_MODE_UI_TOOL_NAME)
                            .unwrap_or_else(|| "<missing>".to_string());
                    let codemode_skybridge_uri =
                        code_mode_app_skybridge_uri_for_tool(CODE_MODE_UI_TOOL_NAME)
                            .unwrap_or_else(|| "<missing>".to_string());
                    tracing::info!(
                        surface = "mcp",
                        service = labby_codemode::SERVICE,
                        action = "mcp_app.advertise",
                        tool = CODE_MODE_UI_TOOL_NAME,
                        resource_uri = %codemode_resource_uri,
                        skybridge_uri = %codemode_skybridge_uri,
                        "advertised explicit Code Mode MCP app tool"
                    );
                    descriptors.push(
                        self.registry
                            .permanent_tools()
                            .code_mode_ui_tool(&code_mode_upstreams),
                    );
                    advertised_names.insert(CODE_MODE_UI_TOOL_NAME.to_string());
                    gateway_tool_count += 1;
                }
            }
        }

        #[cfg(feature = "gateway")]
        if self.route_scope.is_root() && tool_execute_scope_allowed(auth) {
            descriptors.push(
                self.registry
                    .permanent_tools()
                    .mcp_app_tool(mcp_apps_config.manager),
            );
            advertised_names.insert(MCP_APP_TOOL_NAME.to_string());
            gateway_tool_count += 1;
        }

        #[cfg(feature = "gateway")]
        if add_server_app_visible
            && (!matches!(&project_shadow, ProjectDiscoveryShadow::Bound(_))
                || project_shadow.allows_builtin_service("gateway", SystemTime::now())
                    == Some(true))
        {
            descriptors.push(self.registry.permanent_tools().add_server_tool());
            advertised_names.insert(ADD_SERVER_TOOL_NAME.to_string());
            gateway_tool_count += 1;
        }

        #[cfg(feature = "gateway")]
        if gateway_status_app_visible
            && (!matches!(&project_shadow, ProjectDiscoveryShadow::Bound(_))
                || project_shadow.allows_builtin_service("gateway", SystemTime::now())
                    == Some(true))
        {
            descriptors.push(self.registry.permanent_tools().gateway_status_tool());
            advertised_names.insert(GATEWAY_STATUS_TOOL_NAME.to_string());
            gateway_tool_count += 1;
        }

        #[cfg(feature = "gateway")]
        if settings_app_visible
            && (!matches!(&project_shadow, ProjectDiscoveryShadow::Bound(_))
                || project_shadow.allows_builtin_service("setup", SystemTime::now()) == Some(true))
        {
            descriptors.push(self.registry.permanent_tools().settings_tool());
            advertised_names.insert(SETTINGS_TOOL_NAME.to_string());
            gateway_tool_count += 1;
        }

        // Merge upstream tools from the already-healthy catalog only. The
        // hidden-raw-tools path must never cold-connect upstreams: a single
        // slow or unhealthy server can otherwise stall the host's tool refresh
        // and make Labby's synthetic Code Mode tool appear to disappear. Code
        // Mode execution/search still performs cold discovery through the
        // gateway manager when the caller asks for upstream catalog data.
        #[cfg(feature = "gateway")]
        if let Some(pool) = peer_contract.current_upstream_pool().await {
            pool_present = true;
            let upstream_status = pool.upstream_status().await;
            catalog_upstream_count = upstream_status.len();
            open_upstream_count = upstream_status
                .iter()
                .filter(|(_, health)| health.is_open())
                .count();
            let oauth_subject = self.route_oauth_subject(oauth_upstream_subject_for_request(
                auth,
                self.request_subject(&context),
            ));
            let oauth_configs = if oauth_subject.is_some() {
                self.route_scoped_oauth_upstream_configs().await
            } else {
                Vec::new()
            };
            let upstream_tools = if !self.route_scope.exposes_tools() {
                Vec::new()
            } else if hide_raw_tools {
                if self.route_scope.exposes_resources() {
                    pool.cached_mcp_app_tools_allowed(
                        self.route_scope.allowed_upstreams(),
                        &oauth_configs,
                        oauth_subject.as_deref(),
                        MAX_UPSTREAM_TOOLS,
                    )
                    .await
                } else {
                    Vec::new()
                }
            } else {
                pool.healthy_tools_allowed(self.route_scope.allowed_upstreams())
                    .await
            };
            for ut in upstream_tools {
                if hide_raw_tools && !tool_execute_scope_allowed(auth) && ut.destructive {
                    continue;
                }
                let tool_name = ut.tool.name.as_ref();
                if matches!(&project_shadow, ProjectDiscoveryShadow::Bound(_)) {
                    project_shadow_checked_tool_count += 1;
                    if project_shadow.allows_upstream_tool(
                        ut.upstream_name.as_ref(),
                        tool_name,
                        SystemTime::now(),
                    ) != Some(true)
                    {
                        project_shadow_would_suppress_tool_count += 1;
                        continue;
                    }
                }
                if crate::mcp::permanent_tools::is_reserved_non_upstream_tool_name(tool_name)
                    || builtin_names.contains(tool_name)
                    || !advertised_names.insert(tool_name.to_string())
                {
                    tracing::debug!(
                        surface = "mcp",
                        service = "labby",
                        action = "tool.register",
                        tool = tool_name,
                        "skipping upstream tool that collides with an already advertised tool"
                    );
                    continue;
                }
                descriptors.push(crate::mcp::permanent_tools::with_labby_security(ut.tool));
                upstream_tool_count += 1;
            }
            if !hide_raw_tools
                && self.route_scope.exposes_tools()
                && let Some(oauth_subject) = oauth_subject.as_ref()
            {
                let subject_tool_limit = MAX_UPSTREAM_TOOLS.saturating_sub(upstream_tool_count);
                for (_, upstream_tools) in pool
                    .cached_subject_scoped_tools_bounded(
                        &oauth_configs,
                        oauth_subject.as_ref(),
                        subject_tool_limit,
                    )
                    .await
                {
                    for ut in upstream_tools {
                        let tool_name = ut.name.as_ref();
                        if crate::mcp::permanent_tools::is_reserved_non_upstream_tool_name(
                            tool_name,
                        ) || builtin_names.contains(tool_name)
                            || !advertised_names.insert(tool_name.to_string())
                        {
                            continue;
                        }
                        descriptors.push(crate::mcp::permanent_tools::with_labby_security(ut));
                        subject_scoped_tool_count += 1;
                    }
                }
            }
            for (upstream, _) in &upstream_status {
                if pool.upstream_tool_last_error(upstream).await.is_some() {
                    upstream_tool_error_count += 1;
                }
            }
        }

        if !self.route_scope.exposes_resources() {
            #[cfg(feature = "gateway")]
            descriptors.retain(|descriptor| descriptor.name.as_ref() != CODE_MODE_UI_TOOL_NAME);
            for descriptor in &mut descriptors {
                strip_resource_backed_ui_meta(&mut descriptor.meta);
            }
        }

        #[cfg(feature = "gateway")]
        if !self.route_scope.exposes_tools() {
            let keep_code_mode = self.route_scope.exposes_code_mode();
            descriptors.retain(|descriptor| {
                keep_code_mode
                    && matches!(
                        descriptor.name.as_ref(),
                        CODE_MODE_TOOL_NAME
                            | CODE_MODE_READ_TOOL_NAME
                            | CODE_MODE_UI_TOOL_NAME
                            | MCP_APP_TOOL_NAME
                    )
            });
        }
        descriptors.sort_by(|left, right| left.name.cmp(&right.name));
        #[cfg(feature = "gateway")]
        if matches!(&project_shadow, ProjectDiscoveryShadow::Bound(_))
            && project_shadow.cursor_binding_fingerprint(SystemTime::now())
                != project_cursor_binding
        {
            return Ok(self.unavailable_project_tool_list(&context, start).await);
        }
        let mut page_collector = page_collector;
        let complete_contract = ToolCatalogSnapshot::from_descriptors(&descriptors);
        let descriptor_revision = hex::encode(complete_contract.contract_hash);
        #[cfg(feature = "gateway")]
        let contract_revision =
            project_cursor_binding
                .as_ref()
                .map_or(descriptor_revision.clone(), |binding| {
                    labby_auth::util::fingerprint(&format!(
                        "labby.mcp.project-tools-result.v1\0{binding}\0{descriptor_revision}"
                    ))
                });
        #[cfg(not(feature = "gateway"))]
        let contract_revision = descriptor_revision;
        if let Err(error) = page_collector.bind_revision(&contract_revision) {
            let elapsed_ms = start.elapsed().as_millis();
            let kind = pagination_error_kind(&error);
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "list_tools",
                subject,
                elapsed_ms,
                kind,
                "tool list failed"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "list_tools",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind,
                },
            )
            .await;
            return Err(error);
        }
        for descriptor in descriptors.iter().cloned() {
            page_collector.accept(descriptor);
            if page_collector.finished() {
                break;
            }
        }
        let (tools, next_cursor) = match page_collector.finish() {
            Ok(page) => page,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let kind = pagination_error_kind(&error);
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_tools",
                    subject,
                    elapsed_ms,
                    kind,
                    "tool list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_tools",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind,
                    },
                )
                .await;
                return Err(error);
            }
        };
        let page_tool_count = tools.len();
        let has_next_cursor = next_cursor.is_some();
        if !has_next_cursor && self.transport_label != "http" {
            let subject_key = self.request_subject(&context).map(str::to_owned);
            self.last_listed_tool_contract
                .write()
                .await
                .publish(subject_key, complete_contract);
        }

        #[cfg(feature = "gateway")]
        let project_shadow_state = project_shadow.state_label_at(SystemTime::now());
        #[cfg(not(feature = "gateway"))]
        let project_shadow_state = "legacy";
        #[cfg(feature = "gateway")]
        if project_shadow_state != "bound" {
            project_shadow_checked_tool_count = 0;
            project_shadow_would_suppress_tool_count = 0;
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_tools",
            subject,
            elapsed_ms,
            builtin_tool_count,
            gateway_tool_count,
            upstream_tool_count,
            upstream_ui_tool_count,
            subject_scoped_tool_count,
            suppressed_builtin_tool_count,
            pool_present,
            cold_discovery_skipped = hide_raw_tools,
            oauth_subject_catalog_source = "cached_only",
            upstream_catalog_source = if pool_present {
                "cached"
            } else {
                "not_initialized"
            },
            catalog_upstream_count,
            open_upstream_count,
            upstream_tool_error_count,
            manager_code_mode_enabled,
            process_code_mode_enabled,
            hide_raw_tools,
            visibility_mode,
            project_shadow_state,
            project_shadow_checked_tool_count,
            project_shadow_would_suppress_tool_count,
            page_tool_count,
            has_next_cursor,
            "tool list ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "list_tools",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        let mut result = ListToolsResult::with_all_items(tools)
            .with_ttl_ms(0)
            .with_cache_scope(rmcp::model::CacheScope::Private);
        result.next_cursor = next_cursor;
        Ok(result)
    }
}

/// The note appended to the `codemode` descriptor.
///
/// Shared with `PermanentToolRegistry::code_mode_descriptor` so the advertised
/// description and the hashed peer contract can never disagree.
#[cfg(feature = "gateway")]
pub(crate) fn code_mode_app_text_note() -> String {
    format!(
        "This entry point has no static Labby UI, but nested upstream MCP Apps attach dynamically when a called tool returns `_meta.ui`. When advertised, use `{CODE_MODE_UI_TOOL_NAME}` for the visual trace inspector; `{MCP_APP_TOOL_NAME}` can inspect or restore that Labby-owned app surface."
    )
}

/// Description for the optional `codemode_ui` MCP App twin.
#[cfg(feature = "gateway")]
pub(crate) fn code_mode_ui_description(upstreams: &[CodeModeUpstreamDescription]) -> String {
    crate::mcp::call_tool_codemode::code_mode_description_with_suffix(
        upstreams,
        &format!(
            "This explicit UI entry point renders the Code Mode trace inspector. Use `{CODE_MODE_TOOL_NAME}` when nested upstream MCP Apps should become the active result UI."
        ),
    )
}

/// Description for the always-available `mcp_app` control tool.
#[cfg(feature = "gateway")]
pub(crate) const fn mcp_app_tool_description() -> &'static str {
    "Enable, disable, and inspect Labby-owned MCP App surfaces. The control tool remains available even when its own manager UI is disabled. Targets include the manager UI, Code Mode inspector, gateway status, server logs, Add Server, Settings, or all managed apps."
}

#[cfg(feature = "gateway")]
pub(crate) fn mcp_app_tool_schema() -> Arc<serde_json::Map<String, Value>> {
    static SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(|| {
        let Value::Object(schema) = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["status", "enable", "disable"],
                    "default": "status",
                    "description": "Inspect or change whether one or more Labby-owned MCP Apps are advertised."
                },
                "target": {
                    "type": "string",
                    "enum": ["manager", "codemode", "gateway_status", "server_logs", "add_server", "settings", "all"],
                    "default": "codemode",
                    "description": "Legacy direct target shape. Use all for the switchboard snapshot or a bulk change."
                },
                "params": {
                    "type": "object",
                    "properties": {
                        "target": {
                            "type": "string",
                            "enum": ["manager", "codemode", "gateway_status", "server_logs", "add_server", "settings", "all"],
                            "default": "codemode",
                            "description": "Labby-owned MCP App target used by the shared app host."
                        }
                    },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        }) else {
            unreachable!("MCP App management schema is an object")
        };
        Arc::new(schema)
    });
    Arc::clone(&SCHEMA)
}

#[cfg(feature = "gateway")]
/// Build MCP Apps metadata for the opt-in MCP App manager UI.
pub(crate) fn mcp_app_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = mcp_apps_app_resource_uri_for_tool(tool_name)
        .expect("MCP App manager must have an associated UI resource");
    owned_app_tool_meta(resource_uri, mcp_apps_app_skybridge_uri_for_tool(tool_name))
}

#[cfg(feature = "gateway")]
/// Build MCP Apps metadata for the explicit Code Mode UI tool.
pub(crate) fn code_mode_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = code_mode_app_resource_uri_for_tool(tool_name)
        .expect("Code Mode tools must have an associated UI resource");
    // Anthropic / MCP Apps (SEP-1724) binding: hosts read `_meta.ui.resourceUri`.
    // OpenAI Apps SDK binding: ChatGPT / Codex hosts bind the widget via
    // `openai/outputTemplate` rather than `_meta.ui`. It points at the skybridge
    // variant of the same widget — identical HTML, served under the
    // `text/html+skybridge` MIME those hosts expect — so the Claude resource
    // stays untouched. The widget self-hydrates from `window.openai.toolOutput`.
    owned_app_tool_meta(
        resource_uri,
        code_mode_app_skybridge_uri_for_tool(tool_name),
    )
}

/// Build MCP Apps metadata for the Server Logs tool.
pub(crate) fn server_logs_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = server_logs_app_resource_uri_for_tool(tool_name)
        .expect("server log tools must have an associated UI resource");
    owned_app_tool_meta(
        resource_uri,
        server_logs_app_skybridge_uri_for_tool(tool_name),
    )
}

#[cfg(feature = "gateway")]
/// Build MCP Apps metadata for the synthetic Add Server tool.
pub(crate) fn add_server_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = add_server_app_resource_uri_for_tool(tool_name)
        .expect("Add Server tool must have an associated UI resource");
    owned_app_tool_meta(
        resource_uri,
        add_server_app_skybridge_uri_for_tool(tool_name),
    )
}

#[cfg(feature = "gateway")]
/// Build MCP Apps metadata for the synthetic Gateway Status tool.
pub(crate) fn gateway_status_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = gateway_status_app_resource_uri_for_tool(tool_name)
        .expect("Gateway Status tool must have an associated UI resource");
    owned_app_tool_meta(
        resource_uri,
        gateway_status_app_skybridge_uri_for_tool(tool_name),
    )
}

#[cfg(feature = "gateway")]
pub(crate) fn settings_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = settings_app_resource_uri_for_tool(tool_name)
        .expect("Settings tool must have an associated UI resource");
    owned_app_tool_meta(resource_uri, settings_app_skybridge_uri_for_tool(tool_name))
}

/// Bind one tool to its MCP Apps and optional OpenAI skybridge resources.
fn owned_app_tool_meta(resource_uri: String, skybridge_uri: Option<String>) -> MetaObject {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "ui".to_string(),
        serde_json::json!({ "resourceUri": resource_uri }),
    );
    if let Some(skybridge_uri) = skybridge_uri {
        meta.insert(
            "openai/outputTemplate".to_string(),
            serde_json::json!(skybridge_uri),
        );
    }
    MetaObject(meta)
}

/// Agent-readable fallback for hosts that do not render the attached app.
#[cfg(feature = "skills")]
pub(crate) fn skill_library_tool_description(service_description: &str) -> String {
    format!(
        "{service_description} This tool also opens Labby's Artifact Library app on compatible hosts. On non-App hosts, call the documented artifacts.* actions directly with the same action and params envelope. Save and import do not activate an Artifact."
    )
}

/// Bind the canonical `skills` service descriptor to both supported app hosts.
/// Authorization is still evaluated by the shared dispatcher at call time.
#[cfg(feature = "skills")]
pub(crate) fn skill_library_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = skill_library_app_resource_uri_for_tool(tool_name)
        .expect("Skill Library tool must have an associated UI resource");
    let mut meta = owned_app_tool_meta(
        resource_uri.clone(),
        skill_library_app_skybridge_uri_for_tool(tool_name),
    );
    meta.0.insert(
        "ui".to_string(),
        serde_json::json!({
            "resourceUri": resource_uri,
            "visibility": ["model", "app"]
        }),
    );
    meta.0
        .insert("openai/widgetAccessible".to_string(), Value::Bool(true));
    meta.0.insert(
        "openai/toolInvocation/invoking".to_string(),
        Value::String("Opening the Skill Library…".to_string()),
    );
    meta.0.insert(
        "openai/toolInvocation/invoked".to_string(),
        Value::String("Skill Library ready".to_string()),
    );
    meta.0.insert(
        "securitySchemes".to_string(),
        serde_json::json!([{
            "type": "oauth2",
            "scopes": ["lab:read", "lab", "lab:admin"]
        }]),
    );
    meta
}

#[cfg(feature = "gateway")]
/// Describe the synthetic Add Server callback contract for agent clients.
pub(crate) fn add_server_tool_schema() -> Arc<serde_json::Map<String, Value>> {
    static SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(|| {
        let Value::Object(schema) = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "test", "create"],
                    "default": "open",
                    "description": "Open the form, test a proposed server, or create it. Most callers should omit this to open the app."
                },
                "params": {
                    "type": "object",
                    "description": "For test/create, pass a proposed upstream server configuration.",
                    "required": ["spec"],
                    "properties": {
                        "spec": {
                            "type": "object",
                            "required": ["name"],
                            "properties": {
                                "name": {
                                    "type": "string",
                                    "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]*$",
                                    "description": "Unique gateway server name."
                                },
                                "url": {
                                    "type": "string",
                                    "format": "uri",
                                    "description": "HTTP(S) MCP endpoint for a remote server."
                                },
                                "command": {
                                    "type": "string",
                                    "minLength": 1,
                                    "description": "Executable for a local stdio MCP server."
                                },
                                "args": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "default": [],
                                    "description": "Arguments passed to the local stdio command."
                                },
                                "enabled": {
                                    "type": "boolean",
                                    "default": true
                                },
                                "proxy_resources": {
                                    "type": "boolean",
                                    "default": true,
                                    "description": "Expose discovered upstream resources downstream."
                                },
                                "proxy_prompts": {
                                    "type": "boolean",
                                    "default": true,
                                    "description": "Expose discovered upstream prompts downstream."
                                }
                            },
                            "oneOf": [
                                {
                                    "required": ["url"],
                                    "not": { "anyOf": [{ "required": ["command"] }, { "required": ["args"] }] }
                                },
                                {
                                    "required": ["command"],
                                    "not": { "required": ["url"] }
                                }
                            ],
                            "additionalProperties": true
                        }
                    },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        }) else {
            unreachable!("Add Server schema is an object")
        };
        Arc::new(schema)
    });
    Arc::clone(&SCHEMA)
}

#[cfg(feature = "gateway")]
/// Describe the read-only Gateway Status callback contract.
pub(crate) fn gateway_status_tool_schema() -> Arc<serde_json::Map<String, Value>> {
    static SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(|| {
        let Value::Object(schema) = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "refresh"],
                    "default": "open",
                    "description": "Open the status app or reprobe route-visible upstream tool catalogs before returning a refreshed live snapshot."
                },
                "params": {
                    "type": "object",
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        }) else {
            unreachable!("Gateway Status schema is an object")
        };
        Arc::new(schema)
    });
    Arc::clone(&SCHEMA)
}

#[cfg(feature = "gateway")]
pub(crate) fn settings_tool_schema() -> Arc<serde_json::Map<String, Value>> {
    static SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(|| {
        let Value::Object(schema) = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "schema", "state", "config.update", "env.update"],
                    "default": "open",
                    "description": "Open the settings app or invoke one of its schema-backed callbacks."
                },
                "params": { "type": "object", "additionalProperties": true }
            },
            "additionalProperties": false
        }) else {
            unreachable!("Settings schema is an object")
        };
        Arc::new(schema)
    });
    Arc::clone(&SCHEMA)
}

#[cfg(feature = "gateway")]
pub(crate) fn code_mode_execute_schema() -> Arc<serde_json::Map<String, Value>> {
    static EXECUTE_SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(
        || match serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "minLength": 1,
                    "description": "JavaScript async arrow function to execute. Use await callTool(id, params) with JSON-serializable params."
                },
                "upstreams": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional upstream allowlist for this execution."
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tool allowlist for this execution. Accepts raw tool names or <upstream>::<tool> ids."
                }
            },
            "required": ["code"]
        }) {
            Value::Object(map) => Arc::new(map),
            _ => unreachable!("execute schema must be an object"),
        },
    );
    Arc::clone(&EXECUTE_SCHEMA)
}

#[cfg(feature = "gateway")]
pub(crate) fn code_mode_trace_output_schema() -> Arc<serde_json::Map<String, Value>> {
    static TRACE_OUTPUT_SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(
        || match serde_json::json!({
        "type": "object",
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "code_mode_execute_trace" },
                    "call_count": { "type": "integer", "minimum": 0 },
                    "input_tokens": { "type": "integer", "minimum": 0 },
                    "output_tokens": { "type": "integer", "minimum": 0 },
                    "calls": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "namespace": { "type": "string" },
                                "tool": { "type": "string" },
                                "ok": { "type": "boolean" },
                                "elapsed_ms": { "type": "integer", "minimum": 0 },
                                "start_ms": { "type": "integer", "minimum": 0 },
                                "params": {},
                                "error_kind": { "type": "string" },
                                "ui": {
                                    "type": "object",
                                    "properties": {
                                        "resourceUri": {
                                            "type": "string",
                                            "description": "Native MCP UI resource URI returned by the upstream tool for this call."
                                        }
                                    },
                                    "required": ["resourceUri"],
                                    "additionalProperties": true
                                }
                            },
                            "required": ["id", "namespace", "tool", "ok", "elapsed_ms"],
                            "additionalProperties": true
                        }
                    },
                    "result": {},
                    "result_shape": { "type": "object" },
                    "result_shaping": { "type": "object" },
                    "artifacts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "absolute_path": { "type": "string" },
                                "content_type": {
                                    "type": "string",
                                    "maxLength": 256,
                                    "pattern": "^[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+$",
                                    "description": "Simple ASCII type/subtype media type for the artifact receipt."
                                },
                                "bytes": { "type": "integer", "minimum": 0 },
                                "sha256": {
                                    "type": "string",
                                    "pattern": "^[a-f0-9]{64}$"
                                }
                            },
                            "required": ["path", "absolute_path", "content_type", "bytes", "sha256"],
                            "additionalProperties": false
                        }
                    },
                    "logs_count": { "type": "integer", "minimum": 0 }
                },
                "required": ["kind", "call_count", "calls", "result_shape", "logs_count"],
                "additionalProperties": true
            }
        ]
        }) {
            Value::Object(map) => Arc::new(map),
            _ => unreachable!("trace output schema must be an object"),
        },
    );
    Arc::clone(&TRACE_OUTPUT_SCHEMA)
}

// These tests drive the live upstream pool through labby-gateway's `testkit`
// helpers. `proxy-testkit` is the documented switch that enables that feature,
// so gating here keeps labby-gateway out of the ordinary slice builds the
// feature contract exists to isolate. `--all-features` (what `just test` runs)
// turns it on.
#[cfg(test)]
#[cfg(all(feature = "gateway", feature = "proxy-testkit"))]
mod tests;
