use serde::de::DeserializeOwned;
use serde_json::Value;

use labby_runtime::dispatch_helpers::{
    action_schema, handle_builtin, help_payload, require_str, to_json,
};
use labby_runtime::error::ToolError;

use super::SHARED_GATEWAY_OAUTH_SUBJECT;
use super::catalog::ACTIONS;
use super::client::require_gateway_manager;
use super::manager::{GatewayManager, ImportTombstoneSelector};
use super::params::{
    CodeModeSetParams, GatewayAddParams, GatewayClientConfigParams, GatewayDiscoverParams,
    GatewayEnrichApplyParams, GatewayEnrichPreviewParams, GatewayImportParams,
    GatewayImportTombstoneParams, GatewayMcpCleanupParams, GatewayMcpToggleParams,
    GatewayNameParams, GatewayOauthNameParams, GatewayReloadParams, GatewayStatusParams,
    GatewayTestParams, GatewayUpdateParams, GatewayUpdatePatch, ProtectedRouteNameParams,
    ProtectedRouteSpecParams, ProtectedRouteUpdateParams, ServiceConfigGetParams,
    ServiceConfigSetParams, VirtualServerMcpPolicyParams, VirtualServerNameParams,
    VirtualServerSurfaceParams,
};
use super::types::{
    DiscoveredServerView, ImportErrorView, ImportSkipReason, ImportSkipView,
    McpClientTransportType, ServiceActionView,
};

fn parse_params<T: DeserializeOwned>(params_value: Value) -> Result<T, ToolError> {
    serde_json::from_value(params_value).map_err(|e| ToolError::InvalidParam {
        message: format!("invalid gateway params: {e}"),
        param: "params".to_string(),
    })
}

pub async fn dispatch_with_manager(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    // Defense-in-depth: built-ins handled here so direct callers of
    // dispatch_with_manager (e.g. HTTP handlers) also get the correct behavior.
    if let Some(result) = handle_builtin(action, &params_value, "gateway", ACTIONS) {
        return result;
    }
    match action {
        "gateway.code_mode.get" | "gateway.code_mode.set" => {
            handle_tool_actions(manager, action, params_value).await
        }
        "gateway.discover" => handle_discover(manager, params_value).await,
        "gateway.enrich.preview" => {
            let params: GatewayEnrichPreviewParams = parse_params(params_value)?;
            to_json(manager.preview_enrichment(params).await?)
        }
        "gateway.enrich.apply" => {
            let params: GatewayEnrichApplyParams = parse_params(params_value)?;
            to_json(manager.apply_enrichment(params).await?)
        }
        "gateway.import" => handle_import(manager, params_value).await,
        "gateway.import_pending.list" => to_json(manager.list_pending_imports().await),
        "gateway.import_pending.approve" => {
            let name = require_str(&params_value, "name")?;
            to_json(manager.approve_pending_import(name).await?)
        }
        "gateway.import_pending.reject" => {
            let name = require_str(&params_value, "name")?;
            to_json(manager.reject_pending_import(name).await?)
        }
        "gateway.import_tombstones.list"
        | "gateway.import_tombstones.clear"
        | "gateway.import_tombstones.restore" => {
            handle_import_tombstone_actions(manager, action, params_value).await
        }
        "gateway.servers" => to_json(manager.gateway_servers_doc().await?),
        "gateway.schema" => {
            let name = require_str(&params_value, "name")?;
            to_json(manager.gateway_server_schema(&name).await?)
        }
        "gateway.list"
        | "gateway.server.get"
        | "gateway.supported_services"
        | "gateway.get"
        | "gateway.test"
        | "gateway.add"
        | "gateway.update"
        | "gateway.remove"
        | "gateway.reload"
        | "gateway.status"
        | "gateway.client_config.get"
        | "gateway.discovered_tools"
        | "gateway.discovered_resources"
        | "gateway.discovered_prompts"
        | "gateway.public_urls.get" => handle_gateway_actions(manager, action, params_value).await,
        action if action.starts_with("gateway.protected_route.") => {
            handle_protected_route_actions(manager, action, params_value).await
        }
        action if action.starts_with("gateway.virtual_server.") => {
            handle_virtual_server_actions(manager, action, params_value).await
        }
        action if action.starts_with("gateway.service_") => {
            handle_service_actions(manager, action, params_value).await
        }
        action if action.starts_with("gateway.oauth.") => {
            handle_oauth_actions(manager, action, params_value).await
        }
        action if action.starts_with("gateway.mcp.") => {
            handle_mcp_actions(manager, action, params_value).await
        }
        unknown => unknown_action(unknown),
    }
}

const KNOWN_CLIENTS: &[&str] = &[
    "cursor",
    "claude-code",
    "claude-desktop",
    "codex",
    "windsurf",
    "opencode",
    "vscode",
    "gemini",
];

async fn handle_discover(
    manager: &GatewayManager,
    params_value: Value,
) -> Result<Value, ToolError> {
    let params: GatewayDiscoverParams = parse_params(params_value)?;

    for client in &params.clients {
        if !KNOWN_CLIENTS.contains(&client.as_str()) {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "unknown client kind: '{}'. Valid: {}",
                    client,
                    KNOWN_CLIENTS.join(", ")
                ),
                param: "clients".to_string(),
            });
        }
    }

    let home = super::discovery::home_dir().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "cannot determine home directory".to_string(),
    })?;

    let mut discovered = tokio::task::spawn_blocking(move || super::discovery::discover_all(&home))
        .await
        .map_err(|e| ToolError::internal_message(format!("discovery task panicked: {e}")))?;
    if !params.clients.is_empty() {
        let filter: std::collections::HashSet<&str> =
            params.clients.iter().map(String::as_str).collect();
        discovered.retain(|s| filter.contains(s.source_client.as_str()));
    }

    let cfg = manager.current_config().await;
    let existing: std::collections::HashSet<String> =
        cfg.upstream.iter().map(|u| u.name.clone()).collect();

    let views = shape_discovered_views(discovered, &cfg, &existing, &params);

    to_json(views)
}

fn shape_discovered_views(
    discovered: Vec<super::discovery::DiscoveredServer>,
    cfg: &labby_runtime::gateway_config::GatewayConfig,
    existing: &std::collections::HashSet<String>,
    params: &GatewayDiscoverParams,
) -> Vec<DiscoveredServerView> {
    discovered
        .into_iter()
        .filter(|s| params.include_existing || !existing.contains(&s.name))
        .map(|s| {
            let tombstoned = super::manager::discovered_is_tombstoned(cfg, &s);
            let transport = if s.spec.url.is_some() {
                McpClientTransportType::Http
            } else {
                McpClientTransportType::Stdio
            };
            let command_preview = s.spec.command.as_ref().map(|c| {
                c.split_whitespace()
                    .next()
                    .unwrap_or(c.as_str())
                    .to_string()
            });
            DiscoveredServerView {
                name: s.name,
                source_client: s.source_client,
                source_path: s.source_path,
                transport,
                command_preview,
                url_preview: s.spec.url.as_deref().map(redact_url_preview),
                env_key_count: s.env_key_count,
                already_configured: existing.contains(&s.spec.name),
                transport_fingerprint: s
                    .spec
                    .imported_from
                    .as_ref()
                    .and_then(|source| source.transport_fingerprint.clone()),
                tombstoned,
            }
        })
        .collect()
}

async fn handle_import(manager: &GatewayManager, params_value: Value) -> Result<Value, ToolError> {
    let params: GatewayImportParams = parse_params(params_value)?;

    if !params.names.is_empty() && params.all {
        return Err(ToolError::InvalidParam {
            message: "gateway.import requires either `all` or `names`, not both".to_string(),
            param: "names".to_string(),
        });
    }

    if params.names.is_empty() && !params.all {
        return Err(ToolError::InvalidParam {
            message: "gateway.import requires either `all: true` or a non-empty `names` list"
                .to_string(),
            param: "names".to_string(),
        });
    }

    for client in &params.clients {
        if !KNOWN_CLIENTS.contains(&client.as_str()) {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "unknown client kind: '{}'. Valid: {}",
                    client,
                    KNOWN_CLIENTS.join(", ")
                ),
                param: "clients".to_string(),
            });
        }
    }

    let home = super::discovery::home_dir().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "cannot determine home directory".to_string(),
    })?;

    let mut discovered = tokio::task::spawn_blocking(move || super::discovery::discover_all(&home))
        .await
        .map_err(|e| ToolError::internal_message(format!("discovery task panicked: {e}")))?;
    if !params.clients.is_empty() {
        let filter: std::collections::HashSet<&str> =
            params.clients.iter().map(String::as_str).collect();
        discovered.retain(|s| filter.contains(s.source_client.as_str()));
    }

    // Reaching here: exactly one of `all=true` or a non-empty `names` list is set.
    // (both-provided is rejected above; neither-provided is rejected above)
    let to_import: Vec<_> = if params.all {
        discovered
    } else {
        let wanted: std::collections::HashSet<&str> =
            params.names.iter().map(String::as_str).collect();
        discovered
            .into_iter()
            .filter(|s| wanted.contains(s.name.as_str()))
            .collect()
    };

    let cfg = manager.current_config().await;
    let (mut result, specs_to_add) =
        super::manager::partition_discovered_for_import(&cfg, to_import);

    if !specs_to_add.is_empty() {
        let outcome = manager
            .batch_add(specs_to_add, Some("gateway.import"), None)
            .await?;

        result.imported.extend(outcome.views);

        for (name, err) in outcome.errors {
            if matches!(err, ToolError::Conflict { .. }) {
                result.skipped.push(ImportSkipView {
                    name,
                    reason: ImportSkipReason::Conflict,
                });
            } else {
                result.errors.push(ImportErrorView {
                    name,
                    message: err.to_string(),
                });
            }
        }
    }

    to_json(result)
}

async fn handle_import_tombstone_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.import_tombstones.list" => to_json(manager.list_import_tombstones().await),
        "gateway.import_tombstones.clear" => {
            let params: GatewayImportTombstoneParams = parse_params(params_value)?;
            to_json(manager.clear_import_tombstone(params.into()).await?)
        }
        "gateway.import_tombstones.restore" => {
            let params: GatewayImportTombstoneParams = parse_params(params_value)?;
            let origin = params.origin.clone();
            let owner = params.owner.clone();
            to_json(
                manager
                    .restore_import_tombstone(
                        params.into(),
                        origin.as_deref(),
                        owner.map(Into::into),
                    )
                    .await?,
            )
        }
        unknown => unknown_action(unknown),
    }
}

impl From<GatewayImportTombstoneParams> for ImportTombstoneSelector {
    fn from(value: GatewayImportTombstoneParams) -> Self {
        Self {
            name: value.name,
            source_client: value.source_client,
            source_path: value.source_path,
            server_name: value.server_name,
            transport_fingerprint: value.transport_fingerprint,
        }
    }
}

fn redact_url_preview(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return "<redacted>".to_string();
    };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    parsed.to_string()
}

async fn handle_tool_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.code_mode.get" => to_json(manager.code_mode_config().await),
        "gateway.code_mode.set" => {
            let params: CodeModeSetParams = parse_params(params_value)?;
            let mut next = manager.code_mode_config().await;
            if let Some(enabled) = params.enabled {
                next.enabled = enabled;
            }
            if let Some(trace_params) = params.trace_params {
                next.trace_params = trace_params;
            }
            if let Some(result_shape_policy) = params.result_shape_policy {
                next.result_shape_policy = result_shape_policy;
            }
            if let Some(timeout_ms) = params.timeout_ms {
                next.timeout_ms = timeout_ms;
            }
            if let Some(max_response_bytes) = params.max_response_bytes {
                next.max_response_bytes = max_response_bytes;
            }
            if let Some(max_response_tokens) = params.max_response_tokens {
                next.max_response_tokens = max_response_tokens;
            }
            if let Some(token_estimate_divisor) = params.token_estimate_divisor {
                next.token_estimate_divisor = token_estimate_divisor;
            }
            if let Some(max_log_entries) = params.max_log_entries {
                next.max_log_entries = max_log_entries;
            }
            if let Some(max_log_bytes) = params.max_log_bytes {
                next.max_log_bytes = max_log_bytes;
            }
            to_json(manager.set_code_mode_config(next, None, None).await?)
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_protected_route_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.protected_route.list" => to_json(manager.protected_route_list().await),
        "gateway.protected_route.get" => {
            let params: ProtectedRouteNameParams = parse_params(params_value)?;
            to_json(manager.protected_route_get(&params.name).await?)
        }
        "gateway.protected_route.add" => {
            let params: ProtectedRouteSpecParams = parse_params(params_value)?;
            to_json(manager.protected_route_add(params.route).await?)
        }
        "gateway.protected_route.update" => {
            let params: ProtectedRouteUpdateParams = parse_params(params_value)?;
            to_json(
                manager
                    .protected_route_update(&params.name, params.route)
                    .await?,
            )
        }
        "gateway.protected_route.remove" => {
            let params: ProtectedRouteNameParams = parse_params(params_value)?;
            to_json(manager.protected_route_remove(&params.name).await?)
        }
        "gateway.protected_route.test" => {
            let params: ProtectedRouteSpecParams = parse_params(params_value)?;
            to_json(manager.protected_route_test(params.route).await?)
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_virtual_server_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.virtual_server.enable" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(manager.enable_virtual_server(&params.id).await?)
        }
        "gateway.virtual_server.disable" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(manager.disable_virtual_server(&params.id).await?)
        }
        "gateway.virtual_server.remove" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(manager.remove_virtual_server(&params.id).await?)
        }
        "gateway.virtual_server.quarantine.list" => {
            to_json(manager.list_quarantined_virtual_servers().await?)
        }
        "gateway.virtual_server.quarantine.restore" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(
                manager
                    .restore_quarantined_virtual_server(&params.id)
                    .await?,
            )
        }
        "gateway.virtual_server.set_surface" => {
            let params: VirtualServerSurfaceParams = parse_params(params_value)?;
            to_json(
                manager
                    .set_virtual_server_surface(&params.id, &params.surface, params.enabled)
                    .await?,
            )
        }
        "gateway.virtual_server.get_mcp_policy" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(manager.get_virtual_server_mcp_policy(&params.id).await?)
        }
        "gateway.virtual_server.set_mcp_policy" => {
            let params: VirtualServerMcpPolicyParams = parse_params(params_value)?;
            let service = manager.service_for_virtual_server_id(&params.id).await?;
            let valid_actions = compiled_service_actions(manager, &service)?;
            for action in &params.allowed_actions {
                if !valid_actions
                    .iter()
                    .any(|candidate| candidate.name == action.as_str())
                {
                    return Err(ToolError::InvalidParam {
                        message: format!("action `{action}` is not valid for service `{service}`"),
                        param: "allowed_actions".to_string(),
                    });
                }
            }
            to_json(
                manager
                    .set_virtual_server_mcp_policy(&params.id, &params.allowed_actions)
                    .await?,
            )
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_service_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.service_config.get" => {
            let params: ServiceConfigGetParams = parse_params(params_value)?;
            to_json(manager.get_service_config(&params.service).await?)
        }
        "gateway.service_config.set" => {
            let params: ServiceConfigSetParams = parse_params(params_value)?;
            to_json(
                manager
                    .set_service_config(&params.service, &params.values)
                    .await?,
            )
        }
        "gateway.service_actions" => {
            let params: ServiceConfigGetParams = parse_params(params_value)?;
            to_json(compiled_service_actions(manager, &params.service)?)
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_gateway_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.list" => to_json(manager.list().await?),
        "gateway.server.get" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(manager.get_server(&params.id).await?)
        }
        "gateway.supported_services" => {
            let registry = manager.builtin_service_registry();
            to_json(super::service_catalog::supported_services_from_registry(
                registry.as_ref(),
            ))
        }
        "gateway.get" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            to_json(manager.get(&params.name).await?)
        }
        "gateway.test" => {
            // SECURITY NOTE: When called with a `spec` (unsaved config) for a
            // stdio-backed upstream, this action may **execute a local command**.
            // The `command` field of the spec is passed directly to the child
            // process launcher; there is no sandbox.  Only callers with gateway
            // admin privileges should be able to reach this action, and operators
            // must treat `spec`-mode as equivalent to running the named binary.
            //
            // When called with a `name` (saved config), the command comes from the
            // persisted config file, which is under operator control.  The same
            // execution risk applies — the test action spawns the stdio process
            // and probes it exactly as the gateway would during live operation.
            let params: GatewayTestParams = parse_params(params_value)?;
            match (params.name.as_deref(), params.spec.as_ref()) {
                (Some(name), None) => to_json(manager.test(Err(name)).await?),
                (None, Some(spec)) => to_json(manager.test(Ok(spec)).await?),
                (Some(_), Some(_)) => Err(ToolError::InvalidParam {
                    message: "gateway.test accepts either `name` or `spec`, not both".to_string(),
                    param: "name".to_string(),
                }),
                (None, None) => Err(ToolError::MissingParam {
                    message: "gateway.test requires either `name` or `spec`".to_string(),
                    param: "name".to_string(),
                }),
            }
        }
        "gateway.add" => {
            let params: GatewayAddParams = parse_params(params_value)?;
            to_json(
                manager
                    .add(
                        params.spec,
                        params.bearer_token_value,
                        params.origin.as_deref(),
                        params.owner.map(Into::into),
                    )
                    .await?,
            )
        }
        "gateway.update" => {
            let params: GatewayUpdateParams = parse_params(params_value)?;
            to_json(
                manager
                    .update(
                        &params.name,
                        params.patch,
                        params.bearer_token_value,
                        params.origin.as_deref(),
                        params.owner.map(Into::into),
                    )
                    .await?,
            )
        }
        "gateway.remove" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            to_json(
                manager
                    .remove(
                        &params.name,
                        params.origin.as_deref(),
                        params.owner.map(Into::into),
                    )
                    .await?,
            )
        }
        "gateway.reload" => {
            let params: GatewayReloadParams = parse_params(params_value)?;
            to_json(
                manager
                    .reload_with_origin(params.origin.as_deref(), params.owner.map(Into::into))
                    .await?,
            )
        }
        "gateway.status" => {
            let params: GatewayStatusParams = parse_params(params_value)?;
            to_json(manager.status(params.name.as_deref()).await?)
        }
        "gateway.client_config.get" => {
            let params: GatewayClientConfigParams = parse_params(params_value)?;
            to_json(manager.client_config(&params.name).await?)
        }
        "gateway.public_urls.get" => {
            let urls = manager.public_urls();
            let effective_mcp_gateway = urls.effective_mcp_gateway().map(str::to_owned);
            to_json(serde_json::json!({
                "app": urls.app,
                "mcp_gateway": urls.mcp_gateway,
                "effective_mcp_gateway": effective_mcp_gateway,
            }))
        }
        "gateway.discovered_tools" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            to_json(manager.discovered_tools(&params.name).await?)
        }
        "gateway.discovered_resources" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            to_json(manager.discovered_resources(&params.name).await?)
        }
        "gateway.discovered_prompts" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            to_json(manager.discovered_prompts(&params.name).await?)
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_oauth_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.oauth.probe" => {
            let url = require_str(&params_value, "url")?;
            to_json(crate::gateway::oauth::probe(manager, url).await?)
        }
        "gateway.oauth.start" => {
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            let subject = params
                .subject
                .as_deref()
                .unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT);
            to_json(
                crate::gateway::oauth::begin_authorization(manager, &params.upstream, subject)
                    .await?,
            )
        }
        "gateway.oauth.status" => {
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            let subject = params
                .subject
                .as_deref()
                .unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT);
            to_json(crate::gateway::oauth::status(manager, &params.upstream, subject).await?)
        }
        "gateway.oauth.clear" => {
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            let subject = params
                .subject
                .as_deref()
                .unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT);
            crate::gateway::oauth::clear(manager, &params.upstream, subject).await?;
            to_json(serde_json::json!({ "ok": true }))
        }
        // Q-H3: poll loop moved from cli/gateway.rs into shared dispatch so all
        // surfaces (CLI, API, MCP) share the same orchestration logic.
        "gateway.oauth.wait" => {
            // Extract timeout_secs before parse_params consumes params_value.
            let timeout_secs: u64 = params_value
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(120);
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            let subject = params
                .subject
                .as_deref()
                .unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT);
            let timeout = std::time::Duration::from_secs(timeout_secs);
            let authenticated = manager
                .await_upstream_authorization(&params.upstream, subject, timeout)
                .await?;
            to_json(serde_json::json!({
                "authenticated": authenticated,
                "timed_out": !authenticated,
            }))
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_mcp_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.mcp.enable" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            to_json(
                manager
                    .update(
                        &params.name,
                        GatewayUpdatePatch {
                            enabled: Some(true),
                            ..GatewayUpdatePatch::default()
                        },
                        None,
                        params.origin.as_deref(),
                        params.owner.clone().map(Into::into),
                    )
                    .await?,
            )
        }
        "gateway.mcp.list" => to_json(manager.mcp_runtime_list().await?),
        "gateway.mcp.disable" => {
            let params: GatewayMcpToggleParams = parse_params(params_value)?;
            let gateway = manager
                .update(
                    &params.name,
                    GatewayUpdatePatch {
                        enabled: Some(false),
                        ..GatewayUpdatePatch::default()
                    },
                    None,
                    params.origin.as_deref(),
                    params.owner.clone().map(Into::into),
                )
                .await?;
            let cleanup = if params.cleanup {
                Some(
                    manager
                        .cleanup_upstream_processes(&params.name, params.aggressive, false)
                        .await?,
                )
            } else {
                None
            };
            to_json(serde_json::json!({
                "gateway": gateway,
                "cleanup": cleanup,
            }))
        }
        "gateway.mcp.cleanup" => {
            let params: GatewayMcpCleanupParams = parse_params(params_value)?;
            to_json(
                manager
                    .cleanup_upstream_processes(&params.name, params.aggressive, params.dry_run)
                    .await?,
            )
        }
        unknown => unknown_action(unknown),
    }
}

fn unknown_action(unknown: &str) -> Result<Value, ToolError> {
    Err(ToolError::UnknownAction {
        message: format!("unknown action '{unknown}'"),
        valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
        hint: None,
    })
}

fn compiled_service_actions(
    manager: &GatewayManager,
    service: &str,
) -> Result<Vec<ServiceActionView>, ToolError> {
    let registry = manager.builtin_service_registry();
    let actions = registry
        .service_actions(service)
        .ok_or_else(|| ToolError::InvalidParam {
            message: format!("unknown service `{service}`"),
            param: "service".to_string(),
        })?;

    Ok(actions
        .iter()
        .map(|action| ServiceActionView {
            name: action.name.to_string(),
            description: action.description.to_string(),
            destructive: action.destructive,
        })
        .collect())
}

/// Public entry point for gateway dispatch.
///
/// Built-in actions (`help`, `schema`) are handled **before** manager
/// resolution so they succeed even when no gateway manager is installed.
/// This matches the shared dispatch contract used by every other service.
pub async fn dispatch(action: &str, params_value: Value) -> Result<Value, ToolError> {
    // Handle catalog-discovery built-ins first — they must not fail when no
    // gateway manager is installed (e.g. during initial setup or test runs
    // that do not wire a manager).  Fixing the dispatch contract here is the
    // minimum required change (see bead lab-l3cm).
    match action {
        "help" => return Ok(help_payload("gateway", ACTIONS)),
        "schema" => {
            let action_name = require_str(&params_value, "action")?;
            return action_schema(ACTIONS, action_name);
        }
        _ => {}
    }
    let manager = require_gateway_manager()?;
    dispatch_with_manager(&manager, action, params_value).await
}

#[cfg(test)]
#[allow(clippy::panic)]
#[path = "dispatch_tests.rs"]
mod tests;
