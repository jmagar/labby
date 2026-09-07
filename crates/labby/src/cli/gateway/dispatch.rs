use std::process::ExitCode;

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::cli::gateway::{
    GatewayArgs, GatewayClientsCommand, GatewayCommand, GatewayEnrichCommand,
    GatewayLoadoutCommand, GatewayLoadoutCreateArgs, GatewayLoadoutUpdateArgs,
    GatewayMcpAuthCommand, GatewayMcpCommand, GatewayPendingCommand, GatewayProtectedRouteCommand,
    GatewayProtectedRouteUpdateArgs, GatewayProtectedRouteUpsertArgs, GatewayQuarantineCommand,
    GatewaySkillsCommand, GatewayUpdateArgs, GatewayUsageCommand, LazyGatewayManager,
};
use crate::cli::helpers::{run_action_command, run_confirmable_action_command};
use crate::config::{GatewayLoadoutConfig, LabConfig, ProtectedMcpRouteConfig};
use crate::dispatch::error::ToolError;
use crate::output::OutputFormat;

use super::code::run_gateway_code;
use super::list::run_gateway_list;
use super::oauth::run_gateway_oauth_start;
use crate::live_gateway as remote;

/// Dispatch `action`/`params` against the authoritative `labby serve` daemon.
///
/// Gateway actions may be scoped by the daemon's authenticated caller and
/// selected Team/Project. A one-shot local manager has neither, so falling back
/// to one would turn a failed authority lookup into installation-wide access.
pub(super) async fn dispatch_gateway_action(
    _manager: &LazyGatewayManager<'_>,
    config: &LabConfig,
    action: String,
    params: Value,
) -> Result<Value, ToolError> {
    if let Some(live) = remote::detect(config, "cli").await? {
        return live.dispatch_action(&action, params).await;
    }
    Err(gateway_daemon_unavailable())
}

fn gateway_daemon_unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "daemon_unavailable".to_owned(),
        message: "Gateway CLI actions require an authoritative Labby daemon".to_owned(),
    }
}

fn protected_route_target_from_args(
    gateway_subset: bool,
    project_id: Option<String>,
    loadout: Option<String>,
    upstreams: Vec<String>,
    services: Vec<String>,
    expose_code_mode: bool,
) -> Option<crate::config::ProtectedMcpRouteTarget> {
    (gateway_subset || project_id.is_some() || loadout.is_some()).then_some(
        crate::config::ProtectedMcpRouteTarget::GatewaySubset(
            crate::config::ProtectedGatewaySubsetTarget {
                project_id,
                loadout,
                upstreams,
                services,
                expose_code_mode,
            },
        ),
    )
}

fn protected_route_from_args(args: GatewayProtectedRouteUpsertArgs) -> ProtectedMcpRouteConfig {
    let target = protected_route_target_from_args(
        args.gateway_subset,
        args.project_id,
        args.loadout,
        args.target_upstream,
        args.target_service,
        args.expose_code_mode,
    );
    ProtectedMcpRouteConfig {
        name: args.name,
        enabled: args.enabled,
        public_host: args.public_host,
        public_path: args.public_path,
        upstream: if target.is_some() {
            None
        } else {
            args.upstream
        },
        backend_url: if target.is_some() {
            String::new()
        } else {
            args.backend_url.unwrap_or_default()
        },
        backend_mcp_path: args.backend_mcp_path.unwrap_or_else(|| "/mcp".to_string()),
        scopes: args.scopes,
        health_path: args.health_path,
        target,
    }
}

fn protected_route_from_update_args(args: GatewayProtectedRouteUpdateArgs) -> ProtectedRouteUpdate {
    let preserve_project_id = args.project_id.is_none() && !args.clear_project_id;
    let gateway_subset = args.gateway_subset || args.clear_project_id;
    let target = protected_route_target_from_args(
        gateway_subset,
        args.project_id,
        args.loadout,
        args.target_upstream,
        args.target_service,
        args.expose_code_mode,
    );
    let name = args.name;
    let route = ProtectedMcpRouteConfig {
        name: args.new_name.unwrap_or_else(|| name.clone()),
        enabled: args.enabled.unwrap_or(true),
        public_host: args.public_host,
        public_path: args.public_path,
        upstream: if target.is_some() {
            None
        } else {
            args.upstream
        },
        backend_url: if target.is_some() {
            String::new()
        } else {
            args.backend_url.unwrap_or_default()
        },
        backend_mcp_path: args.backend_mcp_path.unwrap_or_else(|| "/mcp".to_string()),
        scopes: args.scopes,
        health_path: args.health_path,
        target,
    };
    ProtectedRouteUpdate {
        name,
        route,
        preserve_project_id,
    }
}

struct ProtectedRouteUpdate {
    name: String,
    route: ProtectedMcpRouteConfig,
    preserve_project_id: bool,
}

fn loadout_from_create_args(args: GatewayLoadoutCreateArgs) -> GatewayLoadoutConfig {
    GatewayLoadoutConfig {
        name: args.name,
        description: args.description,
        upstreams: args.upstreams,
        services: args.services,
        credential_bindings: Vec::new(),
        expose_code_mode: args.code_mode,
        expose_tools: !args.no_tools,
        expose_resources: !args.no_resources,
        expose_prompts: !args.no_prompts,
        expose_skills: !args.no_skills,
    }
}

fn loadout_patch_from_args(args: GatewayLoadoutUpdateArgs) -> Value {
    let mut patch = Map::new();
    insert_if_some(&mut patch, "name", args.new_name);
    if args.clear_description {
        patch.insert("description".to_string(), Value::Null);
    } else if let Some(description) = args.description {
        patch.insert("description".to_string(), Value::String(description));
    }
    if args.clear_upstreams {
        patch.insert("upstreams".to_string(), json!([]));
    } else if !args.upstreams.is_empty() {
        patch.insert("upstreams".to_string(), json!(args.upstreams));
    }
    if args.clear_services {
        patch.insert("services".to_string(), json!([]));
    } else if !args.services.is_empty() {
        patch.insert("services".to_string(), json!(args.services));
    }
    insert_if_some(&mut patch, "expose_tools", args.expose_tools);
    insert_if_some(&mut patch, "expose_resources", args.expose_resources);
    insert_if_some(&mut patch, "expose_prompts", args.expose_prompts);
    insert_if_some(&mut patch, "expose_skills", args.expose_skills);
    insert_if_some(&mut patch, "expose_code_mode", args.expose_code_mode);
    Value::Object(patch)
}

fn update_patch_from_args(args: GatewayUpdateArgs) -> Value {
    let url_was_set = args.url.is_some();
    let command_was_set = args.command.is_some();
    let mut patch = Map::new();

    insert_if_some(&mut patch, "name", args.new_name);
    insert_if_some(&mut patch, "proxy_resources", args.proxy_resources);
    insert_if_some(&mut patch, "proxy_skills", args.proxy_skills);
    if args.clear_expose_skills {
        patch.insert("expose_skills".to_string(), Value::Null);
    } else if !args.expose_skills.is_empty() {
        patch.insert(
            "expose_skills".to_string(),
            serde_json::to_value(args.expose_skills).unwrap_or(Value::Null),
        );
    }

    if args.clear_url || command_was_set {
        patch.insert("url".to_string(), Value::Null);
    } else {
        insert_if_some(&mut patch, "url", args.url);
    }

    if args.clear_command || url_was_set {
        patch.insert("command".to_string(), Value::Null);
    } else {
        insert_if_some(&mut patch, "command", args.command);
    }

    if url_was_set {
        patch.insert("args".to_string(), json!([]));
    } else if !args.args.is_empty() {
        patch.insert("args".to_string(), json!(args.args));
    }

    if args.clear_bearer_token_env {
        patch.insert("bearer_token_env".to_string(), Value::Null);
    } else {
        insert_if_some(&mut patch, "bearer_token_env", args.bearer_token_env);
    }

    Value::Object(patch)
}

fn insert_if_some<T: serde::Serialize>(
    patch: &mut Map<String, Value>,
    key: &str,
    value: Option<T>,
) {
    if let Some(value) = value {
        patch.insert(key.to_string(), json!(value));
    }
}

pub(super) async fn dispatch_command(
    manager: &LazyGatewayManager<'_>,
    config: &LabConfig,
    args: Box<GatewayArgs>,
    format: OutputFormat,
) -> Result<ExitCode> {
    let cli_origin = format!("cli:{}", std::process::id());
    let cli_owner = json!({
        "surface": "cli",
        "client_name": "lab-cli",
        "raw": cli_origin,
    });
    match args.command {
        GatewayCommand::Mcp(args) => match args.command {
            GatewayMcpCommand::Auth(args) => match args.command {
                GatewayMcpAuthCommand::Start(args) => {
                    return run_gateway_oauth_start(manager, config, args, format).await;
                }
                GatewayMcpAuthCommand::Open(mut args) => {
                    args.open = true;
                    return run_gateway_oauth_start(manager, config, args, format).await;
                }
                GatewayMcpAuthCommand::Status(args) => {
                    return run_action_command(
                        "gateway",
                        "gateway.oauth.status".to_string(),
                        json!({ "upstream": args.name }),
                        format,
                        |action, params| async move {
                            dispatch_gateway_action(manager, config, action, params).await
                        },
                    )
                    .await;
                }
                GatewayMcpAuthCommand::Clear(args) => {
                    return run_action_command(
                        "gateway",
                        "gateway.oauth.clear".to_string(),
                        json!({ "upstream": args.name }),
                        format,
                        |action, params| async move {
                            dispatch_gateway_action(manager, config, action, params).await
                        },
                    )
                    .await;
                }
                GatewayMcpAuthCommand::RevokeGoogle(args) => {
                    return run_action_command(
                        "gateway",
                        "gateway.oauth.google_revoke".to_string(),
                        json!({ "upstream": args.name, "confirm": args.confirm }),
                        format,
                        |action, params| async move {
                            dispatch_gateway_action(manager, config, action, params).await
                        },
                    )
                    .await;
                }
            },
            GatewayMcpCommand::List => {
                return run_action_command(
                    "gateway",
                    "gateway.mcp.list".to_string(),
                    json!({}),
                    format,
                    |action, params| async move {
                        dispatch_gateway_action(manager, config, action, params).await
                    },
                )
                .await;
            }
            GatewayMcpCommand::Enable(args) => {
                return run_action_command(
                    "gateway",
                    "gateway.mcp.enable".to_string(),
                    json!({
                        "name": args.name,
                        "origin": cli_origin,
                        "owner": cli_owner,
                    }),
                    format,
                    |action, params| async move {
                        dispatch_gateway_action(manager, config, action, params).await
                    },
                )
                .await;
            }
            GatewayMcpCommand::Disable(args) => {
                return run_action_command(
                    "gateway",
                    "gateway.mcp.disable".to_string(),
                    json!({
                        "name": args.name,
                        "cleanup": args.cleanup,
                        "aggressive": args.aggressive,
                        "origin": cli_origin,
                        "owner": cli_owner,
                    }),
                    format,
                    |action, params| async move {
                        dispatch_gateway_action(manager, config, action, params).await
                    },
                )
                .await;
            }
            GatewayMcpCommand::Restart(args) => {
                return run_action_command(
                    "gateway",
                    "gateway.mcp.restart".to_string(),
                    json!({
                        "name": args.name,
                        "aggressive": args.aggressive,
                        "origin": cli_origin,
                        "owner": cli_owner,
                    }),
                    format,
                    |action, params| async move {
                        dispatch_gateway_action(manager, config, action, params).await
                    },
                )
                .await;
            }
            GatewayMcpCommand::Cleanup(args) => {
                return run_action_command(
                    "gateway",
                    "gateway.mcp.cleanup".to_string(),
                    json!({
                        "name": args.name,
                        "aggressive": args.aggressive,
                        "dry_run": args.dry_run,
                    }),
                    format,
                    |action, params| async move {
                        dispatch_gateway_action(manager, config, action, params).await
                    },
                )
                .await;
            }
        },
        GatewayCommand::List => {
            return run_gateway_list(manager, config, format).await;
        }
        command => {
            if let GatewayCommand::Code(args) = command {
                return run_gateway_code(manager, config, args, format).await;
            }
            let mut confirmed = true;
            let mut dry_run = false;
            let (action, params) = match command {
                GatewayCommand::List => unreachable!("handled above"),
                GatewayCommand::Get(args) => {
                    ("gateway.get".to_string(), json!({ "name": args.name }))
                }
                GatewayCommand::Test(args) => {
                    ("gateway.test".to_string(), json!({ "name": args.name }))
                }
                GatewayCommand::Add(args) => (
                    "gateway.add".to_string(),
                    json!({
                        "origin": cli_origin,
                        "owner": cli_owner,
                        "spec": {
                            "name": args.name,
                            "url": args.url,
                            "command": args.command,
                            "args": args.args,
                            "bearer_token_env": args.bearer_token_env,
                            "proxy_resources": args.proxy_resources,
                            "proxy_skills": args.proxy_skills,
                            "expose_skills": if args.expose_skills.is_empty() { None } else { Some(args.expose_skills) },
                        }
                    }),
                ),
                GatewayCommand::Update(args) => {
                    let name = args.name.clone();
                    (
                        "gateway.update".to_string(),
                        json!({
                            "name": name,
                            "origin": cli_origin,
                            "owner": cli_owner,
                            "patch": update_patch_from_args(args)
                        }),
                    )
                }
                GatewayCommand::Remove(args) => (
                    "gateway.remove".to_string(),
                    json!({ "name": args.name, "origin": cli_origin, "owner": cli_owner }),
                ),
                GatewayCommand::Quarantine(args) => match args.command {
                    GatewayQuarantineCommand::List => (
                        "gateway.virtual_server.quarantine.list".to_string(),
                        json!({}),
                    ),
                    GatewayQuarantineCommand::Restore(args) => (
                        "gateway.virtual_server.quarantine.restore".to_string(),
                        json!({ "id": args.id }),
                    ),
                },
                GatewayCommand::ProtectedRoute(args) => match args.command {
                    GatewayProtectedRouteCommand::List => {
                        ("gateway.protected_route.list_state".to_string(), json!({}))
                    }
                    GatewayProtectedRouteCommand::Get(args) => (
                        "gateway.protected_route.get".to_string(),
                        json!({ "name": args.name }),
                    ),
                    GatewayProtectedRouteCommand::Add(args) => {
                        let stage_for_restart = args.stage_for_restart;
                        let route = protected_route_from_args(args);
                        let action = if stage_for_restart || route.is_gateway_subset() {
                            "gateway.protected_route.stage_add"
                        } else {
                            "gateway.protected_route.add"
                        };
                        (action.to_string(), json!({ "route": route }))
                    }
                    GatewayProtectedRouteCommand::Update(args) => {
                        let stage_for_restart = args.stage_for_restart;
                        let update = protected_route_from_update_args(args);
                        let action = if stage_for_restart || update.route.is_gateway_subset() {
                            "gateway.protected_route.stage_update"
                        } else {
                            "gateway.protected_route.update"
                        };
                        (
                            action.to_string(),
                            json!({
                                "name": update.name,
                                "route": update.route,
                                "preserve_project_id": update.preserve_project_id,
                            }),
                        )
                    }
                    GatewayProtectedRouteCommand::Remove(args) => (
                        if args.stage_for_restart {
                            "gateway.protected_route.stage_remove"
                        } else {
                            "gateway.protected_route.remove"
                        }
                        .to_string(),
                        json!({ "name": args.name }),
                    ),
                    GatewayProtectedRouteCommand::Test(args) => (
                        "gateway.protected_route.test".to_string(),
                        json!({ "route": protected_route_from_args(args) }),
                    ),
                },
                GatewayCommand::Loadout(args) => match args.command {
                    GatewayLoadoutCommand::List => {
                        ("gateway.loadout.list_state".to_string(), json!({}))
                    }
                    GatewayLoadoutCommand::Get(args) => (
                        "gateway.loadout.get".to_string(),
                        json!({ "name": args.name }),
                    ),
                    GatewayLoadoutCommand::Add(args) => (
                        "gateway.loadout.add".to_string(),
                        json!({ "loadout": loadout_from_create_args(args) }),
                    ),
                    GatewayLoadoutCommand::Update(args) => {
                        let name = args.name.clone();
                        let stage_for_restart = args.stage_for_restart;
                        (
                            if stage_for_restart {
                                "gateway.loadout.stage_patch"
                            } else {
                                "gateway.loadout.patch"
                            }
                            .to_string(),
                            json!({ "name": name, "patch": loadout_patch_from_args(args) }),
                        )
                    }
                    GatewayLoadoutCommand::Remove(args) => (
                        if args.stage_for_restart {
                            "gateway.loadout.stage_remove"
                        } else {
                            "gateway.loadout.remove"
                        }
                        .to_string(),
                        json!({ "name": args.name }),
                    ),
                },
                GatewayCommand::Reload => (
                    "gateway.reload".to_string(),
                    json!({ "origin": cli_origin, "owner": cli_owner }),
                ),
                GatewayCommand::Discover(args) => (
                    "gateway.discover".to_string(),
                    json!({
                        "clients": args.clients,
                        "include_existing": args.include_existing,
                    }),
                ),
                GatewayCommand::Import(args) => {
                    confirmed = args.yes;
                    (
                        "gateway.import".to_string(),
                        json!({
                            "all": args.all,
                            "names": args.names,
                            "clients": args.clients,
                        }),
                    )
                }
                GatewayCommand::Pending(args) => match args.command {
                    GatewayPendingCommand::List => {
                        ("gateway.import_pending.list".to_string(), json!({}))
                    }
                    GatewayPendingCommand::Approve(name_args) => {
                        confirmed = name_args.yes;
                        dry_run = name_args.dry_run;
                        (
                            "gateway.import_pending.approve".to_string(),
                            json!({ "name": name_args.name }),
                        )
                    }
                    GatewayPendingCommand::Reject(name_args) => {
                        confirmed = name_args.yes;
                        dry_run = name_args.dry_run;
                        (
                            "gateway.import_pending.reject".to_string(),
                            json!({ "name": name_args.name }),
                        )
                    }
                },
                GatewayCommand::PublicUrls => ("gateway.public_urls.get".to_string(), json!({})),
                GatewayCommand::Clients(args) => match args.command {
                    GatewayClientsCommand::List => ("gateway.clients.list".to_string(), json!({})),
                },
                GatewayCommand::Enrich(args) => match args.command {
                    None => {
                        confirmed = args.yes;
                        (
                            "gateway.enrich.preview".to_string(),
                            json!({
                                "upstreams": args.upstreams,
                                "all": args.all,
                                "provider": args.provider,
                                "max_upstreams": args.max_upstreams,
                                "timeout_ms": args.timeout_ms,
                            }),
                        )
                    }
                    Some(GatewayEnrichCommand::Apply(args)) => {
                        confirmed = args.yes;
                        (
                            "gateway.enrich.apply".to_string(),
                            json!({
                                "upstream": args.upstream,
                                "hint": args.hint,
                                "metadata_hash": args.metadata_hash,
                            }),
                        )
                    }
                },
                GatewayCommand::Skills(args) => match args.command {
                    GatewaySkillsCommand::List(args) => (
                        "gateway.skills.list".to_string(),
                        json!({ "upstream": args.upstream }),
                    ),
                    GatewaySkillsCommand::Trust(args) => {
                        confirmed = args.yes;
                        (
                            "gateway.update".to_string(),
                            json!({
                                "name": args.upstream,
                                "origin": cli_origin,
                                "owner": cli_owner,
                                "patch": { "proxy_skills": true },
                            }),
                        )
                    }
                    GatewaySkillsCommand::Untrust(args) => (
                        "gateway.update".to_string(),
                        json!({
                            "name": args.upstream,
                            "origin": cli_origin,
                            "owner": cli_owner,
                            "patch": { "proxy_skills": false },
                        }),
                    ),
                    GatewaySkillsCommand::Expose(args) => (
                        "gateway.update".to_string(),
                        json!({
                            "name": args.upstream,
                            "origin": cli_origin,
                            "owner": cli_owner,
                            "patch": { "expose_skills": args.patterns },
                        }),
                    ),
                    GatewaySkillsCommand::ExposeAll(args) => (
                        "gateway.update".to_string(),
                        json!({
                            "name": args.upstream,
                            "origin": cli_origin,
                            "owner": cli_owner,
                            "patch": { "expose_skills": Value::Null },
                        }),
                    ),
                },
                GatewayCommand::Usage(args) => match args.command {
                    GatewayUsageCommand::Metrics(m) => (
                        "gateway.usage.metrics".to_string(),
                        json!({
                            "since_unix": m.since_unix,
                            "until_unix": m.until_unix,
                            "upstream": m.upstream,
                            "tool": m.tool,
                            "capability": m.capability,
                            "operation": m.operation,
                            "subject_scoped": m.subject_scoped,
                            "actor": m.actor,
                            "outcome": m.outcome,
                            "search": m.search,
                            "bucket_count": m.bucket_count,
                            "timezone": m.timezone,
                            "timezone_offset_minutes": m.timezone_offset_minutes,
                            "include_facets": m.include_facets,
                        }),
                    ),
                    GatewayUsageCommand::Calls(c) => (
                        "gateway.usage.calls".to_string(),
                        json!({
                            "since_unix": c.since_unix,
                            "until_unix": c.until_unix,
                            "upstream": c.upstream,
                            "tool": c.tool,
                            "capability": c.capability,
                            "operation": c.operation,
                            "subject_scoped": c.subject_scoped,
                            "actor": c.actor,
                            "outcome": c.outcome,
                            "search": c.search,
                            "limit": c.limit,
                            "cursor": c.cursor,
                            "include_total": c.include_total,
                            "offset": c.offset,
                        }),
                    ),
                },
                GatewayCommand::Mcp(_) => unreachable!("handled above"),
                GatewayCommand::Code(_) => unreachable!("handled above"),
            };

            if dry_run {
                crate::cli::helpers::print_dry_run("gateway", &action, &params, format);
                return Ok(ExitCode::SUCCESS);
            }

            return run_confirmable_action_command(
                "gateway",
                crate::dispatch::gateway::ACTIONS,
                action,
                params,
                confirmed,
                format,
                |action, params| async move {
                    dispatch_gateway_action(manager, config, action, params).await
                },
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cli::gateway::LazyGatewayManager;
    use crate::cli::{Cli, Command};
    use crate::config::{LabConfig, ProtectedGatewaySubsetTarget, ProtectedMcpRouteTarget};

    use super::*;

    #[tokio::test]
    async fn dispatch_gateway_action_never_builds_local_manager_when_remote_succeeds() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/labby.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "apiBaseUrl": server.uri(),
                "paletteCatalogUrl": format!("{}/v1/palette/catalog", server.uri()),
                "paletteExecuteUrl": format!("{}/v1/palette/execute", server.uri()),
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"name": "example"}])))
            .mount(&server)
            .await;
        // `detect` identifies a daemon one of two ways, and which one it picks
        // depends on the *ambient* environment: with no `LABBY_MCP_HTTP_TOKEN`
        // it trusts `/.well-known/labby.json`, but when that variable is set in
        // the process environment it skips discovery entirely and probes
        // `/v1/gateway/actions` for `gateway.reload`. Mocking only the
        // discovery route made this test pass or fail according to whether the
        // machine running it happened to have a labby token exported — which is
        // why it failed only on the self-hosted Windows runner. Mock both, so
        // the test asserts dispatch behaviour rather than the state of the
        // host's environment.
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!([{"name": "gateway.reload"}])),
            )
            .mount(&server)
            .await;

        let url = url::Url::parse(&server.uri()).expect("wiremock uri parses");
        let mut config = LabConfig::default();
        config.mcp.host = Some(url.host_str().expect("wiremock host").to_string());
        config.mcp.port = url.port();

        let manager = LazyGatewayManager::new(&config, false);
        let result =
            dispatch_gateway_action(&manager, &config, "gateway.list".to_string(), json!({}))
                .await
                .expect("remote dispatch should succeed");

        assert_eq!(result, json!([{"name": "example"}]));
        assert!(
            manager.built().is_none(),
            "local GatewayManager (and its auth.db) must not be constructed when the remote path succeeds"
        );
    }

    #[test]
    fn missing_daemon_is_not_a_local_authority_fallback() {
        let error = gateway_daemon_unavailable();
        assert!(matches!(
            error,
            ToolError::Sdk { ref sdk_kind, ref message }
                if sdk_kind == "daemon_unavailable"
                    && message.contains("authoritative Labby daemon")
        ));
    }

    fn parsed_update(args: &[&str]) -> GatewayUpdateArgs {
        let cli = Cli::try_parse_from(args).expect("parse gateway update args");
        let Command::Gateway(gateway) = cli.command else {
            panic!("expected gateway command");
        };
        let GatewayCommand::Update(update) = gateway.command else {
            panic!("expected gateway update command");
        };
        update
    }

    fn parsed_protected_route(args: &[&str]) -> GatewayProtectedRouteCommand {
        let cli = Cli::try_parse_from(args).expect("parse protected-route args");
        let Command::Gateway(gateway) = cli.command else {
            panic!("expected gateway command");
        };
        let GatewayCommand::ProtectedRoute(route) = gateway.command else {
            panic!("expected protected-route command");
        };
        route.command
    }

    fn gateway_subset_target(route: ProtectedMcpRouteConfig) -> ProtectedGatewaySubsetTarget {
        let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = route.target else {
            panic!("expected gateway-subset target");
        };
        target
    }

    #[test]
    fn protected_route_add_and_test_bind_project() {
        for operation in ["add", "test"] {
            let command = parsed_protected_route(&[
                "labby",
                "gateway",
                "protected-route",
                operation,
                "--name",
                "ops",
                "--public-host",
                "mcp.example.com",
                "--public-path",
                "/ops",
                "--gateway-subset",
                "--project-id",
                "project-42",
            ]);
            let args = match command {
                GatewayProtectedRouteCommand::Add(args)
                | GatewayProtectedRouteCommand::Test(args) => args,
                _ => panic!("expected add or test"),
            };
            let target = gateway_subset_target(protected_route_from_args(args));
            assert_eq!(target.project_id.as_deref(), Some("project-42"));
        }
    }

    #[test]
    fn protected_route_update_preserves_or_explicitly_clears_project() {
        let base = [
            "labby",
            "gateway",
            "protected-route",
            "update",
            "ops",
            "--public-host",
            "mcp.example.com",
            "--public-path",
            "/ops",
            "--gateway-subset",
        ];
        let GatewayProtectedRouteCommand::Update(args) = parsed_protected_route(&base) else {
            panic!("expected update");
        };
        let update = protected_route_from_update_args(args);
        let target = gateway_subset_target(update.route);
        assert_eq!(target.project_id, None);
        assert!(update.preserve_project_id);

        let GatewayProtectedRouteCommand::Update(args) = parsed_protected_route(&[
            "labby",
            "gateway",
            "protected-route",
            "update",
            "ops",
            "--public-host",
            "mcp.example.com",
            "--public-path",
            "/ops",
            "--clear-project-id",
        ]) else {
            panic!("expected update");
        };
        let update = protected_route_from_update_args(args);
        let target = gateway_subset_target(update.route);
        assert_eq!(target.project_id, None);
        assert!(!update.preserve_project_id);
    }

    #[test]
    fn gateway_update_command_transport_clears_url_side() {
        let update = parsed_update(&[
            "lab",
            "gateway",
            "update",
            "fixture",
            "--command",
            "local-mcp-server",
            "--arg=--stdio",
        ]);

        assert_eq!(
            update_patch_from_args(update),
            json!({
                "url": null,
                "command": "local-mcp-server",
                "args": ["--stdio"],
            })
        );
    }

    #[test]
    fn gateway_update_url_transport_clears_stdio_side() {
        let update = parsed_update(&[
            "lab",
            "gateway",
            "update",
            "fixture",
            "--url",
            "https://example.test/mcp",
        ]);

        assert_eq!(
            update_patch_from_args(update),
            json!({
                "url": "https://example.test/mcp",
                "command": null,
                "args": [],
            })
        );
    }

    #[test]
    fn gateway_update_explicit_clear_flags_emit_nullable_patch_fields() {
        let update = parsed_update(&[
            "lab",
            "gateway",
            "update",
            "fixture",
            "--clear-url",
            "--clear-command",
            "--clear-bearer-token-env",
        ]);

        assert_eq!(
            update_patch_from_args(update),
            json!({
                "url": null,
                "command": null,
                "bearer_token_env": null,
            })
        );
    }

    #[test]
    fn gateway_update_proxy_resources_omits_nullable_transport_fields() {
        let update = parsed_update(&[
            "lab",
            "gateway",
            "update",
            "fixture",
            "--proxy-resources",
            "false",
        ]);

        assert_eq!(
            update_patch_from_args(update),
            json!({
                "proxy_resources": false,
            })
        );
    }

    fn parsed_usage(args: &[&str]) -> GatewayUsageCommand {
        let cli = Cli::try_parse_from(args).expect("parse gateway usage args");
        let Command::Gateway(gateway) = cli.command else {
            panic!("expected gateway command");
        };
        let GatewayCommand::Usage(usage) = gateway.command else {
            panic!("expected gateway usage command");
        };
        usage.command
    }

    fn usage_params(command: GatewayUsageCommand) -> Value {
        match command {
            GatewayUsageCommand::Metrics(m) => json!({
                "since_unix": m.since_unix,
                "until_unix": m.until_unix,
                "upstream": m.upstream,
                "tool": m.tool,
                "capability": m.capability,
                "operation": m.operation,
                "subject_scoped": m.subject_scoped,
                "actor": m.actor,
                "outcome": m.outcome,
                "search": m.search,
                "bucket_count": m.bucket_count,
                "timezone": m.timezone,
                "timezone_offset_minutes": m.timezone_offset_minutes,
                "include_facets": m.include_facets,
            }),
            GatewayUsageCommand::Calls(c) => json!({
                "since_unix": c.since_unix,
                "until_unix": c.until_unix,
                "upstream": c.upstream,
                "tool": c.tool,
                "capability": c.capability,
                "operation": c.operation,
                "subject_scoped": c.subject_scoped,
                "actor": c.actor,
                "outcome": c.outcome,
                "search": c.search,
                "limit": c.limit,
                "cursor": c.cursor,
                "include_total": c.include_total,
                "offset": c.offset,
            }),
        }
    }

    #[test]
    fn gateway_usage_metrics_parses_flags_into_params() {
        let usage = parsed_usage(&[
            "lab",
            "gateway",
            "usage",
            "metrics",
            "--since-unix",
            "1000",
            "--until-unix",
            "2000",
            "--upstream",
            "github",
            "--tool",
            "github::search_repos",
            "--capability",
            "resources",
            "--operation",
            "resource.read",
            "--subject-scoped",
            "true",
            "--actor",
            "codex",
            "--outcome",
            "failed",
            "--search",
            "timeout",
            "--bucket-count",
            "24",
            "--timezone",
            "America/New_York",
            "--timezone-offset-minutes",
            "-240",
            "--include-facets",
        ]);

        assert_eq!(
            usage_params(usage),
            json!({
                "since_unix": 1000,
                "until_unix": 2000,
                "upstream": "github",
                "tool": "github::search_repos",
                "capability": "resources",
                "operation": "resource.read",
                "subject_scoped": true,
                "actor": "codex",
                "outcome": "failed",
                "search": "timeout",
                "bucket_count": 24,
                "timezone": "America/New_York",
                "timezone_offset_minutes": -240,
                "include_facets": true,
            })
        );
    }

    #[test]
    fn gateway_usage_metrics_defaults_are_null() {
        let usage = parsed_usage(&["lab", "gateway", "usage", "metrics"]);

        assert_eq!(
            usage_params(usage),
            json!({
                "since_unix": null,
                "until_unix": null,
                "upstream": null,
                "tool": null,
                "capability": null,
                "operation": null,
                "subject_scoped": null,
                "actor": null,
                "outcome": null,
                "search": null,
                "bucket_count": null,
                "timezone": null,
                "timezone_offset_minutes": null,
                "include_facets": false,
            })
        );
    }

    #[test]
    fn gateway_usage_calls_parses_flags_into_params() {
        let usage = parsed_usage(&[
            "lab", "gateway", "usage", "calls", "--limit", "50", "--offset", "10",
        ]);

        assert_eq!(
            usage_params(usage),
            json!({
                "since_unix": null,
                "until_unix": null,
                "upstream": null,
                "tool": null,
                "capability": null,
                "operation": null,
                "subject_scoped": null,
                "actor": null,
                "outcome": null,
                "search": null,
                "limit": 50,
                "cursor": null,
                "include_total": false,
                "offset": 10,
            })
        );
    }

    #[test]
    fn gateway_usage_calls_defaults_are_null() {
        let usage = parsed_usage(&["lab", "gateway", "usage", "calls"]);

        assert_eq!(
            usage_params(usage),
            json!({
                "since_unix": null,
                "until_unix": null,
                "upstream": null,
                "tool": null,
                "capability": null,
                "operation": null,
                "subject_scoped": null,
                "actor": null,
                "outcome": null,
                "search": null,
                "limit": null,
                "cursor": null,
                "include_total": false,
                "offset": null,
            })
        );
    }

    #[test]
    fn gateway_usage_calls_accepts_keyset_cursor() {
        let usage = parsed_usage(&[
            "lab",
            "gateway",
            "usage",
            "calls",
            "--cursor",
            "123:45",
            "--include-total",
        ]);

        assert_eq!(
            usage_params(usage),
            json!({
                "since_unix": null,
                "until_unix": null,
                "upstream": null,
                "tool": null,
                "capability": null,
                "operation": null,
                "subject_scoped": null,
                "actor": null,
                "outcome": null,
                "search": null,
                "limit": null,
                "cursor": "123:45",
                "include_total": true,
                "offset": null,
            })
        );
    }
}
