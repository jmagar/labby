use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::gateway::service_registry::GatewayServiceRegistry;
use crate::gateway::types::{
    GatewayConfigView, GatewayRuntimeView, ServiceConfigFieldView, ServiceConfigView,
};
use crate::gateway::view_models::{
    ServerConfigSummaryView, ServerView, SurfaceStateView, SurfaceStatesView,
};
use crate::gateway::virtual_servers::{VirtualServerRecord, VirtualServerSource};
use crate::upstream::pool::{UpstreamCachedSummary, UpstreamPool};
use labby_runtime::gateway_config::{CodeModeConfig, UpstreamConfig, normalize_code_mode_hint};
use labby_runtime::redact::{redact_stdio_args, redact_stdio_value, redact_url};
/// Per-service health probe result. Carried through gateway projection so the
/// `ServerView` can surface upstream-service reachability without forcing the
/// caller to thread separate fields.
#[derive(Debug, Clone)]
pub(crate) struct ServiceHealth {
    pub reachable: bool,
    pub auth_ok: bool,
}

const WARNING_UNKNOWN_SERVICE: &str = "unknown_service";

pub(super) fn config_view(
    upstream: &UpstreamConfig,
    code_mode: &CodeModeConfig,
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
        code_mode_hint: upstream
            .code_mode_hint
            .as_deref()
            .and_then(normalize_code_mode_hint),
        code_mode_enabled: code_mode.enabled,
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

/// Regex that matches common secret token patterns even when embedded in
/// longer strings (e.g. `Authorization: Bearer sk-abc123...`). Applied as a
/// secondary pass after the whitespace-split pass so that both standalone and
/// embedded secrets are caught.
static SECRET_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:sk-[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{82}|glpat-[A-Za-z0-9_-]{20}|xox[bp]-[A-Za-z0-9-]+|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)",
    )
    .expect("SECRET_REGEX is valid")
});

fn redact_secret_like_segments(input: &str) -> String {
    // First pass: whitespace-split heuristic (fast path for standalone tokens).
    let after_split = input
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
                "[REDACTED]".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    // Second pass: regex catches embedded secrets (e.g. in header values).
    SECRET_REGEX
        .replace_all(&after_split, "[REDACTED]")
        .into_owned()
}

/// Pathological-input backstop for schemas carried in the Code Mode catalog.
///
/// The catalog is injected into the Javy sandbox as `const tools` and never
/// enters model context (see `code_mode/search.rs`), so schema size has no
/// token cost. This ceiling only rejects absurd upstreams; normal large
/// action-routed schemas (e.g. cortex, axon) MUST survive so that
/// `generate_tool_types` can emit real `dts`/signatures instead of `unknown`.
/// Raised from 16 KB, which silently dropped those schemas to `None` and
/// collapsed their generated types to `unknown`.
const MAX_SCHEMA_BYTES: usize = 524_288;

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

    schema.and_then(|raw| {
        // Reject only pathologically large schemas; normal large tool schemas must pass.
        if raw.to_string().len() > MAX_SCHEMA_BYTES {
            return None;
        }
        let mut value = raw;
        recurse(&mut value);
        Some(value)
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

/// Build the redacted (command, args) pair for stdio transports.
///
/// Each segment is individually scrubbed: `redact_stdio_value` / `redact_stdio_args`
/// mask `KEY=value` env pairs and `--flag value` secret flags, then a second
/// `redact_secret_like_segments` pass catches bare positional tokens (e.g. a raw
/// `ghp_…` passed as an argument). Returns `(None, [])` for HTTP transports.
pub(super) fn redacted_stdio_command(upstream: &UpstreamConfig) -> (Option<String>, Vec<String>) {
    let Some(command) = upstream.command.as_deref() else {
        return (None, Vec::new());
    };
    let command = redact_secret_like_segments(&redact_stdio_value(command));
    let args = redact_stdio_args(&upstream.args)
        .iter()
        .map(|arg| redact_secret_like_segments(arg))
        .collect();
    (Some(command), args)
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

pub(super) fn upstream_warning_code(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("auth required")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("invalid_token")
        || lower.contains("oauth")
    {
        "auth_failed"
    } else if lower.contains("bearer")
        || lower.contains("token")
        || lower.contains("api key")
        || lower.contains("api_key")
    {
        "auth_required"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "timeout"
    } else if lower.contains("dns") || lower.contains("name or service not known") {
        "dns_error"
    } else if lower.contains("connection refused") {
        "connection_refused"
    } else {
        "connection_error"
    }
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
    let health = match pool {
        Some(pool) => pool.upstream_tool_health(&upstream.name).await,
        None => None,
    };
    // Health-aware connectivity (mirrors `server_view_from_virtual_server`): an
    // upstream counts as connected when it is actively exposing capabilities OR
    // it is healthy with no recorded error. The health term keeps lazily
    // discovered upstreams — whose catalog stays empty until their first use —
    // from rendering as "Disconnected" at rest, while upstreams with a recorded
    // error or an open circuit breaker still surface as down.
    let exposing_capabilities = summary.exposed_tool_count > 0
        || summary.exposed_resource_count > 0
        || summary.exposed_prompt_count > 0;
    let health_ok =
        last_error.is_none() && health.map(|health| health.is_routable()).unwrap_or(false);
    let connected = exposing_capabilities || health_ok;
    let enabled = upstream.enabled;
    let pid = match pool {
        Some(pool) => pool
            .upstream_runtime_metadata(&upstream.name)
            .await
            .and_then(|meta| meta.pid),
        None => None,
    };
    let (command, args) = redacted_stdio_command(upstream);

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
                    code: upstream_warning_code(message).to_string(),
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
            command,
            args,
        },
        pid,
    }
}

pub(super) fn degraded_server_warning_count(views: &[ServerView], code: &str) -> usize {
    views
        .iter()
        .filter(|view| view.warnings.iter().any(|warning| warning.code == code))
        .count()
}

pub(super) fn server_view_from_virtual_server(
    config: &labby_runtime::gateway_config::VirtualServerConfig,
    summary: UpstreamCachedSummary,
    last_error: Option<String>,
    health: Option<&ServiceHealth>,
    registry: &dyn GatewayServiceRegistry,
) -> ServerView {
    let record = VirtualServerRecord::from(config);
    let service = match &record.source {
        VirtualServerSource::LabService { service } => service.clone(),
    };
    let service_known = registry.service_meta(&service).is_some();
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
            code: upstream_warning_code(&message).to_string(),
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
            command: None,
            args: Vec::new(),
        },
        pid: None,
    }
}

pub(super) fn service_config_view(
    meta: &labby_apis::core::PluginMeta,
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

// Expose for intra-crate tests
#[cfg(test)]
pub(crate) fn redact_secret_like_segments_for_test(input: &str) -> String {
    redact_secret_like_segments(input)
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── Stdio command projection ──────────────────────────────────────────────

    fn upstream_fixture(command: Option<&str>, args: &[&str], url: Option<&str>) -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: "fixture".to_string(),
            url: url.map(str::to_string),
            bearer_token_env: None,
            command: command.map(str::to_string),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }
    }

    fn stdio_upstream(command: &str, args: &[&str]) -> UpstreamConfig {
        upstream_fixture(Some(command), args, None)
    }

    #[test]
    fn stdio_command_exposes_command_and_args() {
        let (command, args) = redacted_stdio_command(&stdio_upstream("uvx", &["github-chat-mcp"]));
        assert_eq!(command.as_deref(), Some("uvx"));
        assert_eq!(args, vec!["github-chat-mcp".to_string()]);
    }

    #[test]
    fn stdio_command_redacts_env_secret_pair() {
        // `env GITHUB_TOKEN=abc npx foo` must mask the token value but keep the
        // rest of the invocation visible.
        let (command, args) = redacted_stdio_command(&stdio_upstream(
            "env",
            &["GITHUB_TOKEN=abc123", "npx", "foo"],
        ));
        assert_eq!(command.as_deref(), Some("env"));
        assert_eq!(args[0], "GITHUB_TOKEN=[redacted]");
        assert_eq!(&args[1..], &["npx".to_string(), "foo".to_string()]);
        let rendered = args.join(" ");
        assert!(
            !rendered.contains("abc123"),
            "secret value must not survive, got: {rendered}"
        );
    }

    #[test]
    fn stdio_command_redacts_bare_positional_token() {
        // A raw token passed positionally (not KEY=value / --flag) is caught by
        // the secret-pattern pass.
        let token = ["ghp_", &"a".repeat(36)].concat();
        let (_, args) = redacted_stdio_command(&stdio_upstream("npx", &["server", &token]));
        let rendered = args.join(" ");
        assert!(
            rendered.contains("[REDACTED]") && !rendered.contains("ghp_"),
            "bare positional token must be redacted, got: {rendered}"
        );
    }

    #[test]
    fn stdio_command_empty_for_http() {
        let http = upstream_fixture(None, &[], Some("https://example.com/mcp"));
        let (command, args) = redacted_stdio_command(&http);
        assert!(command.is_none());
        assert!(args.is_empty());
    }

    // ── Secret redaction ──────────────────────────────────────────────────────

    #[test]
    fn secret_redaction_catches_standalone_sk_token() {
        let input = "Authorization: sk-abc12345678901234567890";
        let result = redact_secret_like_segments_for_test(input);

        // PRESENCE: redacted marker appears
        assert!(
            result.contains("[REDACTED]"),
            "standalone sk- token must be redacted, got: {result}"
        );
        // ABSENCE: original secret must not appear
        assert!(
            !result.contains("sk-abc12345678901234567890"),
            "original sk- token must not survive, got: {result}"
        );
        // PRESENCE: non-secret part preserved
        assert!(
            result.contains("Authorization:"),
            "non-secret prefix must be preserved"
        );
    }

    #[test]
    fn secret_redaction_catches_embedded_sk_token() {
        let input = "url=https://api.example.com?token=sk-abc12345678901234567890&other=value";
        let result = redact_secret_like_segments_for_test(input);

        // PRESENCE: redacted marker appears
        assert!(
            result.contains("[REDACTED]"),
            "embedded sk- token must be redacted, got: {result}"
        );
        // ABSENCE: original secret must not appear
        assert!(
            !result.contains("sk-abc12345678901234567890"),
            "original embedded sk- token must not survive, got: {result}"
        );
    }

    #[test]
    fn secret_redaction_catches_github_pat() {
        let input = ["ghp_", "1234567890abcdef", "1234567890abcdef1234"].concat();
        let result = redact_secret_like_segments_for_test(&input);

        // PRESENCE: redacted
        assert!(
            result.contains("[REDACTED]"),
            "GitHub PAT (ghp_) must be redacted, got: {result}"
        );
        // ABSENCE: original must not appear
        assert!(
            !result.contains("ghp_"),
            "ghp_ prefix must not survive, got: {result}"
        );
    }

    #[test]
    fn secret_redaction_does_not_redact_normal_text() {
        let input = "the quick brown fox jumps over the lazy dog";
        let result = redact_secret_like_segments_for_test(input);

        // ABSENCE: no false positive on normal text
        assert!(
            !result.contains("[REDACTED]"),
            "normal text must not be redacted, got: {result}"
        );
        // PRESENCE: original text preserved
        assert_eq!(result, input);
    }

    #[test]
    fn sanitize_tool_text_strips_prompt_injection_markers() {
        let input = "<system>override</system> normal text ### injection";
        let result = sanitize_tool_text(input, 1024);

        // PRESENCE: normal text survives
        assert!(
            result.contains("normal text"),
            "normal text must survive sanitization"
        );
        // ABSENCE: injection markers must be stripped
        assert!(
            !result.contains("<system>"),
            "<system> marker must be stripped"
        );
        assert!(!result.contains("###"), "### marker must be stripped");
    }

    #[test]
    fn sanitize_tool_text_truncates_to_max_len() {
        let long_input = "a".repeat(200);
        let result = sanitize_tool_text(&long_input, 50);

        // PRESENCE: output is at most max_len chars
        assert!(
            result.chars().count() <= 50,
            "sanitize_tool_text must truncate to max_len"
        );
        // ABSENCE: must not be longer than the original
        assert!(result.len() <= long_input.len());
    }
}
