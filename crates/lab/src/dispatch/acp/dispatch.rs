//! Top-level action router for the `acp` dispatch service.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::acp::types::StartSessionInput;
use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, to_json};

use super::catalog::ACTIONS;
use super::client::require_registry;
use super::page_context::{PageContextInput, build_prompt_with_context};
use super::params::{
    LocalPromptAttachment, opt_str, opt_u64, require_str, validate_local_attachments,
};

/// SSE ticket lifetime in seconds.
const TICKET_TTL_SECS: u64 = 30;
const MAX_ACP_PARAMS_BYTES: usize = 64 * 1024;
const MAX_ACP_PROMPT_PARAMS_BYTES: usize = 384 * 1024;
const MAX_ACP_PROMPT_BYTES: usize = 32 * 1024;

fn require_confirm(params: &Value, action: &str) -> Result<(), ToolError> {
    if params
        .get("confirm")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(ToolError::ConfirmationRequired {
            message: format!("{action} is destructive; pass `\"confirm\": true` to proceed"),
        })
    }
}

fn ensure_params_size(params: &Value, max_bytes: usize) -> Result<(), ToolError> {
    let size = serde_json::to_vec(params)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if size > max_bytes {
        return Err(ToolError::Sdk {
            sdk_kind: "content_too_large".to_string(),
            message: format!("ACP params exceed {max_bytes} bytes (received {size} bytes)"),
        });
    }
    Ok(())
}

fn ensure_prompt_size(prompt: &str) -> Result<(), ToolError> {
    let size = prompt.len();
    if size > MAX_ACP_PROMPT_BYTES {
        return Err(ToolError::Sdk {
            sdk_kind: "content_too_large".to_string(),
            message: format!(
                "ACP prompt exceeds {MAX_ACP_PROMPT_BYTES} bytes (received {size} bytes)"
            ),
        });
    }
    Ok(())
}

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("acp", ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }
        other => {
            if !ACTIONS.iter().any(|a| a.name == other) {
                tracing::warn!(
                    surface = "acp",
                    service = "dispatch",
                    action = %other,
                    "ACP dispatch: unknown action",
                );
                return Err(ToolError::UnknownAction {
                    message: format!("unknown action `{other}` for service `acp`"),
                    valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
                    hint: None,
                });
            }
            let registry = require_registry()?;
            let started = std::time::Instant::now();
            tracing::info!(
                surface = "acp", service = "dispatch", action = %other,
                "ACP MCP tool call received",
            );
            let result = dispatch_with_registry(&registry, other, params).await;
            let elapsed_ms = started.elapsed().as_millis();
            if let Err(ref e) = result {
                tracing::warn!(
                    surface = "acp", service = "dispatch", action = %other,
                    elapsed_ms, error_kind = %e.kind(),
                    "ACP MCP tool call failed",
                );
            } else {
                tracing::info!(
                    surface = "acp", service = "dispatch", action = %other,
                    elapsed_ms, "ACP MCP tool call completed",
                );
            }
            result
        }
    }
}

pub async fn dispatch_with_registry(
    registry: &crate::acp::registry::AcpSessionRegistry,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let max_params_bytes = if action == "session.prompt" {
        MAX_ACP_PROMPT_PARAMS_BYTES
    } else {
        MAX_ACP_PARAMS_BYTES
    };
    ensure_params_size(&params, max_params_bytes)?;

    match action {
        "help" => Ok(help_payload("acp", ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }

        // ── Provider actions ──────────────────────────────────────────────
        "provider.list" => {
            let providers: Vec<_> = registry
                .provider_healths()
                .into_iter()
                .map(|health| {
                    json!({
                        "name": health.provider,
                        "available": health.available,
                        "version": health.version,
                        "error": health.message,
                    })
                })
                .collect();
            to_json(json!({
                "providers": providers
            }))
        }

        "provider.get" => {
            let provider = require_str(&params, "provider")?;
            let provider = crate::acp::runtime::normalize_provider_id(Some(provider));
            let health = registry
                .provider_healths()
                .into_iter()
                .find(|health| health.provider == provider)
                .ok_or_else(|| ToolError::InvalidParam {
                    message: format!("unknown provider `{provider}`"),
                    param: "provider".to_string(),
                })?;
            to_json(json!({
                "name": health.provider,
                "available": health.available,
                "version": health.version,
                "error": health.message,
            }))
        }

        "provider.select" => {
            let provider = require_str(&params, "provider")?;
            let provider = crate::acp::runtime::normalize_provider_id(Some(provider));
            if !registry
                .provider_healths()
                .iter()
                .any(|health| health.provider == provider)
            {
                return Err(ToolError::InvalidParam {
                    message: format!("unknown provider `{provider}`"),
                    param: "provider".to_string(),
                });
            }
            to_json(json!({ "selected": provider }))
        }

        // ── Session read actions ──────────────────────────────────────────
        "session.list" => {
            let principal = opt_str(&params, "principal").unwrap_or("");
            let sessions = registry.list_sessions(principal).await;
            to_json(json!({ "sessions": sessions }))
        }

        "session.get" => {
            let session_id = require_str(&params, "session_id")?;
            let summary = registry
                .get_session(session_id)
                .await
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("session `{session_id}` not found"),
                })?;
            to_json(summary)
        }

        "session.events" => {
            let session_id = require_str(&params, "session_id")?;
            let principal = opt_str(&params, "principal").unwrap_or("");
            let since = opt_u64(&params, "since")?.unwrap_or(0);
            let events = registry
                .get_events_since(session_id, since, principal)
                .await?;
            to_json(json!({ "events": events, "count": events.len() }))
        }

        // ── Session write actions ─────────────────────────────────────────
        "session.start" => {
            let principal = opt_str(&params, "principal").unwrap_or("");
            let provider = opt_str(&params, "provider").map(|s| s.to_string());
            let title = opt_str(&params, "title").map(|s| s.to_string());
            let cwd = opt_str(&params, "cwd").unwrap_or("").to_string();

            let input = StartSessionInput {
                provider,
                title,
                cwd,
                principal: if principal.is_empty() {
                    None
                } else {
                    Some(principal.to_string())
                },
            };
            let summary = registry.create_session(input, principal).await?;
            to_json(summary)
        }

        "session.prompt" => {
            let session_id = require_str(&params, "session_id")?;
            let principal = opt_str(&params, "principal").unwrap_or("");
            let raw_text = params
                .get("text")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ToolError::MissingParam {
                    message: "required param `text` is missing or empty".to_string(),
                    param: "text".to_string(),
                })?;
            ensure_prompt_size(raw_text)?;

            // Optional structured page context (HTTP / MCP / CLI can all supply it).
            let page_ctx = params
                .get("page_context")
                .and_then(|v| v.as_object())
                .map(|obj| PageContextInput {
                    route: obj.get("route").and_then(|v| v.as_str()).unwrap_or(""),
                    entity_type: obj.get("entityType").and_then(|v| v.as_str()),
                    entity_id: obj.get("entityId").and_then(|v| v.as_str()),
                });
            let effective_text = build_prompt_with_context(session_id, raw_text, page_ctx.as_ref());
            ensure_prompt_size(&effective_text)?;

            let attachments: Vec<LocalPromptAttachment> = params
                .get("attachments")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| ToolError::InvalidParam {
                    message: format!("invalid attachments payload: {error}"),
                    param: "attachments".into(),
                })?
                .unwrap_or_default();
            validate_local_attachments(&attachments)?;

            registry
                .prompt_session_with_attachments(
                    session_id,
                    &effective_text,
                    attachments,
                    principal,
                )
                .await?;
            to_json(json!({ "ok": true, "session_id": session_id }))
        }

        "session.cancel" => {
            require_confirm(&params, "session.cancel")?;
            let session_id = require_str(&params, "session_id")?;
            let principal = opt_str(&params, "principal").unwrap_or("");
            registry.cancel_session(session_id, principal).await?;
            to_json(json!({ "ok": true, "session_id": session_id }))
        }

        "session.permission.approve" => {
            require_confirm(&params, "session.permission.approve")?;
            let session_id = require_str(&params, "session_id")?;
            let request_id = require_str(&params, "request_id")?;
            let option_id = require_str(&params, "option_id")?;
            let principal = require_str(&params, "principal")?;
            registry
                .approve_permission(session_id, principal, request_id, option_id)
                .await?;
            to_json(json!({
                "ok": true,
                "session_id": session_id,
                "request_id": request_id,
                "option_id": option_id,
            }))
        }

        "session.permission.reject" => {
            let session_id = require_str(&params, "session_id")?;
            let request_id = require_str(&params, "request_id")?;
            let principal = opt_str(&params, "principal").unwrap_or("");
            registry
                .reject_permission(session_id, principal, request_id)
                .await?;
            to_json(json!({
                "ok": true,
                "session_id": session_id,
                "request_id": request_id,
            }))
        }

        "session.close" => {
            require_confirm(&params, "session.close")?;
            let session_id = require_str(&params, "session_id")?;
            let principal = opt_str(&params, "principal").unwrap_or("");
            registry.close_session(session_id, principal).await?;
            to_json(json!({ "ok": true, "session_id": session_id }))
        }

        "session.subscribe_ticket" => {
            let session_id = require_str(&params, "session_id")?;
            let principal = opt_str(&params, "principal").unwrap_or("");
            registry.check_session_access(session_id, principal).await?;
            let ticket = issue_subscribe_ticket(session_id, principal)?;
            to_json(json!({
                "ticket": ticket,
                "expires_in_secs": TICKET_TTL_SECS,
            }))
        }

        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `{unknown}` for service `acp`"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

// ── SSE ticket ────────────────────────────────────────────────────────────────

/// Issue a short-lived HMAC-signed ticket for browser EventSource SSE auth.
///
/// Ticket format (URL-safe base64 of): `{session_id}:{principal}:{exp}:{hmac_hex}`
///
/// Uses the same `LAB_ACP_HMAC_SECRET` as permission outcome signing.
/// Falls back to a process-ephemeral key if the env var is not set.
fn issue_subscribe_ticket(session_id: &str, principal: &str) -> Result<String, ToolError> {
    let key = load_hmac_key();
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + TICKET_TTL_SECS;

    let payload = format!("{session_id}:{principal}:{exp}");

    let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("HMAC key error: {e}"),
    })?;
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_hex = sig.iter().map(|b| format!("{b:02x}")).collect::<String>();

    let ticket = format!("{payload}:{sig_hex}");
    Ok(B64.encode(ticket.as_bytes()))
}

/// Validate an SSE ticket. Returns `(session_id, principal)` on success.
pub fn validate_subscribe_ticket(ticket: &str) -> Result<(String, String), ToolError> {
    let raw = B64.decode(ticket).map_err(|_| ToolError::Sdk {
        sdk_kind: "auth_failed".to_string(),
        message: "invalid ticket encoding".to_string(),
    })?;
    let raw_str = std::str::from_utf8(&raw).map_err(|_| ToolError::Sdk {
        sdk_kind: "auth_failed".to_string(),
        message: "invalid ticket encoding".to_string(),
    })?;

    // Format: session_id:principal:exp:sig_hex
    let parts: Vec<&str> = raw_str.splitn(4, ':').collect();
    if parts.len() != 4 {
        return Err(ToolError::Sdk {
            sdk_kind: "auth_failed".to_string(),
            message: "malformed ticket".to_string(),
        });
    }
    let (session_id, principal, exp_str, sig_hex) = (parts[0], parts[1], parts[2], parts[3]);

    let exp: u64 = exp_str.parse().unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now > exp {
        return Err(ToolError::Sdk {
            sdk_kind: "auth_failed".to_string(),
            message: "ticket expired".to_string(),
        });
    }

    let key = load_hmac_key();
    let payload = format!("{session_id}:{principal}:{exp_str}");
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|_| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "HMAC key error".to_string(),
    })?;
    mac.update(payload.as_bytes());

    let expected = mac.finalize().into_bytes();
    let expected_hex: String = expected.iter().map(|b| format!("{b:02x}")).collect();
    if expected_hex != sig_hex {
        return Err(ToolError::Sdk {
            sdk_kind: "auth_failed".to_string(),
            message: "invalid ticket signature".to_string(),
        });
    }

    Ok((session_id.to_string(), principal.to_string()))
}

fn load_hmac_key() -> &'static [u8] {
    use std::sync::OnceLock;
    static KEY: OnceLock<Vec<u8>> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Ok(secret) = std::env::var("LAB_ACP_HMAC_SECRET") {
            if !secret.is_empty() {
                return secret.into_bytes();
            }
        }
        // Fall back to process-ephemeral key — stable for the process lifetime.
        use sha2::{Digest, Sha256};
        let pid = std::process::id();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Sha256::digest(format!("lab-acp-hmac-ephemeral:{pid}:{now}").as_bytes()).to_vec()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::acp::registry::AcpSessionRegistry;
    use serde_json::json;

    #[tokio::test]
    async fn help_returns_catalog_object() {
        let v = dispatch("help", json!({})).await.unwrap();
        assert!(v.is_object());
        assert_eq!(v["service"], "acp");
    }

    #[tokio::test]
    async fn schema_returns_action_shape() {
        let v = dispatch("schema", json!({"action": "session.start"}))
            .await
            .unwrap();
        assert!(v.is_object());
    }

    #[tokio::test]
    async fn unknown_action_returns_kind() {
        let e = dispatch("session.serch", json!({})).await.unwrap_err();
        assert_eq!(e.kind(), "unknown_action");
    }

    #[tokio::test]
    async fn session_prompt_rejects_oversized_prompt() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        registry
            .inject_fake_session("sess-over-limit", "alice")
            .await;
        let prompt = "x".repeat(MAX_ACP_PROMPT_BYTES + 1);

        let err = dispatch_with_registry(
            &registry,
            "session.prompt",
            json!({
                "session_id": "sess-over-limit",
                "principal": "alice",
                "text": prompt,
            }),
        )
        .await
        .expect_err("oversized prompt must be rejected");

        assert_eq!(err.kind(), "content_too_large");
    }

    #[tokio::test]
    async fn session_prompt_rejects_oversized_params() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        let oversized = "x".repeat(MAX_ACP_PARAMS_BYTES + 1);

        let err = dispatch_with_registry(
            &registry,
            "session.list",
            json!({
                "principal": "alice",
                "padding": oversized,
            }),
        )
        .await
        .expect_err("oversized params must be rejected");

        assert_eq!(err.kind(), "content_too_large");
    }

    #[tokio::test]
    async fn session_prompt_rejects_oversized_prompt_params() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        let oversized = "x".repeat(MAX_ACP_PROMPT_PARAMS_BYTES + 1);

        let err = dispatch_with_registry(
            &registry,
            "session.prompt",
            json!({
                "session_id": "sess-over-limit",
                "principal": "alice",
                "text": "hello",
                "padding": oversized,
            }),
        )
        .await
        .expect_err("oversized session.prompt params must be rejected");

        assert_eq!(err.kind(), "content_too_large");
    }

    #[tokio::test]
    async fn session_prompt_dispatches_normal_prompt() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        registry.inject_fake_session("sess-normal", "alice").await;

        let value = dispatch_with_registry(
            &registry,
            "session.prompt",
            json!({
                "session_id": "sess-normal",
                "principal": "alice",
                "text": "hello",
            }),
        )
        .await
        .expect("normal prompt dispatch");

        assert_eq!(value["ok"], true);
        assert_eq!(value["session_id"], "sess-normal");
    }

    #[test]
    fn subscribe_ticket_round_trip() {
        let ticket = issue_subscribe_ticket("sess-123", "user@example.com").unwrap();
        let (session_id, principal) = validate_subscribe_ticket(&ticket).unwrap();
        assert_eq!(session_id, "sess-123");
        assert_eq!(principal, "user@example.com");
    }

    #[test]
    fn subscribe_ticket_empty_principal() {
        let ticket = issue_subscribe_ticket("sess-456", "").unwrap();
        let (session_id, principal) = validate_subscribe_ticket(&ticket).unwrap();
        assert_eq!(session_id, "sess-456");
        assert_eq!(principal, "");
    }
}
