use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::cli::gateway::{
    GatewayArgs, GatewayCommand, GatewayMcpAuthCommand, GatewayMcpCommand, GatewayPendingCommand,
    GatewayProtectedRouteCommand, GatewayProtectedRouteUpdateArgs, GatewayProtectedRouteUpsertArgs,
    GatewayQuarantineCommand, GatewayUpdateArgs,
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

fn update_patch_from_args(args: GatewayUpdateArgs) -> Value {
    let url_was_set = args.url.is_some();
    let command_was_set = args.command.is_some();
    let mut patch = Map::new();

    insert_if_some(&mut patch, "name", args.new_name);
    insert_if_some(&mut patch, "proxy_resources", args.proxy_resources);

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

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::json;

    use crate::cli::{Cli, Command};

    use super::*;

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
}
