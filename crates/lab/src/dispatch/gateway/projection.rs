use std::collections::HashMap;

use crate::config::{ToolSearchConfig, UpstreamConfig};
use crate::dispatch::gateway::service_catalog::service_meta;
use crate::dispatch::gateway::types::{
    GatewayConfigView, GatewayRuntimeView, ServiceConfigFieldView, ServiceConfigView,
};
use crate::dispatch::gateway::view_models::{
    ServerConfigSummaryView, ServerView, SurfaceStateView, SurfaceStatesView,
};
use crate::dispatch::gateway::virtual_servers::{VirtualServerRecord, VirtualServerSource};
use crate::dispatch::redact::{redact_stdio_args, redact_stdio_value, redact_url};
use crate::dispatch::upstream::pool::{UpstreamCachedSummary, UpstreamPool};
use crate::tui::events::ServiceHealth;

const WARNING_UNKNOWN_SERVICE: &str = "unknown_service";

pub(super) fn config_view(
    upstream: &UpstreamConfig,
    tool_search: &ToolSearchConfig,
) -> GatewayConfigView {
    GatewayConfigView {
        name: upstream.name.clone(),
        enabled: upstream.enabled,
        url: upstream.url.as_deref().map(redact_url),
        command: upstream.command.as_deref().map(redact_stdio_value),
        args: redact_stdio_args(&upstream.args),
        bearer_token_env: upstream.bearer_token_env.clone(),
        oauth_enabled: upstream.oauth.is_some(),
        proxy_resources: upstream.proxy_resources,
        proxy_prompts: upstream.proxy_prompts,
        expose_tools: upstream.expose_tools.clone(),
        expose_resources: upstream.expose_resources.clone(),
        expose_prompts: upstream.expose_prompts.clone(),
        tool_search_enabled: tool_search.enabled,
        tool_search_top_k_default: tool_search.top_k_default,
        tool_search_max_tools: tool_search.max_tools,
        imported_from: upstream.imported_from.clone(),
    }
}

pub(super) fn sanitize_tool_text(input: &str, max_len: usize) -> String {
    let mut sanitized = input.to_string();
    sanitized.retain(|ch| {
        !matches!(
            ch,
            '\u{0000}'..='\u{001F}'
                | '\u{007F}'..='\u{009F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
    });
    for marker in ["<system>", "[INST]", "###", "<<"] {
        sanitized = sanitized.replace(marker, "");
    }
    redact_secret_like_segments(&sanitized)
        .chars()
        .take(max_len)
        .collect()
}

fn redact_secret_like_segments(input: &str) -> String {
    input
        .split_whitespace()
        .map(|segment| {
            let looks_secret = segment.starts_with("sk-")
                || segment.starts_with("ghp_")
                || segment.starts_with("github_pat_")
                || segment.starts_with("glpat-")
                || segment.starts_with("xoxb-")
                || segment.starts_with("xoxp-")
                || segment.starts_with("eyJ");
            if looks_secret {
                "<redacted>".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn sanitize_schema(schema: Option<serde_json::Value>) -> Option<serde_json::Value> {
    fn recurse(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                *text = sanitize_tool_text(text, 2048);
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    recurse(value);
                }
            }
            serde_json::Value::Object(map) => {
                for value in map.values_mut() {
                    recurse(value);
                }
            }
            _ => {}
        }
    }

    schema.map(|mut value| {
        recurse(&mut value);
        value
    })
}

pub(super) fn redacted_gateway_target(upstream: &UpstreamConfig) -> Option<String> {
    upstream.url.as_deref().map(redact_url).or_else(|| {
        upstream.command.as_deref().map(|command| {
            let args = redact_stdio_args(&upstream.args);
            format_redacted_gateway_command(command, &args)
        })
    })
}

pub(super) fn format_redacted_gateway_command(command: &str, args: &[String]) -> String {
    if command == "env" {
        let _ = args;
        return "env".to_string();
    }

    redact_stdio_value(command)
}

pub(super) fn empty_upstream_summary() -> UpstreamCachedSummary {
    UpstreamCachedSummary::default()
}

fn is_nonessential_capability_error(message: &str) -> bool {
    // Only suppress the well-known optional-capability discovery failures
    // (prompts/resources list not implemented). Broad "-32601" / "Method not
    // found" matching would also hide real tool-call or handshake failures.
    message.starts_with("failed to list prompts from upstream:")
        || message.starts_with("failed to list resources from upstream:")
        || message.starts_with("does not implement MCP prompts discovery")
        || message.starts_with("does not implement MCP resources discovery")
}

pub(super) fn operator_visible_upstream_error(message: Option<String>) -> Option<String> {
    message.filter(|message| !is_nonessential_capability_error(message))
}

pub(super) async fn upstream_summary(
    pool: Option<&UpstreamPool>,
    upstream_name: &str,
) -> UpstreamCachedSummary {
    let Some(pool) = pool else {
        return empty_upstream_summary();
    };

    pool.cached_upstream_summary(upstream_name)
        .await
        .unwrap_or_else(empty_upstream_summary)
}

pub(super) async fn server_view_from_upstream(
    pool: Option<&UpstreamPool>,
    upstream: &UpstreamConfig,
) -> ServerView {
    let summary = upstream_summary(pool, &upstream.name).await;
    let last_error = operator_visible_upstream_error(match pool {
        Some(pool) => pool.upstream_last_error(&upstream.name).await,
        None => None,
    });
    let connected = summary.exposed_tool_count > 0
        || summary.exposed_resource_count > 0
        || summary.exposed_prompt_count > 0;
    let enabled = upstream.enabled;

    ServerView {
        id: upstream.name.clone(),
        name: upstream.name.clone(),
        source: "custom_gateway".to_string(),
        configured: true,
        enabled,
        connected: enabled && connected,
        discovered_tool_count: summary.discovered_tool_count,
        exposed_tool_count: summary.exposed_tool_count,
        discovered_resource_count: summary.discovered_resource_count,
        exposed_resource_count: summary.exposed_resource_count,
        discovered_prompt_count: summary.discovered_prompt_count,
        exposed_prompt_count: summary.exposed_prompt_count,
        surfaces: SurfaceStatesView {
            mcp: SurfaceStateView {
                enabled,
                connected: enabled && connected,
            },
            ..SurfaceStatesView::default()
        },
        warnings: last_error
            .as_ref()
            .map(|message| {
                vec![super::view_models::ServerWarningView {
                    code: "connection_error".to_string(),
                    message: message.clone(),
                }]
            })
            .unwrap_or_default(),
        config_summary: ServerConfigSummaryView {
            transport: Some(if upstream.command.is_some() {
                "stdio".to_string()
            } else {
                "http".to_string()
            }),
            target: redacted_gateway_target(upstream),
        },
    }
}

pub(super) fn degraded_server_warning_count(views: &[ServerView], code: &str) -> usize {
    views
        .iter()
        .filter(|view| view.warnings.iter().any(|warning| warning.code == code))
        .count()
}

pub(super) fn server_view_from_virtual_server(
    config: &crate::config::VirtualServerConfig,
    summary: UpstreamCachedSummary,
    last_error: Option<String>,
    health: Option<&ServiceHealth>,
) -> ServerView {
    let record = VirtualServerRecord::from(config);
    let service = match &record.source {
        VirtualServerSource::LabService { service } => service.clone(),
    };
    let service_known = service_meta(&service).is_some();
    let peer_connected = last_error.is_none()
        && (summary.discovered_tool_count > 0
            || summary.discovered_resource_count > 0
            || summary.discovered_prompt_count > 0);
    let health_connected = health
        .map(|health| health.reachable && health.auth_ok)
        .unwrap_or(false);
    let connected = service_known && record.enabled && (peer_connected || health_connected);
    let mcp_exposed = record.enabled && record.surfaces.mcp;
    let discovered_tool_count = summary.discovered_tool_count;
    let policy_exposed_tool_count = record.mcp_policy.as_ref().and_then(|policy| {
        (!policy.allowed_actions.is_empty()).then_some(policy.allowed_actions.len() + 2)
    });
    let exposed_tool_count = if mcp_exposed {
        policy_exposed_tool_count
            .map(|count| summary.exposed_tool_count.min(count))
            .unwrap_or(summary.exposed_tool_count)
    } else {
        0
    };
    let discovered_resource_count = summary.discovered_resource_count;
    let exposed_resource_count = if mcp_exposed {
        summary.exposed_resource_count
    } else {
        0
    };
    let discovered_prompt_count = summary.discovered_prompt_count;
    let exposed_prompt_count = if mcp_exposed {
        summary.exposed_prompt_count
    } else {
        0
    };
    let mut warnings = Vec::new();
    if !service_known {
        warnings.push(super::view_models::ServerWarningView {
            code: WARNING_UNKNOWN_SERVICE.to_string(),
            message: format!("service `{service}` is not registered in this lab binary"),
        });
    }
    if let Some(message) = last_error {
        warnings.push(super::view_models::ServerWarningView {
            code: "connection_error".to_string(),
            message,
        });
    }

    ServerView {
        id: record.id.clone(),
        name: service.clone(),
        source: "in_process".to_string(),
        configured: true,
        enabled: record.enabled,
        connected,
        discovered_tool_count,
        exposed_tool_count,
        discovered_resource_count,
        exposed_resource_count,
        discovered_prompt_count,
        exposed_prompt_count,
        surfaces: SurfaceStatesView {
            cli: SurfaceStateView {
                enabled: record.surfaces.cli,
                connected: record.surfaces.cli && connected,
            },
            api: SurfaceStateView {
                enabled: record.surfaces.api,
                connected: record.surfaces.api && connected,
            },
            mcp: SurfaceStateView {
                enabled: record.surfaces.mcp,
                connected: record.surfaces.mcp && connected,
            },
            webui: SurfaceStateView {
                enabled: record.surfaces.webui,
                connected: record.surfaces.webui && connected,
            },
        },
        warnings,
        config_summary: ServerConfigSummaryView {
            transport: Some("in_process".to_string()),
            target: Some(service),
        },
    }
}

pub(super) fn service_config_view(
    meta: &lab_apis::core::PluginMeta,
    values: &HashMap<String, String>,
) -> ServiceConfigView {
    let mut fields = Vec::new();
    for env in meta.required_env.iter().chain(meta.optional_env.iter()) {
        let value = values
            .get(env.name)
            .and_then(|value| (!value.trim().is_empty()).then_some(value));
        fields.push(ServiceConfigFieldView {
            name: env.name.to_string(),
            present: value.is_some(),
            secret: env.secret,
            value_preview: value.and_then(|value| (!env.secret).then(|| value.clone())),
        });
    }

    ServiceConfigView {
        service: meta.name.to_string(),
        // A service with no env vars needs no configuration and is always ready.
        configured: if fields.is_empty() {
            true
        } else {
            meta.required_env.iter().all(|env| {
                fields
                    .iter()
                    .any(|field| field.name == env.name && field.present)
            })
        },
        fields,
    }
}

pub(super) async fn runtime_view(
    pool: Option<&UpstreamPool>,
    name: &str,
    prompt_owners: Option<&HashMap<String, String>>,
) -> GatewayRuntimeView {
    let Some(pool) = pool else {
        return GatewayRuntimeView {
            name: name.to_string(),
            ..GatewayRuntimeView::default()
        };
    };

    let summary = pool
        .cached_upstream_summary(name)
        .await
        .unwrap_or_else(empty_upstream_summary);
    let prompt_count = match prompt_owners {
        Some(owners) => owners.values().filter(|owner| *owner == name).count(),
        None => summary.exposed_prompt_count,
    };

    GatewayRuntimeView {
        name: name.to_string(),
        tool_count: summary.discovered_tool_count,
        resource_count: summary.discovered_resource_count,
        prompt_count,
        exposed_tool_count: summary.exposed_tool_count,
        exposed_resource_count: summary.exposed_resource_count,
        exposed_prompt_count: summary.exposed_prompt_count,
        last_error: operator_visible_upstream_error(pool.upstream_last_error(name).await),
    }
}
