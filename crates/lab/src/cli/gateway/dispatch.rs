use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use serde_json::json;

use crate::cli::gateway::{
    GatewayArgs, GatewayCommand, GatewayMcpAuthCommand, GatewayMcpCommand, GatewayPendingCommand,
    GatewayProtectedRouteCommand, GatewayProtectedRouteUpdateArgs, GatewayProtectedRouteUpsertArgs,
    GatewayQuarantineCommand,
};
use crate::cli::helpers::{run_action_command, run_confirmable_action_command};
use crate::config::ProtectedMcpRouteConfig;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::output::OutputFormat;

use super::code::run_gateway_code;
use super::list::run_gateway_list;
use super::oauth::run_gateway_oauth_start;

fn protected_route_target_from_args(
    gateway_subset: bool,
    upstreams: Vec<String>,
    services: Vec<String>,
    expose_code_mode: bool,
) -> Option<crate::config::ProtectedMcpRouteTarget> {
    gateway_subset.then_some(crate::config::ProtectedMcpRouteTarget::GatewaySubset(
        crate::config::ProtectedGatewaySubsetTarget {
            upstreams,
            services,
            expose_code_mode,
        },
    ))
}

fn protected_route_from_args(args: GatewayProtectedRouteUpsertArgs) -> ProtectedMcpRouteConfig {
    let target = protected_route_target_from_args(
        args.gateway_subset,
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

fn protected_route_from_update_args(
    args: GatewayProtectedRouteUpdateArgs,
) -> (String, ProtectedMcpRouteConfig) {
    let target = protected_route_target_from_args(
        args.gateway_subset,
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
    (name, route)
}

pub(super) async fn dispatch_command(
    manager: Arc<GatewayManager>,
    args: GatewayArgs,
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
                    return run_gateway_oauth_start(manager, args, format).await;
                }
                GatewayMcpAuthCommand::Open(mut args) => {
                    args.open = true;
                    return run_gateway_oauth_start(manager, args, format).await;
                }
                GatewayMcpAuthCommand::Status(args) => {
                    return run_action_command(
                        "gateway",
                        "gateway.oauth.status".to_string(),
                        json!({ "upstream": args.name, "subject": args.subject }),
                        format,
                        |action, params| async move {
                            crate::dispatch::gateway::dispatch_with_manager(
                                &manager, &action, params,
                            )
                            .await
                        },
                    )
                    .await;
                }
                GatewayMcpAuthCommand::Clear(args) => {
                    return run_action_command(
                        "gateway",
                        "gateway.oauth.clear".to_string(),
                        json!({ "upstream": args.name, "subject": args.subject }),
                        format,
                        |action, params| async move {
                            crate::dispatch::gateway::dispatch_with_manager(
                                &manager, &action, params,
                            )
                            .await
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
                        crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params)
                            .await
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
                        crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params)
                            .await
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
                        crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params)
                            .await
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
                        crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params)
                            .await
                    },
                )
                .await;
            }
        },
        GatewayCommand::List => {
            return run_gateway_list(manager, format).await;
        }
        command => {
            if let GatewayCommand::Code(args) = command {
                return run_gateway_code(manager, args, format).await;
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
                        }
                    }),
                ),
                GatewayCommand::Update(args) => (
                    "gateway.update".to_string(),
                    json!({
                        "name": args.name,
                        "origin": cli_origin,
                        "owner": cli_owner,
                        "patch": {
                            "name": args.new_name,
                            "url": args.url.map(Some),
                            "command": args.command.map(Some),
                            "args": if args.args.is_empty() { None::<Vec<String>> } else { Some(args.args) },
                            "bearer_token_env": args.bearer_token_env.map(Some),
                            "proxy_resources": args.proxy_resources,
                        }
                    }),
                ),
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
                        ("gateway.protected_route.list".to_string(), json!({}))
                    }
                    GatewayProtectedRouteCommand::Get(args) => (
                        "gateway.protected_route.get".to_string(),
                        json!({ "name": args.name }),
                    ),
                    GatewayProtectedRouteCommand::Add(args) => (
                        "gateway.protected_route.add".to_string(),
                        json!({ "route": protected_route_from_args(args) }),
                    ),
                    GatewayProtectedRouteCommand::Update(args) => {
                        let (name, route) = protected_route_from_update_args(args);
                        (
                            "gateway.protected_route.update".to_string(),
                            json!({ "name": name, "route": route }),
                        )
                    }
                    GatewayProtectedRouteCommand::Remove(args) => (
                        "gateway.protected_route.remove".to_string(),
                        json!({ "name": args.name }),
                    ),
                    GatewayProtectedRouteCommand::Test(args) => (
                        "gateway.protected_route.test".to_string(),
                        json!({ "route": protected_route_from_args(args) }),
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
                    crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params).await
                },
            )
            .await;
        }
    }
}
