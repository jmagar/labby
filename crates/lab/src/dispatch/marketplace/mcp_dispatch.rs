//! Dispatch for `mcp.*` actions within the marketplace service.
//!
//! These actions were absorbed from `dispatch/mcpregistry/` as part of lab-zxx5.2.
//! The `dispatch/mcpregistry/` directory is retained until lab-zxx5.4.
//!
//! All `mcp.*` routing is feature-gated on `mcpregistry`. When the feature is
//! absent, every `mcp.*` action returns a structured `not_configured` error.

#[cfg(feature = "mcpregistry")]
use std::time::Instant;

#[cfg(feature = "mcpregistry")]
use lab_apis::mcpregistry::McpRegistryClient;
#[cfg(feature = "mcpregistry")]
use lab_apis::mcpregistry::types::{EnvironmentVariable, ServerJSON};
use serde_json::Value;

use crate::dispatch::error::ToolError;
#[cfg(feature = "mcpregistry")]
use crate::dispatch::helpers::to_json;
#[cfg(feature = "mcpregistry")]
use crate::dispatch::marketplace::LAB_REGISTRY_META_NAMESPACE;
use crate::dispatch::marketplace::mcp_catalog::MCP_ACTIONS;
use crate::dispatch::marketplace::mcp_client;
#[cfg(feature = "mcpregistry")]
use crate::dispatch::marketplace::mcp_params;
#[cfg(feature = "mcpregistry")]
use crate::dispatch::node::send::send_rpc_to_node;

/// Dispatch a `mcp.*` action using a freshly constructed client.
///
/// Called from `marketplace/dispatch.rs` for any action with the `mcp.` prefix.
pub async fn dispatch_mcp(action: &str, params: Value) -> Result<Value, ToolError> {
    #[cfg(feature = "mcpregistry")]
    {
        let client = mcp_client::require_mcp_client()?;
        dispatch_mcp_with_client(&client, action, params).await
    }
    #[cfg(not(feature = "mcpregistry"))]
    {
        let _ = (action, params);
        Err(mcp_client::not_configured_error())
    }
}

/// Dispatch a `mcp.*` action with a pre-built client (used by API handlers with AppState).
#[cfg(feature = "mcpregistry")]
pub async fn dispatch_mcp_with_client(
    client: &McpRegistryClient,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    match action {
        "mcp.config" => Ok(serde_json::json!({
            "url": mcp_client::configured_registry_url()?
        })),
        "mcp.list" => dispatch_mcp_list(client, &params).await,
        "mcp.get" => {
            let name = mcp_params::require_name(&params)?;
            to_json(client.get_server(&name, "latest").await?)
        }
        "mcp.versions" => {
            let name = mcp_params::require_name(&params)?;
            to_json(client.list_versions(&name).await?)
        }
        "mcp.validate" => {
            let server_json: ServerJSON = serde_json::from_value(params["server_json"].clone())
                .map_err(|e| ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message: format!("invalid server_json: {e}"),
                })?;
            to_json(client.validate(&server_json).await?)
        }
        "mcp.install" => dispatch_mcp_install(client, &params).await,
        "mcp.uninstall" => {
            let gateway_name = params["gateway_name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ToolError::MissingParam {
                    message: "missing required parameter `gateway_name`".to_string(),
                    param: "gateway_name".to_string(),
                })?
                .to_string();

            // Delegate to gateway.remove — pass confirm:true since the caller already confirmed
            // at the mcp.uninstall level (destructive:true is checked by handle_action).
            crate::dispatch::gateway::dispatch(
                "gateway.remove",
                serde_json::json!({ "name": gateway_name, "confirm": true }),
            )
            .await
        }
        "mcp.meta.get" => dispatch_mcp_local(action, params).await,
        "mcp.meta.set" => dispatch_mcp_local(action, params).await,
        "mcp.meta.delete" => dispatch_mcp_local(action, params).await,
        "mcp.sync" => {
            use crate::config;
            let db_path = config::registry_db_path();
            let store = crate::dispatch::marketplace::store::RegistryStore::open(&db_path)
                .await
                .map_err(|e| ToolError::internal_message(format!("registry store open: {e}")))?;
            let count =
                crate::dispatch::marketplace::sync::perform_sync(&store, client, true, "manual")
                    .await?;
            Ok(serde_json::json!({ "synced": count }))
        }
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action '{unknown}'"),
            valid: MCP_ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

#[cfg(feature = "mcpregistry")]
async fn dispatch_mcp_list(client: &McpRegistryClient, params: &Value) -> Result<Value, ToolError> {
    if let Some(param) = ["sort_by", "order"]
        .into_iter()
        .find(|name| params.get(*name).is_some())
    {
        return Err(ToolError::InvalidParam {
            message: "sort_by/order are not supported on the registry surface".to_string(),
            param: param.to_string(),
        });
    }

    let parsed = mcp_params::list_servers_params(params)?;
    let store = open_registry_store().await?;
    ensure_registry_store_populated(&store, client).await?;
    let list_params = store_list_params_from_mcp_params(&parsed);

    if parsed.cursor.is_some() || params.get("limit").is_some() {
        let page = store.list_servers(list_params).await?;
        return Ok(serde_json::json!({
            "servers": page.servers,
            "metadata": {
                "count": page.servers.len(),
                "nextCursor": page.next_cursor,
            },
        }));
    }

    let servers = list_all_store_servers(&store, list_params).await?;
    Ok(serde_json::json!({
        "servers": servers,
        "metadata": {
            "count": servers.len(),
            "nextCursor": null,
        },
    }))
}

#[cfg(feature = "mcpregistry")]
async fn open_registry_store()
-> Result<crate::dispatch::marketplace::store::RegistryStore, ToolError> {
    use crate::config;
    crate::dispatch::marketplace::store::RegistryStore::open(&config::registry_db_path())
        .await
        .map_err(Into::into)
}

#[cfg(feature = "mcpregistry")]
fn store_list_params_from_mcp_params(
    params: &lab_apis::mcpregistry::types::ListServersParams,
) -> crate::dispatch::marketplace::store::StoreListParams {
    let mut store_params = crate::dispatch::marketplace::store::StoreListParams {
        cursor: params.cursor.clone(),
        limit: params.limit,
        version: params.version.clone(),
        updated_since: params.updated_since.clone(),
        include_deleted: false,
        latest_only: params.version.is_none(),
        featured: params.featured,
        reviewed: params.reviewed,
        recommended: params.recommended,
        hidden: params.hidden,
        tag: params.tag.clone(),
        search: None,
    };
    if let Some(search) = &params.search {
        store_params = store_params.with_search(search.clone());
    }
    store_params
}

#[cfg(feature = "mcpregistry")]
async fn ensure_registry_store_populated(
    store: &crate::dispatch::marketplace::store::RegistryStore,
    client: &McpRegistryClient,
) -> Result<(), ToolError> {
    let page = store
        .list_servers(crate::dispatch::marketplace::store::StoreListParams {
            limit: Some(1),
            include_deleted: true,
            ..Default::default()
        })
        .await?;
    if !page.servers.is_empty() {
        return Ok(());
    }

    match crate::dispatch::marketplace::sync::perform_sync(
        store,
        client,
        false,
        "mcp.list-empty-store",
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == "sync_in_progress" => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(feature = "mcpregistry")]
async fn list_all_store_servers(
    store: &crate::dispatch::marketplace::store::RegistryStore,
    params: crate::dispatch::marketplace::store::StoreListParams,
) -> Result<Vec<lab_apis::mcpregistry::types::ServerResponse>, ToolError> {
    let mut cursor = params.cursor.clone();
    let mut servers = Vec::new();
    let mut seen_cursors = std::collections::HashSet::new();

    loop {
        let page = store
            .list_servers(crate::dispatch::marketplace::store::StoreListParams {
                cursor: cursor.clone(),
                limit: Some(100),
                ..params.clone()
            })
            .await?;
        servers.extend(page.servers);
        match page.next_cursor {
            Some(next) if !next.is_empty() => {
                if cursor.as_deref() == Some(next.as_str()) || !seen_cursors.insert(next.clone()) {
                    return Err(ToolError::Sdk {
                        sdk_kind: "invalid_cursor".to_string(),
                        message: "registry store returned a non-advancing cursor".to_string(),
                    });
                }
                cursor = Some(next);
            }
            _ => break,
        }
    }

    Ok(servers)
}

/// Handle `mcp.install`: fetch server details and install to selected targets.
///
/// Takes `params.server_name`, optional `params.gateway_ids`, optional
/// `params.client_targets`, optional `env_values`.
#[cfg(feature = "mcpregistry")]
async fn dispatch_mcp_install(
    client: &McpRegistryClient,
    params: &Value,
) -> Result<Value, ToolError> {
    tracing::info!(
        surface = "mcp",
        service = "marketplace",
        action = "mcp.install",
        event = "install.attempt",
        server_name = install_server_name(params),
        version = install_version(params),
        target_kind = install_target_kind(params),
        gateway_target_count = gateway_target_count(params),
        client_target_count = client_target_count(params),
        "marketplace MCP install attempt started"
    );
    let started = Instant::now();
    let result = dispatch_mcp_install_inner(client, params).await;
    log_mcp_install_outcome(params, started, &result);
    result
}

#[cfg(feature = "mcpregistry")]
async fn dispatch_mcp_install_inner(
    client: &McpRegistryClient,
    params: &Value,
) -> Result<Value, ToolError> {
    let name = mcp_params::require_name(params)?;
    let version = params["version"].as_str().unwrap_or("latest");

    let gateway_ids: Vec<String> = match params.get("gateway_ids") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(_) => {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: "`gateway_ids` must be an array of strings".to_string(),
            });
        }
        None => Vec::new(),
    };

    let client_targets = parse_mcp_client_targets(params)?;

    if gateway_ids.is_empty() && client_targets.is_empty() {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "`gateway_ids` or `client_targets` must not be empty".to_string(),
        });
    }

    let server_resp = client.get_server(&name, version).await?;
    let server = &server_resp.server;

    let http_url = server.remotes.iter().find_map(|r| r.url.as_deref());

    let mut results = Vec::new();

    for gateway_id in &gateway_ids {
        let spec = if let Some(url) = http_url {
            match install_http(url, gateway_id, params).await {
                Ok(s) => s,
                Err(e) => {
                    results.push(serde_json::json!({
                        "gateway_id": gateway_id,
                        "ok": false,
                        "error": e.to_string(),
                    }));
                    continue;
                }
            }
        } else if let Some(pkg) = server.packages.first() {
            install_stdio(pkg, gateway_id, params, &name)?
        } else {
            results.push(serde_json::json!({
                "gateway_id": gateway_id,
                "ok": false,
                "error": format!("server '{name}' has no remotes and no packages — cannot install"),
            }));
            continue;
        };

        // Delegate to gateway.add — confirm:true because the caller already confirmed.
        match crate::dispatch::gateway::dispatch(
            "gateway.add",
            serde_json::json!({ "spec": spec, "confirm": true }),
        )
        .await
        {
            Ok(result) => {
                results.push(serde_json::json!({
                    "gateway_id": gateway_id,
                    "ok": true,
                    "result": result,
                }));
            }
            Err(e) => {
                results.push(serde_json::json!({
                    "gateway_id": gateway_id,
                    "ok": false,
                    "error": e.to_string(),
                }));
            }
        }
    }

    if !client_targets.is_empty() {
        let client_config = mcp_client_config(server, params, &name)?;
        for target in &client_targets {
            let target_id = format!("{}:{}", target.node_id, target.client);
            let outcome = send_rpc_to_node(
                &target.node_id,
                "mcp.install",
                serde_json::json!({
                    "name": name,
                    "client": target.client,
                    "config": client_config.clone(),
                }),
            )
            .await;

            results.push(match outcome {
                Ok(result) => serde_json::json!({
                    "gateway_id": target_id,
                    "target_id": target_id,
                    "node_id": target.node_id,
                    "client": target.client,
                    "ok": true,
                    "result": result,
                }),
                Err(error) => serde_json::json!({
                    "gateway_id": target_id,
                    "target_id": target_id,
                    "node_id": target.node_id,
                    "client": target.client,
                    "ok": false,
                    "error": error.to_string(),
                }),
            });
        }
    }

    Ok(serde_json::json!({ "results": results }))
}

#[cfg(feature = "mcpregistry")]
fn gateway_target_count(params: &Value) -> usize {
    params
        .get("gateway_ids")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

#[cfg(feature = "mcpregistry")]
fn client_target_count(params: &Value) -> usize {
    params
        .get("client_targets")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

#[cfg(feature = "mcpregistry")]
fn install_target_kind(params: &Value) -> &'static str {
    match (
        gateway_target_count(params) > 0,
        client_target_count(params) > 0,
    ) {
        (true, true) => "mixed",
        (true, false) => "gateway",
        (false, true) => "client",
        (false, false) => "none",
    }
}

#[cfg(feature = "mcpregistry")]
fn install_server_name(params: &Value) -> &str {
    params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("<missing>")
}

#[cfg(feature = "mcpregistry")]
fn install_version(params: &Value) -> &str {
    params
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("latest")
}

#[cfg(feature = "mcpregistry")]
fn result_counts(result: &Result<Value, ToolError>) -> (usize, usize) {
    let Some(results) = result
        .as_ref()
        .ok()
        .and_then(|value| value.get("results"))
        .and_then(Value::as_array)
    else {
        return (0, 0);
    };

    results
        .iter()
        .fold((0, 0), |(ok_count, error_count), value| {
            if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                (ok_count + 1, error_count)
            } else {
                (ok_count, error_count + 1)
            }
        })
}

#[cfg(feature = "mcpregistry")]
fn log_mcp_install_outcome(params: &Value, started: Instant, result: &Result<Value, ToolError>) {
    let elapsed_ms = started.elapsed().as_millis();
    let (installed_count, error_count) = result_counts(result);
    match result {
        Ok(_) => tracing::info!(
            surface = "mcp",
            service = "marketplace",
            action = "mcp.install",
            event = "install.finished",
            elapsed_ms,
            server_name = install_server_name(params),
            version = install_version(params),
            target_kind = install_target_kind(params),
            gateway_target_count = gateway_target_count(params),
            client_target_count = client_target_count(params),
            installed_count,
            error_count,
            "marketplace MCP install finished"
        ),
        Err(error) if error.is_internal() => tracing::error!(
            surface = "mcp",
            service = "marketplace",
            action = "mcp.install",
            event = "install.failed",
            elapsed_ms,
            kind = error.kind(),
            server_name = install_server_name(params),
            version = install_version(params),
            target_kind = install_target_kind(params),
            gateway_target_count = gateway_target_count(params),
            client_target_count = client_target_count(params),
            installed_count,
            error_count,
            "marketplace MCP install failed"
        ),
        Err(error) => tracing::warn!(
            surface = "mcp",
            service = "marketplace",
            action = "mcp.install",
            event = "install.failed",
            elapsed_ms,
            kind = error.kind(),
            server_name = install_server_name(params),
            version = install_version(params),
            target_kind = install_target_kind(params),
            gateway_target_count = gateway_target_count(params),
            client_target_count = client_target_count(params),
            installed_count,
            error_count,
            "marketplace MCP install failed"
        ),
    }
}

#[cfg(feature = "mcpregistry")]
#[derive(Debug, Clone)]
struct McpClientTarget {
    node_id: String,
    client: String,
}

#[cfg(feature = "mcpregistry")]
fn parse_mcp_client_targets(params: &Value) -> Result<Vec<McpClientTarget>, ToolError> {
    let Some(raw) = params.get("client_targets") else {
        return Ok(Vec::new());
    };
    let Value::Array(entries) = raw else {
        return Err(ToolError::InvalidParam {
            message: "`client_targets` must be an array".to_string(),
            param: "client_targets".to_string(),
        });
    };

    entries
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let node_id = value
                .get("node_id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| ToolError::InvalidParam {
                    message: "`client_targets[].node_id` is required".to_string(),
                    param: format!("client_targets[{index}].node_id"),
                })?
                .trim()
                .to_string();
            // Bound `node_id` to the same character set used elsewhere in the
            // node registry. Defends against log-injection via newlines or JSON
            // metacharacters in structured tracing output.
            if !node_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            {
                return Err(ToolError::InvalidParam {
                    message: "`client_targets[].node_id` must contain only ASCII alphanumerics, '_', '-', or '.'".to_string(),
                    param: format!("client_targets[{index}].node_id"),
                });
            }
            let client = value
                .get("client")
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "claude" | "codex"))
                .ok_or_else(|| ToolError::InvalidParam {
                    message: "`client_targets[].client` must be `claude` or `codex`".to_string(),
                    param: format!("client_targets[{index}].client"),
                })?
                .to_string();
            Ok(McpClientTarget { node_id, client })
        })
        .collect()
}

/// Validate `runtime_hint` and build the stdio argv for a package.
///
/// Returns `(hint, argv)` where `argv = runtime_arguments + identifier + package_arguments`.
/// Used by both `mcp_client_config` (rendering a single-process MCP client config)
/// and `install_stdio` (registering a stdio server with the gateway).
#[cfg(feature = "mcpregistry")]
fn build_stdio_command<'a>(
    pkg: &'a lab_apis::mcpregistry::types::Package,
    server_name: &str,
) -> Result<(&'a str, Vec<String>), ToolError> {
    if pkg.registry_type == "mcpb" {
        return Err(ToolError::Sdk {
            sdk_kind: "unsupported_registry_type".to_string(),
            message: "mcpb packages are not supported until fileSha256 integrity verification is implemented".to_string(),
        });
    }

    let hint = pkg.runtime_hint.as_deref().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "unsupported_runtime_hint".to_string(),
        message: format!(
            "server '{server_name}' package has no runtimeHint — cannot build stdio command"
        ),
    })?;

    mcp_params::validate_runtime_hint(hint)?;

    let mut argv: Vec<String> = pkg
        .runtime_arguments
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    argv.push(pkg.identifier.clone());
    argv.extend(
        pkg.package_arguments
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string)),
    );
    mcp_params::validate_stdio_argv(hint, &argv)?;

    Ok((hint, argv))
}

#[cfg(feature = "mcpregistry")]
fn mcp_client_config(
    server: &ServerJSON,
    params_value: &Value,
    server_name: &str,
) -> Result<Value, ToolError> {
    if let Some(url) = server.remotes.iter().find_map(|r| r.url.as_deref()) {
        let url_for_check = url.to_string();
        mcp_params::validate_registry_url(&url_for_check)?;
        return Ok(serde_json::json!({ "url": url }));
    }

    let Some(pkg) = server.packages.first() else {
        return Err(ToolError::Sdk {
            sdk_kind: "not_supported".to_string(),
            message: format!(
                "server '{server_name}' has no remotes and no packages — cannot install"
            ),
        });
    };

    let (hint, argv) = build_stdio_command(pkg, server_name)?;
    let env = resolve_mcp_env_values(pkg, params_value, server_name)?;
    let mut config = serde_json::json!({
        "command": hint,
        "args": argv,
    });
    if !env.is_empty() {
        let env_map: std::collections::BTreeMap<String, String> = env.into_iter().collect();
        config["env"] = serde_json::to_value(env_map).map_err(|error| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("serialize MCP env values: {error}"),
        })?;
    }
    Ok(config)
}

#[cfg(feature = "mcpregistry")]
fn resolve_mcp_env_values(
    pkg: &lab_apis::mcpregistry::types::Package,
    params_value: &Value,
    server_name: &str,
) -> Result<Vec<(String, String)>, ToolError> {
    let user_env = match params_value.get("env_values") {
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) => Some(map),
        Some(_) => {
            return Err(ToolError::InvalidParam {
                message: "`env_values` must be an object mapping env var names to string values"
                    .to_string(),
                param: "env_values".to_string(),
            });
        }
    };

    // Preserve `pkg.environment_variables` declaration order — package authors
    // curate this for readability of the resulting `.env` block.
    let mut resolved: Vec<(String, String)> = Vec::new();
    let mut missing_env = Vec::new();
    for ev in &pkg.environment_variables {
        mcp_params::validate_env_var_name(&ev.name)?;
        let user_val = match user_env.and_then(|values| values.get(&ev.name)) {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value.as_str()),
            Some(_) => {
                return Err(ToolError::InvalidParam {
                    message: format!("`env_values.{}` must be a string", ev.name),
                    param: format!("env_values.{}", ev.name),
                });
            }
        };

        let value = if let Some(value) = user_val {
            mcp_params::validate_env_value(&ev.name, value)?;
            Some(value.to_string())
        } else {
            ev.default.clone()
        };

        match value {
            Some(value) => resolved.push((ev.name.clone(), value)),
            None if ev.is_required => missing_env.push(missing_env_metadata(ev)),
            None => {}
        }
    }

    if !missing_env.is_empty() {
        return Err(ToolError::Sdk {
            sdk_kind: "missing_env_values".to_string(),
            message: serde_json::json!({
                "server": server_name,
                "missing": missing_env,
            })
            .to_string(),
        });
    }

    Ok(resolved)
}

/// Build a gateway spec for an HTTP-transport server.
///
/// Validates the URL against SSRF rules and probes for OAuth support.
#[cfg(feature = "mcpregistry")]
async fn install_http(
    url: &str,
    gateway_name: &str,
    params_value: &Value,
) -> Result<Value, ToolError> {
    let url = url.to_string();

    // SSRF validation (synchronous DNS) — must run in spawn_blocking.
    let url_for_check = url.clone();
    tokio::task::spawn_blocking(move || mcp_params::validate_registry_url(&url_for_check))
        .await
        .map_err(|e| {
            ToolError::internal_message(format!("SSRF validation task panicked: {e}"))
        })??;

    // Probe for OAuth support — non-fatal, install proceeds without OAuth on failure.
    let discovered_oauth: Option<Value> =
        if let Some(manager) = crate::dispatch::gateway::current_gateway_manager() {
            match manager.probe_upstream_oauth(&url).await {
                Ok(probe) if probe.oauth_discovered => manager
                    .upstream_oauth_manager(&probe.upstream)
                    .and_then(|m| serde_json::to_value(m.upstream_config().oauth.clone()).ok()),
                Ok(_) | Err(_) => None,
            }
        } else {
            None
        };

    let bearer_token_env = params_value["bearer_token_env"].as_str();

    let mut spec = serde_json::json!({
        "name": gateway_name,
        "url": url,
        "bearer_token_env": bearer_token_env,
        "command": null,
        "args": [],
        "proxy_resources": false,
        "expose_tools": null,
    });

    if let Some(oauth) = discovered_oauth {
        spec["oauth"] = oauth;
    }

    Ok(spec)
}

/// Build a gateway spec for a stdio-transport server, validate security constraints,
/// and write any user-supplied env vars into `~/.lab/.env`.
#[cfg(feature = "mcpregistry")]
fn install_stdio(
    pkg: &lab_apis::mcpregistry::types::Package,
    gateway_name: &str,
    params_value: &Value,
    server_name: &str,
) -> Result<Value, ToolError> {
    let (hint, argv) = build_stdio_command(pkg, server_name)?;
    let resolved_env = resolve_mcp_env_values(pkg, params_value, server_name)?;

    // Write resolved env vars to ~/.lab/.env if there are any.
    if !resolved_env.is_empty() {
        use crate::config;
        let env_path = config::dotenv_path().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "cannot determine ~/.lab/.env path".to_string(),
        })?;
        config::backup_env(&env_path).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to back up .env: {e}"),
        })?;
        let conflicts = config::write_env_pairs(&env_path, &resolved_env, false).map_err(|e| {
            ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to write env vars to .env: {e}"),
            }
        })?;
        if !conflicts.is_empty() {
            tracing::warn!(
                service = "marketplace",
                action = "mcp.install",
                "env write conflicts (skipped): {}",
                conflicts.join("; ")
            );
        }
    }

    Ok(serde_json::json!({
        "name": gateway_name,
        "url": null,
        "command": hint,
        "args": argv,
        "proxy_resources": false,
        "proxy_prompts": false,
        "expose_tools": null,
    }))
}

#[cfg(feature = "mcpregistry")]
fn missing_env_metadata(ev: &EnvironmentVariable) -> Value {
    serde_json::json!({
        "name": ev.name,
        "is_secret": ev.is_secret,
        "is_required": ev.is_required,
        "default": ev.default,
        "choices": ev.choices,
        "description": ev.description,
    })
}

/// Dispatch `mcp.meta.*` actions that work against the local registry store.
#[cfg(feature = "mcpregistry")]
async fn dispatch_mcp_local(action: &str, params: Value) -> Result<Value, ToolError> {
    use crate::config;
    match action {
        "mcp.meta.get" => {
            let name = mcp_params::require_name(&params)?;
            let requested_version = params["version"].as_str().unwrap_or("latest");
            let store = crate::dispatch::marketplace::store::RegistryStore::open(
                &config::registry_db_path(),
            )
            .await?;
            let server = store
                .get_server(&name, requested_version)
                .await?
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!(
                        "server '{name}' version '{requested_version}' not found in local registry store"
                    ),
                })?;
            let resolved_version = server.server.version.clone();
            let metadata = store.get_local_metadata(&name, &resolved_version).await?;
            Ok(serde_json::json!({
                "name": name,
                "version": resolved_version,
                "namespace": LAB_REGISTRY_META_NAMESPACE,
                "metadata": metadata,
            }))
        }
        "mcp.meta.set" => {
            let name = mcp_params::require_name(&params)?;
            let requested_version = params["version"].as_str().unwrap_or("latest");
            let metadata =
                params
                    .get("metadata")
                    .cloned()
                    .ok_or_else(|| ToolError::MissingParam {
                        message: "missing required parameter `metadata`".to_string(),
                        param: "metadata".to_string(),
                    })?;
            let metadata = mcp_params::parse_lab_metadata(&metadata)?;
            let updated_by = params
                .get("updated_by")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("unknown");

            let store = crate::dispatch::marketplace::store::RegistryStore::open(
                &config::registry_db_path(),
            )
            .await?;
            let server = store
                .get_server(&name, requested_version)
                .await?
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!(
                        "server '{name}' version '{requested_version}' not found in local registry store"
                    ),
                })?;
            let resolved_version = server.server.version.clone();
            let metadata_value = serde_json::to_value(metadata)
                .map_err(|e| ToolError::internal_message(format!("serialize lab metadata: {e}")))?;
            store
                .set_local_metadata(&name, &resolved_version, &metadata_value, Some(updated_by))
                .await?;
            let current = store.get_local_metadata(&name, &resolved_version).await?;
            Ok(serde_json::json!({
                "name": name,
                "version": resolved_version,
                "namespace": LAB_REGISTRY_META_NAMESPACE,
                "metadata": current,
            }))
        }
        "mcp.meta.delete" => {
            let name = mcp_params::require_name(&params)?;
            let requested_version = params["version"].as_str().unwrap_or("latest");
            let store = crate::dispatch::marketplace::store::RegistryStore::open(
                &config::registry_db_path(),
            )
            .await?;
            let server = store
                .get_server(&name, requested_version)
                .await?
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!(
                        "server '{name}' version '{requested_version}' not found in local registry store"
                    ),
                })?;
            let resolved_version = server.server.version.clone();
            let deleted = store
                .delete_local_metadata(&name, &resolved_version)
                .await?;
            Ok(serde_json::json!({
                "name": name,
                "version": resolved_version,
                "namespace": LAB_REGISTRY_META_NAMESPACE,
                "deleted": deleted,
            }))
        }
        _ => Err(ToolError::UnknownAction {
            message: format!("unknown action '{action}'"),
            valid: MCP_ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

#[cfg(all(test, feature = "mcpregistry"))]
mod tests {
    use lab_apis::mcpregistry::types::{
        EnvironmentVariable, Package, Transport as RegistryTransport,
    };
    use serde_json::json;

    use super::*;

    fn package(runtime_hint: Option<&str>) -> Package {
        Package {
            registry_type: "npm".to_string(),
            identifier: "@example/server".to_string(),
            version: None,
            transport: RegistryTransport {
                transport_type: "stdio".to_string(),
                url: None,
                headers: Vec::new(),
                variables: None,
            },
            runtime_hint: runtime_hint.map(str::to_string),
            runtime_arguments: vec![json!("-y")],
            package_arguments: Vec::new(),
            environment_variables: Vec::new(),
            file_sha256: None,
            registry_base_url: None,
        }
    }

    #[test]
    fn stdio_install_builds_gateway_spec_from_package() {
        let spec =
            install_stdio(&package(Some("npx")), "demo", &json!({}), "io.github/demo").unwrap();

        assert_eq!(spec["command"], "npx");
        assert_eq!(spec["args"], json!(["-y", "@example/server"]));
        assert_eq!(spec["name"], "demo");
        assert_eq!(spec["proxy_resources"], false);
        assert_eq!(spec["proxy_prompts"], false);
    }

    #[test]
    fn stdio_install_rejects_missing_runtime_hint() {
        let err = install_stdio(&package(None), "demo", &json!({}), "io.github/demo").unwrap_err();

        assert_eq!(err.kind(), "unsupported_runtime_hint");
    }

    #[test]
    fn stdio_install_rejects_docker_privileged_arg() {
        let mut pkg = package(Some("docker"));
        pkg.runtime_arguments = vec![json!("run"), json!("--privileged")];

        let err = install_stdio(&pkg, "demo", &json!({}), "io.github/demo").unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn stdio_install_reports_missing_required_env_metadata() {
        let mut pkg = package(Some("npx"));
        pkg.environment_variables = vec![EnvironmentVariable {
            name: "TRUST_API_KEY".to_string(),
            description: Some("API token".to_string()),
            is_required: true,
            is_secret: true,
            default: None,
            choices: vec!["one".to_string(), "two".to_string()],
            placeholder: None,
            format: Some("token".to_string()),
        }];

        let err = install_stdio(&pkg, "demo", &json!({}), "io.github/demo").unwrap_err();

        assert_eq!(err.kind(), "missing_env_values");
        assert!(err.to_string().contains("TRUST_API_KEY"));
        assert!(err.to_string().contains("API token"));
    }

    #[test]
    fn stdio_install_rejects_protected_env_name() {
        let mut pkg = package(Some("npx"));
        pkg.environment_variables = vec![EnvironmentVariable {
            name: "PATH".to_string(),
            description: None,
            is_required: false,
            is_secret: false,
            default: None,
            choices: Vec::new(),
            placeholder: None,
            format: None,
        }];

        let err = install_stdio(&pkg, "demo", &json!({}), "io.github/demo").unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn stdio_install_rejects_env_values_with_newline() {
        let mut pkg = package(Some("npx"));
        pkg.environment_variables = vec![EnvironmentVariable {
            name: "TOKEN".to_string(),
            description: None,
            is_required: true,
            is_secret: true,
            default: None,
            choices: Vec::new(),
            placeholder: None,
            format: None,
        }];

        let err = install_stdio(
            &pkg,
            "demo",
            &json!({"env_values": {"TOKEN": "abc\ndef"}}),
            "io.github/demo",
        )
        .unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn stdio_install_accepts_extra_env_values_not_in_schema() {
        let spec = install_stdio(
            &package(Some("npx")),
            "demo",
            &json!({"env_values": {"EXTRA_TOKEN": "ignored"}}),
            "io.github/demo",
        )
        .unwrap();

        assert_eq!(spec["command"], "npx");
    }

    #[test]
    fn stdio_install_rejects_mcpb_until_integrity_support_exists() {
        let mut pkg = package(Some("npx"));
        pkg.registry_type = "mcpb".to_string();

        let err = install_stdio(&pkg, "demo", &json!({}), "io.github/demo").unwrap_err();

        assert_eq!(err.kind(), "unsupported_registry_type");
    }

    #[test]
    fn mcp_install_observability_logs_target_counts_not_payloads() {
        let source = include_str!("mcp_dispatch.rs");

        for required in [
            "event = \"install.attempt\"",
            "event = \"install.finished\"",
            "event = \"install.failed\"",
            "elapsed_ms",
            "target_kind",
            "gateway_target_count",
            "client_target_count",
            "installed_count",
            "error_count",
        ] {
            assert!(source.contains(required), "missing {required}");
        }
    }
}
