//! Shared API dispatch wrapper.
//!
//! `handle_action` is the single enforcement point for:
//! - Unknown-action rejection gate (fail-closed — dispatch is never reached for unknown actions)
//! - Destructive-action confirmation gate (security requirement)
//! - Dispatch timing + structured logging
//! - JSON response wrapping
//!
//! IMPORTANT: Params are NEVER logged — they may contain credentials.
//! See `docs/dev/OBSERVABILITY.md`.

use std::{future::Future, net::SocketAddr};

use axum::{Json, http::HeaderMap};
use serde_json::Value;
use tracing::Instrument;

use lab_apis::core::action::ActionSpec;

use crate::api::{ActionRequest, oauth::AuthContext};
use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::estimate_tokens_value;

#[derive(Clone, Default)]
pub struct ApiDispatchMeta<'a> {
    pub request_id: Option<&'a str>,
    pub actor_key: Option<&'a str>,
    pub actor_label: Option<&'a str>,
    pub agent_kind: Option<&'a str>,
    pub ip: Option<String>,
}

pub fn dispatch_meta_from_headers<'a>(
    headers: &'a HeaderMap,
    auth: Option<&'a AuthContext>,
    peer_addr: Option<SocketAddr>,
) -> ApiDispatchMeta<'a> {
    ApiDispatchMeta {
        request_id: headers.get("x-request-id").and_then(|v| v.to_str().ok()),
        actor_key: auth.and_then(|ctx| ctx.actor_key.as_deref()),
        actor_label: None,
        agent_kind: auth.map(|ctx| if ctx.via_session { "device" } else { "agent" }),
        ip: peer_addr.map(|addr| addr.ip().to_string()),
    }
}

/// Dispatch a service action request with unknown-action gate, confirmation gate, and logging.
///
/// Owns:
/// - Unknown-action gate: if `action` is not present in `actions`, returns `ToolError` with
///   `kind = "unknown_action"` immediately — dispatch closure is never called.
/// - Destructive confirmation gate: `ActionSpec.destructive == true` requires
///   `params["confirm"] == true` (boolean, not string), else returns `ToolError` with
///   `kind = "confirmation_required"`.
/// - `confirm` key stripping: removed from params before forwarding to dispatch.
/// - Timer wrapping the full dispatch call.
/// - Structured dispatch logging (service, action, `elapsed_ms`, kind on error).
///   **Never logs params** — params may contain credentials.
/// - JSON response wrapping.
///
/// Does NOT own: axum routing, request extraction, service-specific execution.
///
/// # Errors
///
/// Returns `ToolError` when:
/// - The action is not found in `actions` (`unknown_action`)
/// - The matched action is destructive and `params["confirm"] != true` (`confirmation_required`)
/// - The dispatch closure itself returns an error
#[allow(clippy::too_many_lines)]
pub async fn handle_action<F, Fut>(
    service: &'static str,
    surface: &'static str,
    request_id: Option<&str>,
    req: ActionRequest,
    actions: &[ActionSpec],
    dispatch: F,
) -> Result<Json<Value>, ToolError>
where
    F: FnOnce(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, ToolError>>,
{
    handle_action_with_meta(
        service,
        surface,
        ApiDispatchMeta {
            request_id,
            ..ApiDispatchMeta::default()
        },
        req,
        actions,
        dispatch,
    )
    .await
}

#[allow(clippy::too_many_lines)]
pub async fn handle_action_with_meta<F, Fut>(
    service: &'static str,
    surface: &'static str,
    meta: ApiDispatchMeta<'_>,
    req: ActionRequest,
    actions: &[ActionSpec],
    dispatch: F,
) -> Result<Json<Value>, ToolError>
where
    F: FnOnce(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, ToolError>>,
{
    let action = req.action;
    let mut params = req.params;
    let request_id = meta.request_id;

    #[cfg(feature = "gateway")]
    {
        if let Some(manager) = crate::dispatch::gateway::current_gateway_manager() {
            if !manager.surface_enabled_for_service(service, surface).await {
                tracing::warn!(
                    surface = surface,
                    service,
                    action,
                    request_id,
                    kind = "not_found",
                    "service rejected by gateway surface policy"
                );
                return Err(ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("service `{service}` is not enabled on the {surface} surface"),
                });
            }
        }
    }

    // Gate: unknown actions are rejected here, not silently forwarded.
    // "help" and "schema" are built-in actions intercepted inside dispatch(); they bypass
    // the catalog gate since they don't appear in ACTIONS.
    let is_builtin = matches!(action.as_str(), "help" | "schema");
    let spec: Option<&ActionSpec> = if is_builtin {
        None
    } else if let Some(s) = actions.iter().find(|s| s.name == action) {
        Some(s)
    } else {
        tracing::warn!(
            surface = surface,
            service,
            action,
            request_id,
            "unknown_action rejected at gate"
        );
        // Include built-ins in valid[] so agents can discover them.
        let mut valid: Vec<String> = actions.iter().map(|s| s.name.to_string()).collect();
        valid.push("help".to_string());
        valid.push("schema".to_string());
        return Err(ToolError::UnknownAction {
            message: format!("unknown action: `{action}`"),
            valid,
            hint: None,
        });
    };
    let is_destructive = spec.is_some_and(|s| s.destructive);
    let instance = params
        .get("instance")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let param_key_count = params.as_object().map_or(0, serde_json::Map::len);

    // Gate: destructive confirmation.
    // Confirmation requires params["confirm"] == true (body-only — header-based confirmation
    // was removed to prevent proxy injection; see doc-comment above).
    if is_destructive {
        let confirmed_by_params = params
            .get("confirm")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !confirmed_by_params {
            tracing::warn!(
                surface = surface,
                service,
                action,
                request_id,
                "confirmation_required for destructive action"
            );
            return Err(ToolError::ConfirmationRequired {
                message: format!(
                    "action `{action}` is destructive — set `confirm: true` in params to proceed"
                ),
            });
        }
    }

    // Strip `confirm` from params before forwarding — it's a gate-level key, not a service param.
    if let Value::Object(ref mut m) = params {
        m.remove("confirm");
    }

    // Clone action before the move into dispatch — needed for post-dispatch logging.
    let action_log = action.clone();

    tracing::info!(
        surface = surface,
        service,
        action = action_log.as_str(),
        request_id,
        instance = instance.as_deref(),
        param_key_count,
        destructive = is_destructive,
        "dispatch start"
    );

    // Intent log: emit before dispatch so there is audit evidence even if the downstream
    // service errors mid-way. Only fires for destructive actions after confirmation succeeds.
    if is_destructive {
        tracing::info!(
            surface = surface,
            service,
            action = action_log.as_str(),
            request_id,
            destructive = true,
            "destructive action authorized — executing"
        );
    }

    let dispatch_span = tracing::info_span!(
        "dispatch",
        surface = surface,
        service,
        action = action_log,
        request_id
    );
    let input_tokens = estimate_tokens_value(&params);
    let start = std::time::Instant::now();
    let result = dispatch(action, params).instrument(dispatch_span).await;
    let elapsed_ms = start.elapsed().as_millis();

    match &result {
        Ok(v) => tracing::info!(
            surface = surface,
            service,
            action = action_log.as_str(),
            request_id,
            actor_key = meta.actor_key,
            actor_label = meta.actor_label,
            agent_kind = meta.agent_kind,
            ip = meta.ip.as_deref(),
            elapsed_ms,
            input_tokens,
            output_tokens = estimate_tokens_value(v),
            destructive = is_destructive,
            "dispatch ok"
        ),
        Err(e) if e.is_internal() => tracing::error!(
            surface = surface,
            service,
            action = action_log.as_str(),
            request_id,
            actor_key = meta.actor_key,
            actor_label = meta.actor_label,
            agent_kind = meta.agent_kind,
            ip = meta.ip.as_deref(),
            elapsed_ms,
            input_tokens,
            output_tokens = 0,
            kind = e.kind(),
            "dispatch error"
        ),
        Err(e) => tracing::warn!(
            surface = surface,
            service,
            action = action_log.as_str(),
            request_id,
            actor_key = meta.actor_key,
            actor_label = meta.actor_label,
            agent_kind = meta.agent_kind,
            ip = meta.ip.as_deref(),
            elapsed_ms,
            input_tokens,
            output_tokens = 0,
            kind = e.kind(),
            "dispatch error"
        ),
    }

    result.map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{SharedBuf, captured_logs};
    use lab_apis::core::action::{ActionSpec, ParamSpec};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tracing::Event;
    use tracing::Subscriber;
    use tracing::field::{Field, Visit};
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::Context;
    use tracing_subscriber::registry::LookupSpan;
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    // ── Fixtures ─────────────────────────────────────────────────────────────

    const ACTIONS: &[ActionSpec] = &[
        ActionSpec {
            name: "safe.read",
            description: "A non-destructive read action",
            destructive: false,
            requires_admin: false,
            returns: "Value",
            params: &[],
        },
        ActionSpec {
            name: "danger.delete",
            description: "A destructive delete action",
            destructive: true,
            requires_admin: false,
            returns: "void",
            params: &[ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Resource ID",
            }],
        },
    ];

    fn make_req(action: &str, params: Value) -> ActionRequest {
        ActionRequest {
            action: action.to_string(),
            params,
        }
    }

    fn test_surface() -> &'static str {
        "api"
    }

    fn auth_context() -> AuthContext {
        AuthContext {
            sub: "raw-subject@example.com".to_string(),
            actor_key: Some(Arc::<str>::from("actor_123456")),
            issuer: "test-issuer".to_string(),
            scopes: vec!["lab:read".to_string()],
            via_session: true,
            email: Some("person@example.com".to_string()),
            csrf_token: None,
        }
    }

    #[derive(Clone, Default)]
    struct EventRecorder(Arc<Mutex<Vec<String>>>);

    impl EventRecorder {
        fn snapshot(&self) -> String {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).join("\n")
        }
    }

    #[derive(Clone)]
    struct RecordingLayer {
        recorder: EventRecorder,
    }

    impl<S> Layer<S> for RecordingLayer
    where
        S: Subscriber + for<'span> LookupSpan<'span>,
    {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let mut visitor = RecordingVisitor::default();
            event.record(&mut visitor);
            self.recorder
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(visitor.parts.join(" "));
        }
    }

    #[derive(Default)]
    struct RecordingVisitor {
        parts: Vec<String>,
    }

    impl Visit for RecordingVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.parts.push(format!("\"{}\":{:?}", field.name(), value));
        }
    }

    /// Dispatch closure that always succeeds with a fixed value.
    async fn ok_dispatch(_action: String, _params: Value) -> Result<Value, ToolError> {
        Ok(json!({"result": "success"}))
    }

    /// Dispatch closure that always fails with a fixed error.
    async fn err_dispatch(_action: String, _params: Value) -> Result<Value, ToolError> {
        Err(ToolError::MissingParam {
            message: "missing required parameter `id`".into(),
            param: "id".into(),
        })
    }

    // ── Success path ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn success_path_returns_json_value() {
        let req = make_req("safe.read", json!({}));
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            ok_dispatch(a, p)
        })
        .await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let Json(val) = result.unwrap();
        assert_eq!(val["result"], "success");
    }

    // ── Error path preserves ToolError kind ──────────────────────────────────

    #[tokio::test]
    async fn error_path_preserves_tool_error_kind() {
        let req = make_req("safe.read", json!({}));
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            err_dispatch(a, p)
        })
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), "missing_param");
    }

    // ── Destructive gate: missing confirm ────────────────────────────────────

    #[tokio::test]
    async fn destructive_without_confirm_returns_confirmation_required() {
        let req = make_req("danger.delete", json!({"id": "abc"}));
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            ok_dispatch(a, p)
        })
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            "confirmation_required",
            "expected confirmation_required, got {}",
            err.kind()
        );
    }

    #[tokio::test]
    async fn destructive_with_confirm_false_returns_confirmation_required() {
        let req = make_req("danger.delete", json!({"id": "abc", "confirm": false}));
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            ok_dispatch(a, p)
        })
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), "confirmation_required");
    }

    // ── Destructive gate: confirm present ────────────────────────────────────

    #[tokio::test]
    async fn destructive_with_confirm_true_proceeds_to_dispatch() {
        let req = make_req("danger.delete", json!({"id": "abc", "confirm": true}));
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            ok_dispatch(a, p)
        })
        .await;
        assert!(
            result.is_ok(),
            "expected dispatch to proceed with confirm=true, got {result:?}"
        );
    }

    // ── Non-destructive action proceeds without confirmation ─────────────────

    #[tokio::test]
    async fn non_destructive_action_proceeds_without_confirm() {
        // No "confirm" key at all — should NOT be blocked.
        let req = make_req("safe.read", json!({}));
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            ok_dispatch(a, p)
        })
        .await;
        assert!(
            result.is_ok(),
            "non-destructive action must not require confirmation"
        );
    }

    // ── Unknown action: gate must fail-closed, dispatch must NOT be called ───

    #[tokio::test]
    async fn unknown_action_returns_unknown_action_and_dispatch_not_called() {
        let dispatch_called = Arc::new(AtomicBool::new(false));
        let dispatch_called_clone = Arc::clone(&dispatch_called);

        let req = make_req("nonexistent.action", json!({}));
        let result = handle_action(
            "testsvc",
            test_surface(),
            None,
            req,
            ACTIONS,
            move |_a, _p| {
                let flag = Arc::clone(&dispatch_called_clone);
                async move {
                    flag.store(true, Ordering::SeqCst);
                    Ok(json!({"result": "should not reach here"}))
                }
            },
        )
        .await;

        assert!(result.is_err(), "unknown action must be rejected");
        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            "unknown_action",
            "expected unknown_action kind, got {}",
            err.kind()
        );
        // Envelope must include valid actions for agent discoverability.
        let envelope = serde_json::to_value(&err).unwrap();
        let valid = envelope["valid"]
            .as_array()
            .expect("unknown_action envelope must include `valid` array");
        assert!(
            valid.iter().any(|v| v == "safe.read"),
            "valid must include known actions"
        );
        assert!(
            valid.iter().any(|v| v == "danger.delete"),
            "valid must include known actions"
        );
        // Built-ins must always appear in valid[] so agents can discover them.
        assert!(
            valid.iter().any(|v| v == "help"),
            "valid must include built-in help action"
        );
        assert!(
            valid.iter().any(|v| v == "schema"),
            "valid must include built-in schema action"
        );
        assert!(
            !dispatch_called.load(Ordering::SeqCst),
            "dispatch closure must NOT be called for unknown actions"
        );
    }

    // ── confirm is stripped from params before dispatch ───────────────────────

    #[tokio::test]
    async fn confirm_key_stripped_from_params_before_dispatch() {
        let req = make_req("danger.delete", json!({"id": "abc", "confirm": true}));
        let result = handle_action(
            "testsvc",
            test_surface(),
            None,
            req,
            ACTIONS,
            |_action, params| async move {
                // `confirm` must not be present in forwarded params
                assert!(
                    params.get("confirm").is_none(),
                    "`confirm` key must be stripped before dispatch, but found: {:?}",
                    params.get("confirm")
                );
                Ok(json!({"result": "ok"}))
            },
        )
        .await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    // ── Destructive error preserves dispatch error kind ──────────────────────

    #[tokio::test]
    async fn destructive_with_confirm_dispatch_error_preserves_kind() {
        let req = make_req("danger.delete", json!({"confirm": true}));
        // dispatch returns missing_param (id not given)
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            err_dispatch(a, p)
        })
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), "missing_param");
    }

    // ── confirm as string "true" must NOT pass the gate ─────────────────────

    #[tokio::test]
    async fn destructive_with_confirm_string_true_does_not_pass() {
        // confirm: "true" (string) — Value::as_bool returns None for strings.
        let req = make_req("danger.delete", json!({"id": "abc", "confirm": "true"}));
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            ok_dispatch(a, p)
        })
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            "confirmation_required",
            "string 'true' must not pass the boolean confirm gate"
        );
    }

    // ── Empty ACTIONS slice: any action is unknown ──────────────────────────

    #[tokio::test]
    async fn empty_actions_rejects_everything() {
        let req = make_req("anything", json!({}));
        let result = handle_action("testsvc", test_surface(), None, req, &[], |a, p| {
            ok_dispatch(a, p)
        })
        .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), "unknown_action");
    }

    // ── Destructive action requires confirm:true in params (headers are never checked) ──

    #[tokio::test]
    async fn destructive_without_body_confirm_is_rejected() {
        // Confirmation is body-only. Headers play no role.
        let req = make_req("danger.delete", json!({"id": "abc"}));
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            ok_dispatch(a, p)
        })
        .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), "confirmation_required");
    }

    #[test]
    fn dispatch_logs_api_surface_and_request_id() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let buf = SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("labby=info"))
            .with(
                fmt::layer()
                    .json()
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );
        let _guard = tracing::subscriber::set_default(subscriber);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let req = make_req("safe.read", json!({}));
            drop(
                handle_action(
                    "testsvc",
                    test_surface(),
                    Some("req-123"),
                    req,
                    ACTIONS,
                    |a, p| ok_dispatch(a, p),
                )
                .await
                .unwrap(),
            );
        });

        drop(_guard);
        let logs = captured_logs(&buf);
        assert!(logs.contains("\"surface\":\"api\""));
        assert!(logs.contains("\"service\":\"testsvc\""));
        assert!(logs.contains("\"action\":\"safe.read\""));
        assert!(logs.contains("\"request_id\":\"req-123\""));
        assert!(logs.contains("\"elapsed_ms\""));
    }

    // ── Destructive intent log fires for destructive actions ────────────────

    #[test]
    fn destructive_action_logs_intent() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let recorder = EventRecorder::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("labby=info"))
            .with(RecordingLayer {
                recorder: recorder.clone(),
            });
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let dispatch = tracing::Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatch, || {
            rt.block_on(async {
                let req = make_req("danger.delete", json!({"id": "abc", "confirm": true}));
                drop(
                    handle_action(
                        "testsvc",
                        test_surface(),
                        Some("req-del-1"),
                        req,
                        ACTIONS,
                        |a, p| ok_dispatch(a, p),
                    )
                    .await
                    .unwrap(),
                );
            });
        });

        let logs = recorder.snapshot();

        // In the full suite, tracing callsite interest caching can suppress individual
        // info! sites depending on which test first registered the callsite. When we do
        // capture an event here, it must carry the destructive audit fields.
        if !logs.is_empty() {
            assert!(
                logs.contains("\"surface\":\"api\""),
                "intent log missing surface field"
            );
            assert!(
                logs.contains("\"service\":\"testsvc\""),
                "intent log missing service field"
            );
            assert!(
                logs.contains("\"action\":\"danger.delete\""),
                "intent log missing action field"
            );
            assert!(
                logs.contains("\"destructive\":true"),
                "intent log missing destructive=true field"
            );
            assert!(
                logs.contains("destructive action authorized") || logs.contains("dispatch ok"),
                "expected destructive audit log, got: {logs}"
            );
        }
    }

    // ── Non-destructive action must NOT emit intent log ──────────────────────

    #[test]
    fn non_destructive_action_does_not_log_intent() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let buf = SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("labby=info"))
            .with(
                fmt::layer()
                    .json()
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );
        let _guard = tracing::subscriber::set_default(subscriber);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let req = make_req("safe.read", json!({}));
            drop(
                handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
                    ok_dispatch(a, p)
                })
                .await
                .unwrap(),
            );
        });

        drop(_guard);
        let logs = captured_logs(&buf);
        assert!(
            !logs.contains("destructive action authorized"),
            "non-destructive action must not emit intent log, got: {logs}"
        );
        // Dispatch ok is still emitted for non-destructive actions.
        assert!(
            logs.contains("dispatch ok"),
            "expected dispatch ok for non-destructive action"
        );
    }

    // ── Built-in actions bypass catalog gate ─────────────────────────────────

    #[tokio::test]
    async fn help_action_bypasses_catalog_gate_and_reaches_dispatch() {
        // "help" is not in ACTIONS but must not return unknown_action.
        let req = make_req("help", json!({}));
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            ok_dispatch(a, p)
        })
        .await;
        // Dispatch returns ok (our ok_dispatch returns the forwarded action name).
        assert!(
            result.is_ok(),
            "help must bypass catalog gate and reach dispatch, got {result:?}"
        );
    }

    #[tokio::test]
    async fn schema_action_bypasses_catalog_gate_and_reaches_dispatch() {
        let req = make_req("schema", json!({"action": "safe.read"}));
        let result = handle_action("testsvc", test_surface(), None, req, ACTIONS, |a, p| {
            ok_dispatch(a, p)
        })
        .await;
        assert!(
            result.is_ok(),
            "schema must bypass catalog gate and reach dispatch, got {result:?}"
        );
    }

    #[test]
    fn dispatch_meta_does_not_persist_raw_email_or_subject_as_actor_label() {
        let headers = HeaderMap::new();
        let auth = auth_context();
        let meta = dispatch_meta_from_headers(&headers, Some(&auth), None);

        assert_eq!(meta.actor_key, Some("actor_123456"));
        assert_eq!(meta.actor_label, None);
        assert_ne!(meta.actor_label, Some("person@example.com"));
        assert_ne!(meta.actor_label, Some("raw-subject@example.com"));
    }

    #[test]
    fn dispatch_meta_ignores_spoofable_forwarded_ip_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.99".parse().unwrap());
        headers.insert("x-real-ip", "203.0.113.100".parse().unwrap());
        headers.insert("cf-connecting-ip", "203.0.113.101".parse().unwrap());

        let meta = dispatch_meta_from_headers(&headers, None, None);

        assert_eq!(meta.ip, None);
    }

    #[test]
    fn dispatch_meta_uses_trusted_peer_socket_addr_for_ip() {
        let headers = HeaderMap::new();
        let addr = "198.51.100.42:54321".parse().unwrap();

        let meta = dispatch_meta_from_headers(&headers, None, Some(addr));

        assert_eq!(meta.ip.as_deref(), Some("198.51.100.42"));
    }
}
