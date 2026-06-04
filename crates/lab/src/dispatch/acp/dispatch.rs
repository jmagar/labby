//! Top-level action router for the `acp` dispatch service.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use hex;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::acp::registry::PromptSessionOptions;
use crate::acp::types::StartSessionInput;
use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, to_json};

use super::catalog::ACTIONS;
use super::client::require_registry;
use super::page_context::{PageContextInput, build_prompt_with_context};
use super::params::{
    BulkCloseSelector, LocalPromptAttachment, opt_str, opt_u64, require_str,
    validate_local_attachments,
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
    use std::io::Write;
    struct ByteCounter(usize);
    impl Write for ByteCounter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = ByteCounter(0);
    let size = if serde_json::to_writer(&mut counter, params).is_ok() {
        counter.0
    } else {
        usize::MAX
    };
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

fn try_meta_action(action: &str, params: &Value) -> Option<Result<Value, ToolError>> {
    match action {
        "help" => Some(Ok(help_payload("acp", ACTIONS))),
        "schema" => Some(require_str(params, "action").and_then(|a| action_schema(ACTIONS, a))),
        _ => None,
    }
}

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" | "schema" => try_meta_action(action, &params).unwrap(),
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
    let max_params_bytes = if action == "session.prompt" || action == "session.start_and_prompt" {
        MAX_ACP_PROMPT_PARAMS_BYTES
    } else {
        MAX_ACP_PARAMS_BYTES
    };
    ensure_params_size(&params, max_params_bytes)?;

    if let Some(result) = try_meta_action(action, &params) {
        return result;
    }

    match action {
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
                        "models": health.models,
                        "defaultModelId": health.default_model_id,
                        "currentModelId": health.current_model_id,
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
                "models": health.models,
                "defaultModelId": health.default_model_id,
                "currentModelId": health.current_model_id,
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
            let principal = opt_str(&params, "principal").unwrap_or("");
            registry.check_session_access(session_id, principal).await?;
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
            let model_id = opt_str(&params, "model")
                .or_else(|| opt_str(&params, "model_id"))
                .map(str::to_string);

            let input = StartSessionInput {
                provider,
                title,
                cwd,
                principal: if principal.is_empty() {
                    None
                } else {
                    Some(principal.to_string())
                },
                model_id,
            };
            let summary = registry.create_session(input, principal).await?;
            to_json(summary)
        }

        "session.start_and_prompt" => {
            let principal = require_str(&params, "principal")?;
            let provider = opt_str(&params, "provider").map(|s| s.to_string());
            let title = opt_str(&params, "title").map(|s| s.to_string());
            let cwd = opt_str(&params, "cwd").unwrap_or("").to_string();
            let model_id = opt_str(&params, "model")
                .or_else(|| opt_str(&params, "model_id"))
                .map(str::to_string);
            let raw_text = params.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            if raw_text.is_empty() {
                return Err(ToolError::MissingParam {
                    message: "prompt is required".to_string(),
                    param: "prompt".to_string(),
                });
            }
            ensure_prompt_size(raw_text)?;

            let page_ctx = params
                .get("page_context")
                .and_then(|v| v.as_object())
                .map(|obj| PageContextInput {
                    route: obj.get("route").and_then(|v| v.as_str()).unwrap_or(""),
                    entity_type: obj.get("entityType").and_then(|v| v.as_str()),
                    entity_id: obj.get("entityId").and_then(|v| v.as_str()),
                });
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

            let input = StartSessionInput {
                provider: provider.clone(),
                title,
                cwd,
                principal: Some(principal.to_string()),
                model_id,
            };
            // Synthesize the effective prompt with page-context prefix, just
            // like `session.prompt` does.
            let placeholder_session_id = ""; // session does not exist yet; helper tolerates empty.
            let effective_text =
                build_prompt_with_context(placeholder_session_id, raw_text, page_ctx.as_ref());
            ensure_prompt_size(&effective_text)?;

            let result = registry
                .start_and_prompt(
                    input,
                    &effective_text,
                    attachments,
                    principal,
                    PromptSessionOptions {
                        provider,
                        continuity_mode: opt_str(&params, "continuity_mode").map(ToOwned::to_owned),
                    },
                )
                .await?;
            // Issue the SSE ticket so the client can subscribe to the stream
            // without a second action call.
            let ticket = issue_subscribe_ticket(&result.session_id, principal)?;
            let mut value = to_json(json!(result))?;
            if let Some(map) = value.as_object_mut() {
                map.insert("stream_ticket".to_string(), Value::String(ticket));
                map.insert(
                    "ticket_expires_in_secs".to_string(),
                    Value::Number(TICKET_TTL_SECS.into()),
                );
            }
            Ok(value)
        }

        "session.prompt" => {
            let session_id = require_str(&params, "session_id")?;
            let principal = opt_str(&params, "principal").unwrap_or("");
            let raw_text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
            // Allow attachments-only prompts: the surface adapter already
            // verified that at least one of text/attachments was supplied.
            // We must still cap the size if text is present.
            if !raw_text.is_empty() {
                ensure_prompt_size(raw_text)?;
            }

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
            let model_id = opt_str(&params, "model").or_else(|| opt_str(&params, "model_id"));

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
                    model_id,
                    PromptSessionOptions {
                        provider: opt_str(&params, "provider").map(ToOwned::to_owned),
                        continuity_mode: opt_str(&params, "continuity_mode").map(ToOwned::to_owned),
                    },
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

        "session.bulk_close" => {
            require_confirm(&params, "session.bulk_close")?;
            let selector_value =
                params
                    .get("selector")
                    .cloned()
                    .ok_or_else(|| ToolError::MissingParam {
                        message: "selector is required".to_string(),
                        param: "selector".to_string(),
                    })?;
            let selector: BulkCloseSelector =
                serde_json::from_value(selector_value).map_err(|error| {
                    ToolError::InvalidParam {
                        message: format!("invalid selector: {error}"),
                        param: "selector".to_string(),
                    }
                })?;
            selector.validate_non_empty()?;
            let principal = require_str(&params, "principal")?;
            let result = registry.bulk_close_sessions(selector, principal).await?;
            to_json(json!(result))
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
/// Ticket format (URL-safe base64 of): `{session_id}:{principal}:{exp}:{sig_hex}`
///
/// Shares the same key as permission-outcome signing via `persistence::acp_hmac_key`.
fn issue_subscribe_ticket(session_id: &str, principal: &str) -> Result<String, ToolError> {
    let key = super::persistence::acp_hmac_key();
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + TICKET_TTL_SECS;

    let payload = format!("{session_id}:{principal}:{exp}");

    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("HMAC key error: {e}"),
    })?;
    mac.update(payload.as_bytes());
    let sig_hex = hex::encode(mac.finalize().into_bytes());

    let ticket = format!("{payload}:{sig_hex}");
    Ok(B64.encode(ticket.as_bytes()))
}

/// Validate an SSE ticket. Returns `(session_id, principal)` on success.
pub fn validate_subscribe_ticket(ticket: &str) -> Result<(String, String), ToolError> {
    let auth_err = || ToolError::Sdk {
        sdk_kind: "auth_failed".to_string(),
        message: "invalid ticket".to_string(),
    };

    let raw = B64.decode(ticket).map_err(|_| auth_err())?;
    let raw_str = std::str::from_utf8(&raw).map_err(|_| auth_err())?;

    // Format: session_id:principal:exp:sig_hex
    let parts: Vec<&str> = raw_str.splitn(4, ':').collect();
    if parts.len() != 4 {
        return Err(auth_err());
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

    let key = super::persistence::acp_hmac_key();
    let payload = format!("{session_id}:{principal}:{exp_str}");
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "HMAC key error".to_string(),
    })?;
    mac.update(payload.as_bytes());

    // Decode the provided hex sig and verify constant-time.
    let sig_bytes = hex::decode(sig_hex).map_err(|_| auth_err())?;
    mac.verify_slice(&sig_bytes).map_err(|_| auth_err())?;

    Ok((session_id.to_string(), principal.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::acp::registry::AcpSessionRegistry;
    use lab_apis::acp::types::AcpSessionState;
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

    #[tokio::test]
    async fn session_start_and_prompt_requires_principal() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        let err = dispatch_with_registry(
            &registry,
            "session.start_and_prompt",
            json!({
                "prompt": "hello",
            }),
        )
        .await
        .expect_err("principal must be required");
        assert_eq!(err.kind(), "missing_param");
    }

    #[tokio::test]
    async fn session_start_and_prompt_requires_prompt_text() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        let err = dispatch_with_registry(
            &registry,
            "session.start_and_prompt",
            json!({
                "principal": "alice",
            }),
        )
        .await
        .expect_err("prompt must be required");
        assert_eq!(err.kind(), "missing_param");
    }

    #[tokio::test]
    async fn session_start_and_prompt_rejects_oversized_prompt() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        let huge = "x".repeat(MAX_ACP_PROMPT_BYTES + 1);
        let err = dispatch_with_registry(
            &registry,
            "session.start_and_prompt",
            json!({
                "principal": "alice",
                "prompt": huge,
            }),
        )
        .await
        .expect_err("oversized prompt must be rejected");
        assert_eq!(err.kind(), "content_too_large");
    }

    #[tokio::test]
    async fn session_start_and_prompt_appears_in_schema() {
        let v = dispatch("schema", json!({"action": "session.start_and_prompt"}))
            .await
            .unwrap();
        assert!(v.is_object());
        // Schema entry must surface the canonical params so clients can wire UIs.
        let params = v["params"].as_array().expect("params array");
        let names: Vec<&str> = params.iter().filter_map(|p| p["name"].as_str()).collect();
        assert!(
            names.contains(&"prompt"),
            "schema missing prompt: {names:?}"
        );
        assert!(
            names.contains(&"principal"),
            "schema missing principal: {names:?}"
        );
    }

    #[tokio::test]
    async fn session_bulk_close_rejects_empty_selector() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        let err = dispatch_with_registry(
            &registry,
            "session.bulk_close",
            json!({
                "selector": {},
                "principal": "alice",
                "confirm": true,
            }),
        )
        .await
        .expect_err("empty selector must be rejected");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[tokio::test]
    async fn session_bulk_close_requires_confirm() {
        // Gate-drift guard: the dispatcher arm itself must enforce require_confirm
        // so a surface bypassing destructive-elicitation cannot fall through.
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        let err = dispatch_with_registry(
            &registry,
            "session.bulk_close",
            json!({
                "selector": { "states": ["failed"], "max_count": 100 },
                "principal": "alice",
                // INTENTIONALLY no "confirm": true
            }),
        )
        .await
        .expect_err("missing confirm must be rejected");
        assert_eq!(err.kind(), "confirmation_required");
    }

    #[tokio::test]
    async fn session_bulk_close_only_touches_caller_principal() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        registry.inject_fake_session("sess-alice", "alice").await;
        registry.inject_fake_session("sess-bob", "bob").await;
        // Flip both into Failed so a Failed-only selector would match either.
        registry
            .force_summary_state_for_tests("sess-alice", AcpSessionState::Failed)
            .await;
        registry
            .force_summary_state_for_tests("sess-bob", AcpSessionState::Failed)
            .await;

        let value = dispatch_with_registry(
            &registry,
            "session.bulk_close",
            json!({
                "selector": { "states": ["failed"], "max_count": 100 },
                "principal": "alice",
                "confirm": true,
            }),
        )
        .await
        .expect("bulk_close should succeed");

        let closed = value["closed"].as_array().expect("closed array");
        assert_eq!(closed.len(), 1, "should close exactly one session");
        assert_eq!(closed[0], "sess-alice");
        // Bob's session must still be registered.
        assert!(
            registry.session_exists_for_tests("sess-bob").await,
            "other-principal session must remain untouched",
        );
    }

    // ── Principal isolation (IDOR) ────────────────────────────────────────────

    #[tokio::test]
    async fn session_get_rejects_wrong_principal() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        registry.inject_fake_session("idor-sess", "alice").await;
        let err = dispatch_with_registry(
            &registry,
            "session.get",
            json!({ "session_id": "idor-sess", "principal": "bob" }),
        )
        .await
        .expect_err("wrong principal must be rejected");
        assert_eq!(err.kind(), "not_found", "IDOR must mask as not_found");
    }

    #[tokio::test]
    async fn session_events_rejects_wrong_principal() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        registry.inject_fake_session("idor-events", "alice").await;
        let err = dispatch_with_registry(
            &registry,
            "session.events",
            json!({ "session_id": "idor-events", "principal": "bob" }),
        )
        .await
        .expect_err("wrong principal must be rejected");
        assert_eq!(err.kind(), "not_found", "IDOR must mask as not_found");
    }

    #[tokio::test]
    async fn session_prompt_rejects_wrong_principal() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        registry.inject_fake_session("idor-prompt", "alice").await;
        let err = dispatch_with_registry(
            &registry,
            "session.prompt",
            json!({ "session_id": "idor-prompt", "principal": "bob", "text": "hi" }),
        )
        .await
        .expect_err("wrong principal must be rejected");
        assert_eq!(err.kind(), "not_found", "IDOR must mask as not_found");
    }

    #[tokio::test]
    async fn session_cancel_rejects_wrong_principal() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        registry.inject_fake_session("idor-cancel", "alice").await;
        let err = dispatch_with_registry(
            &registry,
            "session.cancel",
            json!({ "session_id": "idor-cancel", "principal": "bob", "confirm": true }),
        )
        .await
        .expect_err("wrong principal must be rejected");
        assert_eq!(err.kind(), "not_found", "IDOR must mask as not_found");
    }

    #[tokio::test]
    async fn subscribe_ticket_rejects_wrong_principal() {
        let registry = AcpSessionRegistry::new_for_tests(Duration::from_millis(100));
        registry.inject_fake_session("idor-ticket", "alice").await;
        let err = dispatch_with_registry(
            &registry,
            "session.subscribe_ticket",
            json!({ "session_id": "idor-ticket", "principal": "bob" }),
        )
        .await
        .expect_err("wrong principal must be rejected");
        assert_eq!(err.kind(), "not_found", "IDOR must mask as not_found");
    }
}
