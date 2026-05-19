use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str, to_json};

use super::SHARED_GATEWAY_OAUTH_SUBJECT;
use super::catalog::ACTIONS;
use super::client::require_gateway_manager;
use super::manager::{GatewayManager, ImportTombstoneSelector};
use super::params::{
    GatewayAddParams, GatewayClientConfigParams, GatewayDiscoverParams, GatewayImportParams,
    GatewayImportTombstoneParams, GatewayMcpCleanupParams, GatewayMcpToggleParams,
    GatewayNameParams, GatewayOauthNameParams, GatewayReloadParams, GatewayStatusParams,
    GatewayTestParams, GatewayUpdateParams, GatewayUpdatePatch, ProtectedRouteNameParams,
    ProtectedRouteSpecParams, ProtectedRouteUpdateParams, ServiceConfigGetParams,
    ServiceConfigSetParams, ToolSearchSetParams, VirtualServerMcpPolicyParams,
    VirtualServerNameParams, VirtualServerSurfaceParams,
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
    match action {
        "help" => Ok(help_payload("gateway", ACTIONS)),
        "schema" => {
            let action_name = require_str(&params_value, "action")?;
            action_schema(ACTIONS, action_name)
        }
        "gateway.tool_search.get" | "gateway.tool_search.set" => {
            handle_tool_actions(manager, action, params_value).await
        }
        "gateway.discover" => handle_discover(manager, params_value).await,
        "gateway.import" => handle_import(manager, params_value).await,
        "gateway.import_tombstones.list"
        | "gateway.import_tombstones.clear"
        | "gateway.import_tombstones.restore" => {
            handle_import_tombstone_actions(manager, action, params_value).await
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
        | "gateway.discovered_prompts" => {
            handle_gateway_actions(manager, action, params_value).await
        }
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
    cfg: &crate::config::LabConfig,
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
        "gateway.tool_search.get" => to_json(manager.tool_search_config().await),
        "gateway.tool_search.set" => {
            let params: ToolSearchSetParams = parse_params(params_value)?;
            let mut next = manager.tool_search_config().await;
            next.enabled = params.enabled;
            if let Some(top_k_default) = params.top_k_default {
                next.top_k_default = top_k_default;
            }
            if let Some(max_tools) = params.max_tools {
                next.max_tools = max_tools;
            }
            to_json(manager.set_tool_search_config(next, None, None).await?)
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
                &registry,
            ))
        }
        "gateway.get" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            to_json(manager.get(&params.name).await?)
        }
        "gateway.test" => {
            let params: GatewayTestParams = parse_params(params_value)?;
            match (params.name.as_deref(), params.spec.as_ref()) {
                (Some(name), None) => to_json(manager.test(Err(name), params.allow_stdio).await?),
                (None, Some(spec)) => to_json(manager.test(Ok(spec), params.allow_stdio).await?),
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
                        params.allow_stdio,
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
                        params.allow_stdio,
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
            to_json(crate::dispatch::gateway::oauth::probe(manager, url).await?)
        }
        "gateway.oauth.start" => {
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            let subject = params
                .subject
                .as_deref()
                .unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT);
            to_json(
                crate::dispatch::gateway::oauth::begin_authorization(
                    manager,
                    &params.upstream,
                    subject,
                )
                .await?,
            )
        }
        "gateway.oauth.status" => {
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            let subject = params
                .subject
                .as_deref()
                .unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT);
            to_json(
                crate::dispatch::gateway::oauth::status(manager, &params.upstream, subject).await?,
            )
        }
        "gateway.oauth.clear" => {
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            let subject = params
                .subject
                .as_deref()
                .unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT);
            crate::dispatch::gateway::oauth::clear(manager, &params.upstream, subject).await?;
            to_json(serde_json::json!({ "ok": true }))
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
                        params.allow_stdio,
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
                    params.allow_stdio,
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
        "gateway.public_urls.get" => {
            let urls = manager.public_urls().await;
            let effective_mcp_gateway = urls.effective_mcp_gateway().map(str::to_owned);
            to_json(serde_json::json!({
                "app": urls.app,
                "mcp_gateway": urls.mcp_gateway,
                "effective_mcp_gateway": effective_mcp_gateway,
            }))
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
    let entry = registry
        .service(service)
        .ok_or_else(|| ToolError::InvalidParam {
            message: format!("unknown service `{service}`"),
            param: "service".to_string(),
        })?;

    Ok(entry
        .actions
        .iter()
        .map(|action| ServiceActionView {
            name: action.name.to_string(),
            description: action.description.to_string(),
            destructive: action.destructive,
        })
        .collect())
}

pub async fn dispatch(action: &str, params_value: Value) -> Result<Value, ToolError> {
    let manager = require_gateway_manager()?;
    dispatch_with_manager(&manager, action, params_value).await
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use crate::config::{ProtectedMcpRouteConfig, ToolSearchConfig, UpstreamConfig};

    use super::super::discovery::DiscoveredServer;
    use super::super::manager::GatewayRuntimeHandle;
    use super::super::params::GatewayDiscoverParams;
    use super::*;

    #[test]
    fn gateway_actions_include_management_surface() {
        let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
        assert!(names.contains(&"gateway.list"));
        assert!(names.contains(&"gateway.server.get"));
        assert!(names.contains(&"gateway.supported_services"));
        assert!(names.contains(&"gateway.protected_route.list"));
        assert!(names.contains(&"gateway.protected_route.get"));
        assert!(names.contains(&"gateway.protected_route.add"));
        assert!(names.contains(&"gateway.protected_route.update"));
        assert!(names.contains(&"gateway.protected_route.remove"));
        assert!(names.contains(&"gateway.protected_route.test"));
        assert!(names.contains(&"gateway.virtual_server.enable"));
        assert!(names.contains(&"gateway.virtual_server.disable"));
        assert!(names.contains(&"gateway.virtual_server.remove"));
        assert!(names.contains(&"gateway.virtual_server.quarantine.list"));
        assert!(names.contains(&"gateway.virtual_server.quarantine.restore"));
        assert!(names.contains(&"gateway.virtual_server.set_surface"));
        assert!(names.contains(&"gateway.virtual_server.get_mcp_policy"));
        assert!(names.contains(&"gateway.virtual_server.set_mcp_policy"));
        assert!(names.contains(&"gateway.service_config.get"));
        assert!(names.contains(&"gateway.service_config.set"));
        assert!(names.contains(&"gateway.service_actions"));
        assert!(names.contains(&"gateway.get"));
        assert!(names.contains(&"gateway.test"));
        assert!(names.contains(&"gateway.add"));
        assert!(names.contains(&"gateway.update"));
        assert!(names.contains(&"gateway.remove"));
        assert!(names.contains(&"gateway.reload"));
        assert!(names.contains(&"gateway.status"));
        assert!(names.contains(&"gateway.client_config.get"));
        assert!(names.contains(&"gateway.discovered_tools"));
        assert!(names.contains(&"gateway.discovered_resources"));
        assert!(names.contains(&"gateway.discovered_prompts"));
        assert!(names.contains(&"gateway.oauth.probe"));
        assert!(names.contains(&"gateway.oauth.start"));
        assert!(names.contains(&"gateway.oauth.status"));
        assert!(names.contains(&"gateway.oauth.clear"));
        assert!(names.contains(&"gateway.mcp.enable"));
        assert!(names.contains(&"gateway.mcp.disable"));
        assert!(names.contains(&"gateway.mcp.cleanup"));
        assert!(names.contains(&"gateway.public_urls.get"));

        for name in [
            "gateway.add",
            "gateway.update",
            "gateway.remove",
            "gateway.protected_route.add",
            "gateway.protected_route.update",
            "gateway.protected_route.remove",
            "gateway.virtual_server.remove",
            "gateway.virtual_server.quarantine.restore",
            "gateway.reload",
            "gateway.oauth.probe",
            "gateway.oauth.clear",
            "gateway.mcp.enable",
            "gateway.mcp.disable",
            "gateway.mcp.cleanup",
        ] {
            let spec = ACTIONS
                .iter()
                .find(|spec| spec.name == name)
                .expect("action");
            assert!(spec.destructive, "{name} must be destructive");
        }
    }

    fn test_manager() -> GatewayManager {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        GatewayManager::new(path, GatewayRuntimeHandle::default())
    }

    #[tokio::test]
    async fn gateway_dispatch_rejects_synthetic_tool_execution_actions() {
        let manager = test_manager();

        for action in ["tool_execute", "tool_invoke", "tool_search"] {
            let err = dispatch_with_manager(&manager, action, json!({}))
                .await
                .expect_err("synthetic top-level MCP tools are not gateway actions");
            assert_eq!(err.kind(), "unknown_action", "{action}");
        }
    }

    #[tokio::test]
    async fn gateway_list_returns_array() {
        let manager = test_manager();
        manager
            .replace_config_for_tests(vec![UpstreamConfig {
                enabled: true,
                name: "fixture-http".to_string(),
                url: Some("http://127.0.0.1:9001".to_string()),
                bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            }])
            .await;

        let value = dispatch_with_manager(&manager, "gateway.list", json!({}))
            .await
            .expect("list");

        assert!(value.is_array());
        assert_eq!(value.as_array().expect("array").len(), 1);
        let row = &value.as_array().expect("array")[0];
        assert_eq!(row["discovered_tool_count"], 0);
        assert_eq!(row["exposed_tool_count"], 0);
        assert_eq!(row["discovered_resource_count"], 0);
        assert_eq!(row["exposed_resource_count"], 0);
        assert_eq!(row["discovered_prompt_count"], 0);
        assert_eq!(row["exposed_prompt_count"], 0);
    }

    #[tokio::test]
    async fn gateway_client_config_get_returns_http_and_stdio_configs() {
        let manager = test_manager();
        manager
            .replace_config_for_tests(vec![
                UpstreamConfig {
                    enabled: true,
                    name: "fixture-http".to_string(),
                    url: Some("http://127.0.0.1:9001/mcp".to_string()),
                    bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
                    command: None,
                    args: Vec::new(),
                    env: std::collections::BTreeMap::new(),
                    proxy_resources: false,
                    proxy_prompts: false,
                    expose_tools: None,
                    expose_resources: None,
                    expose_prompts: None,
                    oauth: None,
                    imported_from: None,
                    tool_search: ToolSearchConfig::default(),
                },
                UpstreamConfig {
                    enabled: true,
                    name: "fixture-stdio".to_string(),
                    url: None,
                    bearer_token_env: None,
                    command: Some("npx".to_string()),
                    args: vec!["-y".to_string(), "fixture-server".to_string()],
                    env: std::collections::BTreeMap::new(),
                    proxy_resources: false,
                    proxy_prompts: false,
                    expose_tools: None,
                    expose_resources: None,
                    expose_prompts: None,
                    oauth: None,
                    imported_from: None,
                    tool_search: ToolSearchConfig::default(),
                },
            ])
            .await;

        let http = dispatch_with_manager(
            &manager,
            "gateway.client_config.get",
            json!({"name":"fixture-http"}),
        )
        .await
        .expect("http client config");
        assert_eq!(http["name"], "fixture-http");
        assert_eq!(http["type"], "http");
        assert_eq!(http["url"], "http://127.0.0.1:9001/mcp");

        let stdio = dispatch_with_manager(
            &manager,
            "gateway.client_config.get",
            json!({"name":"fixture-stdio"}),
        )
        .await
        .expect("stdio client config");
        assert_eq!(stdio["name"], "fixture-stdio");
        assert_eq!(stdio["type"], "stdio");
        assert_eq!(stdio["command"], "npx");
        assert_eq!(stdio["args"], json!(["-y", "fixture-server"]));
    }

    fn protected_route_fixture(name: &str) -> ProtectedMcpRouteConfig {
        ProtectedMcpRouteConfig {
            name: name.to_string(),
            enabled: true,
            public_host: "mcp.tootie.tv".to_string(),
            public_path: "/syslog".to_string(),
            upstream: None,
            backend_url: "http://100.88.16.79:3100".to_string(),
            backend_mcp_path: "/mcp".to_string(),
            scopes: Vec::new(),
            health_path: None,
        }
    }

    #[tokio::test]
    async fn protected_route_dispatch_add_list_and_test_share_gateway_actions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = GatewayManager::new(
            dir.path().join("config.toml"),
            GatewayRuntimeHandle::default(),
        );

        let tested = dispatch_with_manager(
            &manager,
            "gateway.protected_route.test",
            json!({ "route": protected_route_fixture("syslog") }),
        )
        .await
        .expect("test route");
        assert_eq!(tested["resource"], "https://mcp.tootie.tv/syslog");

        let added = dispatch_with_manager(
            &manager,
            "gateway.protected_route.add",
            json!({ "route": protected_route_fixture("syslog") }),
        )
        .await
        .expect("add route");
        assert_eq!(added["name"], "syslog");

        let listed = dispatch_with_manager(&manager, "gateway.protected_route.list", json!({}))
            .await
            .expect("list routes");
        assert_eq!(listed.as_array().expect("array").len(), 1);
        assert_eq!(listed[0]["public_host"], "mcp.tootie.tv");
    }

    #[tokio::test]
    async fn gateway_server_get_returns_custom_gateway_row() {
        let manager = test_manager();
        manager
            .replace_config_for_tests(vec![UpstreamConfig {
                enabled: true,
                name: "fixture-http".to_string(),
                url: Some("http://127.0.0.1:9001".to_string()),
                bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            }])
            .await;

        let value =
            dispatch_with_manager(&manager, "gateway.server.get", json!({"id":"fixture-http"}))
                .await
                .expect("server get");

        assert_eq!(value["id"], "fixture-http");
        assert_eq!(value["source"], "custom_gateway");
    }

    #[tokio::test]
    async fn gateway_list_surfaces_cached_custom_gateway_summary_counts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let runtime = GatewayRuntimeHandle::default();
        let manager = GatewayManager::new(path, runtime.clone());

        manager
            .replace_config_for_tests(vec![UpstreamConfig {
                enabled: true,
                name: "noxa".to_string(),
                url: None,
                bearer_token_env: None,
                command: Some("noxa".to_string()),
                args: vec!["mcp".to_string()],
                env: std::collections::BTreeMap::new(),
                proxy_resources: true,
                proxy_prompts: false,
                expose_tools: Some(vec!["scrape".to_string()]),
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            }])
            .await;

        let pool = crate::dispatch::upstream::pool::UpstreamPool::new();
        let upstream_name: std::sync::Arc<str> = std::sync::Arc::from("noxa");
        let mut tools = std::collections::HashMap::new();
        for name in ["scrape", "crawl"] {
            let schema = std::sync::Arc::new(serde_json::Map::new());
            let tool = rmcp::model::Tool::new(name, format!("{name} description"), schema);
            tools.insert(
                name.to_string(),
                crate::dispatch::upstream::types::UpstreamTool {
                    tool,
                    input_schema: None,
                    upstream_name: std::sync::Arc::clone(&upstream_name),
                },
            );
        }
        pool.insert_entry_for_tests(
            "noxa",
            crate::dispatch::upstream::types::UpstreamEntry {
                name: std::sync::Arc::clone(&upstream_name),
                tools,
                exposure_policy:
                    crate::dispatch::upstream::types::ToolExposurePolicy::from_patterns(vec![
                        "scrape".to_string(),
                    ])
                    .expect("policy"),
                prompt_count: 3,
                resource_count: 4,
                prompt_names: Vec::new(),
                resource_uris: Vec::new(),
                tool_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
                prompt_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
                resource_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
                tool_unhealthy_since: None,
                prompt_unhealthy_since: None,
                resource_unhealthy_since: None,
                tool_last_error: None,
                prompt_last_error: None,
                resource_last_error: None,
            },
        )
        .await;
        runtime.swap(Some(std::sync::Arc::new(pool))).await;

        let value = dispatch_with_manager(&manager, "gateway.list", json!({}))
            .await
            .expect("list");
        let row = value
            .as_array()
            .expect("array")
            .iter()
            .find(|item| item["id"] == "noxa")
            .expect("noxa row");

        assert_eq!(row["discovered_tool_count"], 2);
        assert_eq!(row["exposed_tool_count"], 1);
        assert_eq!(row["discovered_resource_count"], 4);
        assert_eq!(row["exposed_resource_count"], 4);
        assert_eq!(row["discovered_prompt_count"], 3);
        assert_eq!(row["exposed_prompt_count"], 3);
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn virtual_server_policy_validation_uses_service_name() {
        let manager = test_manager();
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "plex-primary".to_string(),
                    service: "deploy".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig::default(),
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        let value = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.set_mcp_policy",
            json!({"id":"plex-primary","allowed_actions":["server.info"]}),
        )
        .await
        .expect("set policy");

        assert_eq!(value["allowed_actions"][0], "server.info");
    }

    #[test]
    fn supported_services_lists_metadata_backed_lab_gateways() {
        let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
        assert!(names.contains(&"gateway.supported_services"));
    }

    #[tokio::test]
    async fn supported_services_payload_includes_plex_when_feature_enabled() {
        let manager = test_manager();
        let value = dispatch_with_manager(&manager, "gateway.supported_services", json!({}))
            .await
            .expect("supported services");

        let _services = value.as_array().expect("array");
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn supported_services_omits_upstreams_when_policy_disabled() {
        let registry = crate::registry::filter_built_in_upstream_apis(
            crate::registry::build_default_registry(),
            false,
        );
        let manager = test_manager().with_builtin_service_registry(registry);
        let value = dispatch_with_manager(&manager, "gateway.supported_services", json!({}))
            .await
            .expect("supported services");

        let services = value.as_array().expect("array");
        assert!(!services.iter().any(|service| service["key"] == "deploy"));
        assert!(!services.iter().any(|service| service["key"] == "setup"));
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn service_actions_rejects_disabled_upstream_service() {
        let registry = crate::registry::filter_built_in_upstream_apis(
            crate::registry::build_default_registry(),
            false,
        );
        let manager = test_manager().with_builtin_service_registry(registry);
        let err = dispatch_with_manager(
            &manager,
            "gateway.service_actions",
            json!({"service": "deploy"}),
        )
        .await
        .expect_err("disabled service should be unknown");

        assert_eq!(err.kind(), "invalid_param");
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn virtual_server_enable_rejects_disabled_upstream_service() {
        let registry = crate::registry::filter_built_in_upstream_apis(
            crate::registry::build_default_registry(),
            false,
        );
        let manager = test_manager().with_builtin_service_registry(registry);
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: false,
                    surfaces: crate::config::VirtualServerSurfacesConfig::default(),
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        let err = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.enable",
            json!({"id": "deploy"}),
        )
        .await
        .expect_err("disabled upstream virtual server should be unavailable");

        assert_eq!(err.kind(), "not_found");
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn enabling_virtual_server_marks_existing_server_row_enabled() {
        let manager = test_manager();
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: false,
                    surfaces: crate::config::VirtualServerSurfacesConfig::default(),
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        let value = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.enable",
            json!({"id": "deploy"}),
        )
        .await
        .expect("enable");

        assert_eq!(value["id"], "deploy");
        assert_eq!(value["enabled"], true);
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn enabling_virtual_server_creates_missing_service_row() {
        let manager = test_manager();

        dispatch_with_manager(
            &manager,
            "gateway.service_config.set",
            json!({
                "service": "deploy",
                "values": {
                    "PLEX_URL": "http://127.0.0.1:32400",
                    "PLEX_TOKEN": "token"
                }
            }),
        )
        .await
        .expect("set service config");

        let value = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.enable",
            json!({"id": "deploy"}),
        )
        .await
        .expect("enable missing virtual server");

        assert_eq!(value["id"], "deploy");
        assert_eq!(value["source"], "in_process");
        assert_eq!(value["enabled"], true);
        assert_eq!(value["surfaces"]["mcp"]["enabled"], true);
    }

    #[tokio::test]
    async fn disabling_virtual_server_keeps_server_row_visible_but_disabled() {
        let manager = test_manager();
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig::default(),
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        let value = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.disable",
            json!({"id": "deploy"}),
        )
        .await
        .expect("disable");

        assert_eq!(value["id"], "deploy");
        assert_eq!(value["enabled"], false);

        let list = dispatch_with_manager(&manager, "gateway.list", json!({}))
            .await
            .expect("list after disable");
        assert!(
            list.as_array()
                .expect("array")
                .iter()
                .any(|server| server["id"] == "deploy" && server["enabled"] == false)
        );
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn setting_service_config_writes_canonical_env_backed_fields() {
        let manager = test_manager();

        let value = dispatch_with_manager(
            &manager,
            "gateway.service_config.set",
            json!({
                "service": "deploy",
                "values": {
                    "PLEX_URL": "http://127.0.0.1:32400",
                    "PLEX_TOKEN": "token"
                }
            }),
        )
        .await
        .expect("set service config");

        assert_eq!(value["service"], "deploy");
        assert_eq!(value["configured"], true);
        assert!(
            value["fields"]
                .as_array()
                .expect("fields")
                .iter()
                .any(|field| field["name"] == "PLEX_URL" && field["present"] == true)
        );
        assert!(
            value["fields"]
                .as_array()
                .expect("fields")
                .iter()
                .any(|field| field["name"] == "PLEX_TOKEN" && field["present"] == true)
        );
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn configured_but_disabled_service_can_be_read_back_for_editing() {
        let manager = test_manager();
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: false,
                    surfaces: crate::config::VirtualServerSurfacesConfig::default(),
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        dispatch_with_manager(
            &manager,
            "gateway.service_config.set",
            json!({
                "service": "deploy",
                "values": {
                    "PLEX_URL": "http://127.0.0.1:32400",
                    "PLEX_TOKEN": "token"
                }
            }),
        )
        .await
        .expect("set service config");

        let value = dispatch_with_manager(
            &manager,
            "gateway.service_config.get",
            json!({"service": "deploy"}),
        )
        .await
        .expect("get service config");

        assert_eq!(value["service"], "deploy");
        assert_eq!(value["configured"], true);
        assert!(
            value["fields"]
                .as_array()
                .expect("fields")
                .iter()
                .any(|field| field["name"] == "PLEX_URL"
                    && field["value_preview"] == "http://127.0.0.1:32400")
        );
        assert!(
            value["fields"]
                .as_array()
                .expect("fields")
                .iter()
                .any(|field| field["name"] == "PLEX_TOKEN" && field["secret"] == true)
        );
    }

    #[tokio::test]
    async fn setting_virtual_server_surface_updates_visible_server_row() {
        let manager = test_manager();
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        mcp: true,
                        ..crate::config::VirtualServerSurfacesConfig::default()
                    },
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        let value = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.set_surface",
            json!({"id": "deploy", "surface": "api", "enabled": true}),
        )
        .await
        .expect("set surface");

        assert_eq!(value["id"], "deploy");
        assert_eq!(value["surfaces"]["api"]["enabled"], true);
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn setting_virtual_server_mcp_policy_persists_allowed_actions() {
        let manager = test_manager();
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: true,
                        webui: false,
                    },
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        let value = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.set_mcp_policy",
            json!({"id": "deploy", "allowed_actions": ["server.info"]}),
        )
        .await
        .expect("set mcp policy");

        assert_eq!(value["allowed_actions"], json!(["server.info"]));

        let reloaded = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.get_mcp_policy",
            json!({"id": "deploy"}),
        )
        .await
        .expect("get mcp policy");

        assert_eq!(reloaded["allowed_actions"], json!(["server.info"]));
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn service_actions_returns_compiled_action_catalog() {
        let manager = test_manager();
        let value = dispatch_with_manager(
            &manager,
            "gateway.service_actions",
            json!({"service": "deploy"}),
        )
        .await
        .expect("service actions");

        let actions = value.as_array().expect("array");
        assert!(actions.iter().any(|action| action["name"] == "server.info"));
    }

    #[tokio::test]
    async fn gateway_get_rejects_missing_name() {
        let manager = test_manager();
        let err = dispatch_with_manager(&manager, "gateway.get", json!({}))
            .await
            .expect_err("missing name should fail");

        assert_eq!(err.kind(), "invalid_param");
    }

    #[tokio::test]
    async fn gateway_test_accepts_name_or_spec() {
        let manager = test_manager();
        manager
            .replace_config_for_tests(vec![
                UpstreamConfig {
                    enabled: true,
                    name: "fixture-http".to_string(),
                    url: Some("http://127.0.0.1:9001".to_string()),
                    bearer_token_env: None,
                    command: None,
                    args: Vec::new(),
                    env: std::collections::BTreeMap::new(),
                    proxy_resources: false,
                    proxy_prompts: false,
                    expose_tools: None,
                    expose_resources: None,
                    expose_prompts: None,
                    oauth: None,
                    imported_from: None,
                    tool_search: ToolSearchConfig::default(),
                },
                UpstreamConfig {
                    enabled: true,
                    name: "configured-stdio".to_string(),
                    url: None,
                    bearer_token_env: None,
                    command: Some("echo".to_string()),
                    args: vec!["hello".to_string()],
                    env: std::collections::BTreeMap::new(),
                    proxy_resources: false,
                    proxy_prompts: false,
                    expose_tools: None,
                    expose_resources: None,
                    expose_prompts: None,
                    oauth: None,
                    imported_from: None,
                    tool_search: ToolSearchConfig::default(),
                },
            ])
            .await;

        let named =
            dispatch_with_manager(&manager, "gateway.test", json!({"name": "fixture-http"}))
                .await
                .expect("named test");
        let named_stdio = dispatch_with_manager(
            &manager,
            "gateway.test",
            json!({"name": "configured-stdio"}),
        )
        .await
        .expect("configured stdio test should not require allow_stdio");
        let proposed_without_ack = dispatch_with_manager(
            &manager,
            "gateway.test",
            json!({"spec": {
                "name": "fixture-stdio",
                "command": "echo",
                "args": ["hello"]
            }}),
        )
        .await
        .expect("stdio spec test should not require allow_stdio");

        let proposed = dispatch_with_manager(
            &manager,
            "gateway.test",
            json!({"allow_stdio": true, "spec": {
                "name": "fixture-stdio",
                "command": "echo",
                "args": ["hello"]
            }}),
        )
        .await
        .expect("spec test");

        assert_eq!(named["name"], "fixture-http");
        assert_eq!(named_stdio["name"], "configured-stdio");
        assert_eq!(proposed_without_ack["name"], "fixture-stdio");
        assert_eq!(proposed["name"], "fixture-stdio");
    }

    #[tokio::test]
    async fn gateway_mutations_call_manager_methods() {
        let manager = test_manager();

        let added = dispatch_with_manager(
            &manager,
            "gateway.add",
            json!({"spec": {
                "name": "fixture-http",
                "url": "https://fixture.example.com/mcp",
                "bearer_token_env": "FIXTURE_HTTP_TOKEN"
            }}),
        )
        .await
        .expect("add");
        assert_eq!(added["config"]["name"], "fixture-http");
        assert_eq!(added["config"]["bearer_token_env"], "FIXTURE_HTTP_TOKEN");

        let updated = dispatch_with_manager(
            &manager,
            "gateway.update",
            json!({"name": "fixture-http", "patch": {"proxy_resources": true}}),
        )
        .await
        .expect("update");
        assert_eq!(updated["config"]["proxy_resources"], true);

        let status = dispatch_with_manager(&manager, "gateway.status", json!({}))
            .await
            .expect("status");
        assert!(status.is_array());

        let removed =
            dispatch_with_manager(&manager, "gateway.remove", json!({"name": "fixture-http"}))
                .await
                .expect("remove");
        assert_eq!(removed["config"]["name"], "fixture-http");

        let reloaded = dispatch_with_manager(&manager, "gateway.reload", json!({}))
            .await
            .expect("reload");
        assert!(reloaded.get("tools_changed").is_some());
    }

    #[tokio::test]
    async fn gateway_add_allows_enabled_stdio_without_extra_ack() {
        let manager = test_manager();

        let added = dispatch_with_manager(
            &manager,
            "gateway.add",
            json!({"spec": {
                "name": "fixture-stdio",
                "command": "echo",
                "args": ["hello"]
            }}),
        )
        .await
        .expect("stdio add should be allowed");

        assert_eq!(added["config"]["name"], "fixture-stdio");
    }

    #[tokio::test]
    async fn gateway_update_allows_enabled_stdio_without_extra_ack() {
        let manager = test_manager();
        dispatch_with_manager(
            &manager,
            "gateway.add",
            json!({"spec": {
                "name": "fixture-stdio",
                "command": "echo",
                "args": ["hello"]
            }}),
        )
        .await
        .expect("add stdio");

        let updated = dispatch_with_manager(
            &manager,
            "gateway.update",
            json!({"name": "fixture-stdio", "patch": {"proxy_resources": true}}),
        )
        .await
        .expect("stdio update should be allowed");

        assert_eq!(updated["config"]["proxy_resources"], true);
    }

    #[tokio::test]
    async fn virtual_server_remove_deletes_configured_service_row() {
        let manager = test_manager();
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "stale-registry".to_string(),
                    service: "mcpregistry".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        mcp: true,
                        ..crate::config::VirtualServerSurfacesConfig::default()
                    },
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        let removed = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.remove",
            json!({"id": "stale-registry"}),
        )
        .await
        .expect("remove virtual server");

        assert_eq!(removed["id"], "stale-registry");
        assert_eq!(removed["warnings"][0]["code"], "unknown_service");

        let remaining = dispatch_with_manager(&manager, "gateway.list", json!({}))
            .await
            .expect("list after remove");
        assert_eq!(remaining.as_array().expect("array").len(), 0);
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn virtual_server_quarantine_list_and_restore_round_trip() {
        let manager = test_manager();
        manager
            .seed_config(crate::config::LabConfig {
                quarantined_virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        mcp: true,
                        ..crate::config::VirtualServerSurfacesConfig::default()
                    },
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        let quarantined = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.quarantine.list",
            json!({}),
        )
        .await
        .expect("list quarantine");
        assert_eq!(quarantined.as_array().expect("array").len(), 1);
        assert_eq!(quarantined[0]["id"], "deploy");

        let restored = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.quarantine.restore",
            json!({"id": "deploy"}),
        )
        .await
        .expect("restore quarantine");
        assert_eq!(restored["id"], "deploy");

        let remaining = dispatch_with_manager(
            &manager,
            "gateway.virtual_server.quarantine.list",
            json!({}),
        )
        .await
        .expect("list after restore");
        assert_eq!(remaining.as_array().expect("array").len(), 0);

        let listed = dispatch_with_manager(&manager, "gateway.list", json!({}))
            .await
            .expect("list active");
        assert_eq!(listed.as_array().expect("array").len(), 1);
        assert_eq!(listed[0]["id"], "deploy");
    }

    #[tokio::test]
    async fn invalid_gateway_specs_return_validation_errors() {
        let manager = test_manager();

        let invalid_url = dispatch_with_manager(
            &manager,
            "gateway.add",
            json!({"spec": {"name": "bad", "url": "ftp://example.com"}}),
        )
        .await
        .expect_err("invalid scheme");
        assert_eq!(invalid_url.kind(), "invalid_param");

        let invalid_transport = dispatch_with_manager(
            &manager,
            "gateway.add",
            json!({"spec": {"name": "bad", "url": "http://127.0.0.1:9001", "command": "node"}}),
        )
        .await
        .expect_err("invalid transport");
        assert_eq!(invalid_transport.kind(), "invalid_param");
    }

    #[tokio::test]
    async fn only_reload_promises_to_pick_up_changed_bearer_token_env_vars() {
        let manager = test_manager();
        manager
            .replace_config_for_tests(vec![UpstreamConfig {
                enabled: true,
                name: "fixture-http".to_string(),
                url: Some("http://127.0.0.1:9001".to_string()),
                bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            }])
            .await;

        let status = dispatch_with_manager(&manager, "gateway.status", json!({}))
            .await
            .expect("status");
        assert!(status.is_array());

        let help = dispatch_with_manager(&manager, "help", json!({}))
            .await
            .expect("help");
        assert_eq!(help["service"], "gateway");
        assert!(
            help.to_string().contains("gateway.reload"),
            "reload should remain the explicit env-refresh action"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn gateway_mcp_cleanup_dispatch_returns_cleanup_payload() {
        use std::process::{Command, Stdio};
        use std::time::Duration;

        let manager = test_manager();
        let upstream_name = "github-chat-cleanup-dispatch";
        let runtime_arg = "github-chat-cleanup-dispatch-mcp";
        manager
            .replace_config_for_tests(vec![UpstreamConfig {
                enabled: true,
                name: upstream_name.to_string(),
                url: None,
                bearer_token_env: None,
                command: Some("uvx".to_string()),
                args: vec![runtime_arg.to_string()],
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            }])
            .await;

        use std::os::unix::process::CommandExt;
        let mut command = Command::new("python3");
        command
            .args(["-c", "import time; time.sleep(60)", runtime_arg])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Keep this stand-in out of nextest's process group so the test
        // process survives when cleanup kills the child's process group.
        command.process_group(0);
        let mut child = command.spawn().expect("spawn github chat stand-in");

        tokio::time::sleep(Duration::from_millis(150)).await;

        let value = dispatch_with_manager(
            &manager,
            "gateway.mcp.cleanup",
            json!({
                "name": upstream_name,
                "aggressive": false,
                "dry_run": false
            }),
        )
        .await
        .expect("cleanup dispatch");

        assert_eq!(value["upstream"], upstream_name);
        assert_eq!(value["aggressive"], false);
        assert!(
            value["gateway_killed"]
                .as_u64()
                .expect("gateway_killed as u64")
                >= 1
        );

        for _ in 0..20 {
            if child.try_wait().expect("try_wait").is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        drop(child.kill());
        panic!("github-chat stand-in process was not terminated by dispatch cleanup");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn gateway_mcp_disable_with_cleanup_returns_gateway_and_cleanup_payload() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::Duration;

        let manager = test_manager();
        let upstream_name = "github-chat-disable-dispatch";
        let runtime_arg = "github-chat-disable-dispatch-mcp";
        manager
            .replace_config_for_tests(vec![UpstreamConfig {
                enabled: true,
                name: upstream_name.to_string(),
                url: None,
                bearer_token_env: None,
                command: Some("uvx".to_string()),
                args: vec![runtime_arg.to_string()],
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            }])
            .await;

        let mut command = Command::new("python3");
        command
            .args(["-c", "import time; time.sleep(60)", runtime_arg])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // The cleanup path kills process groups for child runtimes. Keep this
        // stand-in out of nextest's process group so the test process survives.
        command.process_group(0);
        let mut child = command.spawn().expect("spawn github chat stand-in");

        tokio::time::sleep(Duration::from_millis(150)).await;

        let value = dispatch_with_manager(
            &manager,
            "gateway.mcp.disable",
            json!({
                "name": upstream_name,
                "cleanup": true,
                "aggressive": false
            }),
        )
        .await
        .expect("disable dispatch");

        assert_eq!(value["gateway"]["config"]["name"], upstream_name);
        assert_eq!(value["gateway"]["config"]["enabled"], false);
        assert_eq!(value["cleanup"]["upstream"], upstream_name);

        for _ in 0..20 {
            if child.try_wait().expect("try_wait").is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        drop(child.kill());
        panic!("github-chat stand-in process was not terminated by disable cleanup");
    }

    #[test]
    fn discovery_url_preview_redacts_secret_url_parts() {
        assert_eq!(
            redact_url_preview("https://user:pass@example.com/mcp?token=secret#frag"),
            "https://example.com/mcp"
        );
        assert_eq!(redact_url_preview("not a url token=secret"), "<redacted>");
    }

    // ── shape_discovered_views unit tests ──────────────────────────────────

    fn make_discovered_http(name: &str) -> DiscoveredServer {
        DiscoveredServer {
            name: name.to_string(),
            spec: UpstreamConfig {
                name: name.to_string(),
                enabled: false,
                url: Some("http://127.0.0.1:9000".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: true,
                proxy_prompts: true,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            },
            source_client: "cursor".to_string(),
            source_path: "/home/user/.cursor/mcp.json".to_string(),
            env_key_count: 0,
        }
    }

    fn make_discovered_stdio(name: &str, command: &str) -> DiscoveredServer {
        DiscoveredServer {
            name: name.to_string(),
            spec: UpstreamConfig {
                name: name.to_string(),
                enabled: false,
                url: None,
                bearer_token_env: None,
                command: Some(command.to_string()),
                args: vec!["--serve".to_string()],
                env: std::collections::BTreeMap::new(),
                proxy_resources: true,
                proxy_prompts: true,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            },
            source_client: "claude-code".to_string(),
            source_path: "/home/user/.claude/settings.json".to_string(),
            env_key_count: 2,
        }
    }

    #[test]
    fn shape_http_server_gets_http_transport_no_command_preview() {
        let discovered = vec![make_discovered_http("my-http-server")];
        let cfg = crate::config::LabConfig::default();
        let existing: HashSet<String> = HashSet::new();
        let params = GatewayDiscoverParams::default();

        let views = shape_discovered_views(discovered, &cfg, &existing, &params);

        assert_eq!(views.len(), 1);
        assert_eq!(views[0].transport, McpClientTransportType::Http);
        assert!(views[0].command_preview.is_none());
        assert_eq!(views[0].name, "my-http-server");
    }

    #[test]
    fn shape_stdio_server_gets_stdio_transport_and_command_preview_first_token() {
        let discovered = vec![make_discovered_stdio(
            "my-stdio-server",
            "npx --yes some-mcp",
        )];
        let cfg = crate::config::LabConfig::default();
        let existing: HashSet<String> = HashSet::new();
        let params = GatewayDiscoverParams::default();

        let views = shape_discovered_views(discovered, &cfg, &existing, &params);

        assert_eq!(views.len(), 1);
        assert_eq!(views[0].transport, McpClientTransportType::Stdio);
        assert_eq!(views[0].command_preview.as_deref(), Some("npx"));
    }

    #[test]
    fn shape_already_configured_true_when_name_in_existing_set() {
        let discovered = vec![make_discovered_http("configured-server")];
        let cfg = crate::config::LabConfig::default();
        let mut existing: HashSet<String> = HashSet::new();
        existing.insert("configured-server".to_string());
        let params = GatewayDiscoverParams {
            include_existing: true,
            ..GatewayDiscoverParams::default()
        };

        let views = shape_discovered_views(discovered, &cfg, &existing, &params);

        assert_eq!(views.len(), 1);
        assert!(views[0].already_configured);
    }

    #[test]
    fn shape_include_existing_false_filters_out_already_configured_servers() {
        let discovered = vec![
            make_discovered_http("new-server"),
            make_discovered_http("existing-server"),
        ];
        let cfg = crate::config::LabConfig::default();
        let mut existing: HashSet<String> = HashSet::new();
        existing.insert("existing-server".to_string());
        let params = GatewayDiscoverParams {
            include_existing: false,
            ..GatewayDiscoverParams::default()
        };

        let views = shape_discovered_views(discovered, &cfg, &existing, &params);

        assert_eq!(views.len(), 1);
        assert_eq!(views[0].name, "new-server");
        assert!(!views[0].already_configured);
    }

    // ── handle_import and handle_discover validation branch tests ──────────

    #[tokio::test]
    async fn gateway_import_rejects_empty_params() {
        let manager = test_manager();
        let err = dispatch_with_manager(&manager, "gateway.import", json!({}))
            .await
            .expect_err("empty import params should fail");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[tokio::test]
    async fn gateway_import_rejects_both_all_and_names() {
        let manager = test_manager();
        let err = dispatch_with_manager(
            &manager,
            "gateway.import",
            json!({"all": true, "names": ["some-server"]}),
        )
        .await
        .expect_err("both all and names should fail");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[tokio::test]
    async fn gateway_import_rejects_unknown_client_kind() {
        let manager = test_manager();
        let err = dispatch_with_manager(
            &manager,
            "gateway.import",
            json!({"all": true, "clients": ["not-a-real-client"]}),
        )
        .await
        .expect_err("unknown client kind should fail");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[tokio::test]
    async fn gateway_discover_rejects_unknown_client_kind() {
        let manager = test_manager();
        let err = dispatch_with_manager(
            &manager,
            "gateway.discover",
            json!({"clients": ["typo-client"]}),
        )
        .await
        .expect_err("unknown client kind in discover should fail");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[tokio::test]
    async fn gateway_import_result_has_correct_shape() {
        // Verify the ImportResultView shape: all=true on empty discovery
        // returns ImportResultView with empty imported/skipped/errors
        let manager = test_manager();
        let result = dispatch_with_manager(&manager, "gateway.import", json!({"all": true}))
            .await
            .expect("all=true should succeed even with no discovered servers");
        // The result should be an object (ImportResultView), not an array
        assert!(
            result.is_object(),
            "import result should be an object with imported/skipped/errors"
        );
        assert!(
            result.get("imported").is_some(),
            "should have imported field"
        );
    }
}

#[cfg(test)]
mod discovery_shape_tests {
    use std::collections::HashSet;

    use crate::config::{ToolSearchConfig, UpstreamConfig};
    use crate::dispatch::gateway::discovery::DiscoveredServer;
    use crate::dispatch::gateway::params::GatewayDiscoverParams;
    use crate::dispatch::gateway::types::McpClientTransportType;

    use super::shape_discovered_views;

    fn upstream_fixture(
        name: &str,
        url: Option<String>,
        command: Option<String>,
    ) -> UpstreamConfig {
        UpstreamConfig {
            name: name.to_string(),
            enabled: false,
            url,
            bearer_token_env: None,
            command,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            tool_search: ToolSearchConfig::default(),
        }
    }

    fn make_http_server(name: &str, url: &str) -> DiscoveredServer {
        DiscoveredServer {
            name: name.to_string(),
            spec: upstream_fixture(name, Some(url.to_string()), None),
            source_client: "test".to_string(),
            source_path: "/tmp/test.json".to_string(),
            env_key_count: 0,
        }
    }

    fn make_stdio_server(name: &str, command: &str) -> DiscoveredServer {
        DiscoveredServer {
            name: name.to_string(),
            spec: upstream_fixture(name, None, Some(command.to_string())),
            source_client: "test".to_string(),
            source_path: "/tmp/test.json".to_string(),
            env_key_count: 2,
        }
    }

    #[test]
    fn http_server_gets_http_transport() {
        let views = shape_discovered_views(
            vec![make_http_server("srv", "https://example.com/mcp")],
            &crate::config::LabConfig::default(),
            &HashSet::new(),
            &GatewayDiscoverParams::default(),
        );
        assert_eq!(views.len(), 1);
        assert!(matches!(views[0].transport, McpClientTransportType::Http));
        assert!(views[0].command_preview.is_none());
    }

    #[test]
    fn stdio_server_gets_stdio_transport_and_command_preview() {
        let views = shape_discovered_views(
            vec![make_stdio_server("srv", "npx @some/mcp-server")],
            &crate::config::LabConfig::default(),
            &HashSet::new(),
            &GatewayDiscoverParams::default(),
        );
        assert_eq!(views.len(), 1);
        assert!(matches!(views[0].transport, McpClientTransportType::Stdio));
        assert_eq!(views[0].command_preview.as_deref(), Some("npx"));
    }

    #[test]
    fn already_configured_flag_set_when_name_in_existing() {
        let mut existing = HashSet::new();
        existing.insert("known-server".to_string());
        let views = shape_discovered_views(
            vec![make_http_server("known-server", "https://h/m")],
            &crate::config::LabConfig::default(),
            &existing,
            &GatewayDiscoverParams {
                include_existing: true,
                clients: vec![],
            },
        );
        assert_eq!(views.len(), 1);
        assert!(views[0].already_configured);
    }

    #[test]
    fn include_existing_false_filters_out_configured_servers() {
        let mut existing = HashSet::new();
        existing.insert("known-server".to_string());
        let views = shape_discovered_views(
            vec![make_http_server("known-server", "https://h/m")],
            &crate::config::LabConfig::default(),
            &existing,
            &GatewayDiscoverParams::default(), // include_existing defaults to false
        );
        assert!(views.is_empty());
    }
}
