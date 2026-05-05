use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    body::{self, Body},
    http::{Request, StatusCode, header},
};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};
use tower::ServiceExt;

use labby::acp::registry::AcpSessionRegistry;
use labby::acp::runtime::set_codex_launch_override_for_tests;
use labby::api::{router::build_router_with_bearer, state::AppState};
use labby::dispatch::acp::dispatch::{dispatch_with_registry, validate_subscribe_ticket};

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

struct LaunchGuard;

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        set_codex_launch_override_for_tests(None, Vec::new());
    }
}

fn choose_python_command() -> String {
    for candidate in ["python3", "python"] {
        if Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return candidate.to_string();
        }
    }
    panic!("python3 or python is required for ACP contract tests");
}

fn write_fake_provider_script() -> PathBuf {
    let root = std::env::current_dir().expect("workspace cwd");
    let dir = root.join("target/test-artifacts");
    std::fs::create_dir_all(&dir).expect("create test artifact dir");

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = dir.join(format!("acp-fake-provider-{unique}.py"));

    std::fs::write(
        &path,
        r#"import json
import os
import sys

session_id = f"stub-session-{os.getpid()}"
capture_path = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("LAB_ACP_FAKE_PROMPT_CAPTURE")
if capture_path:
    open(capture_path, "a", encoding="utf-8").close()

for raw in sys.stdin:
    raw = raw.strip()
    if not raw:
        continue

    msg = json.loads(raw)
    req_id = msg.get("id")
    method = msg.get("method")
    params = msg.get("params") or {}

    if method == "initialize":
        result = {
            "protocolVersion": params.get("protocolVersion", "v1"),
            "agentCapabilities": {},
            "authMethods": [],
            "agentInfo": {
                "name": "stub-acp",
                "version": "0.0.0",
                "title": "Stub ACP",
            },
        }
        print(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": result}), flush=True)
    elif method == "session/new":
        result = {"sessionId": session_id, "configOptions": []}
        print(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": result}), flush=True)
    elif method == "session/prompt":
        if capture_path:
            with open(capture_path, "a", encoding="utf-8") as out:
                out.write(json.dumps(params.get("prompt", [])) + "\n")
        print(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": {}}), flush=True)
    elif req_id is not None:
        error = {"code": -32601, "message": "method not found"}
        print(json.dumps({"jsonrpc": "2.0", "id": req_id, "error": error}), flush=True)
"#,
    )
    .expect("write fake provider script");

    path
}

fn install_fake_provider() -> LaunchGuard {
    let script_path = write_fake_provider_script();
    let python = choose_python_command();
    set_codex_launch_override_for_tests(
        Some(python),
        vec!["-u".to_string(), script_path.display().to_string()],
    );
    LaunchGuard
}

fn install_fake_provider_with_prompt_capture(capture: &std::path::Path) -> LaunchGuard {
    let script_path = write_fake_provider_script();
    let python = choose_python_command();
    set_codex_launch_override_for_tests(
        Some(python),
        vec![
            "-u".to_string(),
            script_path.display().to_string(),
            capture.display().to_string(),
        ],
    );
    LaunchGuard
}

fn prompt_capture_path(name: &str) -> PathBuf {
    let root = std::env::current_dir().expect("workspace cwd");
    let dir = root.join("target/test-artifacts");
    std::fs::create_dir_all(&dir).expect("create test artifact dir");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    dir.join(format!("{name}-{unique}.jsonl"))
}

async fn create_owned_session(registry: &AcpSessionRegistry, principal: &str) -> String {
    let cwd = std::env::current_dir().expect("cwd");
    let value = dispatch_with_registry(
        registry,
        "session.start",
        json!({
            "cwd": cwd.display().to_string(),
            "title": "ACP contract test session",
            "principal": principal,
        }),
    )
    .await
    .expect("session.start");

    value["id"].as_str().expect("session id").to_string()
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response bytes");
    serde_json::from_slice(&bytes).expect("json body")
}

async fn response_body_text(response: axum::response::Response) -> (StatusCode, String) {
    let status = response.status();
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response bytes");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn read_prompt_capture(path: &std::path::Path) -> Vec<Value> {
    for _ in 0..100 {
        if path.metadata().map(|meta| meta.len() > 0).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let raw = std::fs::read_to_string(path).expect("read prompt capture");
    raw.lines()
        .map(|line| serde_json::from_str(line).expect("captured prompt json"))
        .collect()
}

fn acp_test_app() -> (Router, Arc<AcpSessionRegistry>) {
    let state = AppState::from_registry(labby::registry::ToolRegistry::new());
    let registry = Arc::clone(&state.acp_registry);
    let app = build_router_with_bearer(state, Some("secret-token".to_string()), None);
    (app, registry)
}

#[tokio::test]
async fn session_scoped_actions_reject_missing_and_wrong_identity() {
    let _test_guard = test_lock().lock().await;
    let _launch_guard = install_fake_provider();

    let registry = AcpSessionRegistry::new();
    let session_id = create_owned_session(&registry, "alice").await;

    let missing_list = dispatch_with_registry(&registry, "session.list", json!({}))
        .await
        .expect("session.list without principal returns scoped empty list");
    assert!(
        missing_list["sessions"]
            .as_array()
            .expect("sessions array")
            .is_empty()
    );

    let wrong_list = dispatch_with_registry(
        &registry,
        "session.list",
        json!({
            "principal": "bob",
        }),
    )
    .await
    .expect("wrong principal session.list returns scoped empty list");
    assert!(
        wrong_list["sessions"]
            .as_array()
            .expect("sessions array")
            .is_empty()
    );

    let missing_prompt = dispatch_with_registry(
        &registry,
        "session.prompt",
        json!({
            "session_id": session_id,
            "text": "hello from anonymous",
        }),
    )
    .await
    .expect_err("missing principal must not prompt an owned session");
    assert_eq!(missing_prompt.kind(), "auth_failed");

    let wrong_prompt = dispatch_with_registry(
        &registry,
        "session.prompt",
        json!({
            "session_id": session_id,
            "text": "hello from bob",
            "principal": "bob",
        }),
    )
    .await
    .expect_err("wrong principal must not prompt another user's session");
    assert_eq!(wrong_prompt.kind(), "not_found");

    let missing_cancel = dispatch_with_registry(
        &registry,
        "session.cancel",
        json!({
            "session_id": session_id,
            "confirm": true,
        }),
    )
    .await
    .expect_err("missing principal must not cancel an owned session");
    assert_eq!(missing_cancel.kind(), "auth_failed");

    let wrong_cancel = dispatch_with_registry(
        &registry,
        "session.cancel",
        json!({
            "session_id": session_id,
            "confirm": true,
            "principal": "bob",
        }),
    )
    .await
    .expect_err("wrong principal must not cancel another user's session");
    assert_eq!(wrong_cancel.kind(), "not_found");

    let missing_close = dispatch_with_registry(
        &registry,
        "session.close",
        json!({
            "session_id": session_id,
            "confirm": true,
        }),
    )
    .await
    .expect_err("missing principal must not close an owned session");
    assert_eq!(missing_close.kind(), "auth_failed");

    let wrong_close = dispatch_with_registry(
        &registry,
        "session.close",
        json!({
            "session_id": session_id,
            "confirm": true,
            "principal": "bob",
        }),
    )
    .await
    .expect_err("wrong principal must not close another user's session");
    assert_eq!(wrong_close.kind(), "not_found");

    let missing_identity = dispatch_with_registry(
        &registry,
        "session.events",
        json!({
            "session_id": session_id,
            "since": 0,
        }),
    )
    .await
    .expect_err("missing principal must fail for an owned session");
    assert_eq!(missing_identity.kind(), "auth_failed");

    let wrong_identity = dispatch_with_registry(
        &registry,
        "session.events",
        json!({
            "session_id": session_id,
            "since": 0,
            "principal": "bob",
        }),
    )
    .await
    .expect_err("wrong principal must fail");
    assert_eq!(wrong_identity.kind(), "not_found");

    let anonymous_ticket = dispatch_with_registry(
        &registry,
        "session.subscribe_ticket",
        json!({
            "session_id": session_id,
        }),
    )
    .await
    .expect_err("anonymous subscribe ticket must fail for an owned session");
    assert_eq!(anonymous_ticket.kind(), "auth_failed");
}

#[tokio::test]
async fn destructive_acp_actions_require_confirmation() {
    let _test_guard = test_lock().lock().await;

    let registry = AcpSessionRegistry::new();

    for action in ["session.cancel", "session.close"] {
        let err = dispatch_with_registry(
            &registry,
            action,
            json!({
                "session_id": "sess-123",
            }),
        )
        .await
        .expect_err("destructive action without confirm must fail");

        assert_eq!(err.kind(), "confirmation_required");
    }

    let explicit_false = dispatch_with_registry(
        &registry,
        "session.cancel",
        json!({
            "session_id": "sess-123",
            "confirm": false,
        }),
    )
    .await
    .expect_err("confirm=false must fail");
    assert_eq!(explicit_false.kind(), "confirmation_required");
}

#[tokio::test]
async fn http_acp_handlers_scope_sessions_to_authenticated_principal() {
    let _test_guard = test_lock().lock().await;
    let _launch_guard = install_fake_provider();

    let (app, registry) = acp_test_app();

    let unauthenticated_create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/acp/sessions")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({}).to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthenticated_create.status(), StatusCode::UNAUTHORIZED);

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/acp/sessions")
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({}).to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(create.status(), StatusCode::OK);
    let created = json_body(create).await;
    let owned_by_static_bearer = created["id"].as_str().expect("session id");

    let ticket = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/acp/sessions/{owned_by_static_bearer}/subscribe_ticket"
                ))
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(ticket.status(), StatusCode::OK);
    let ticket_json = json_body(ticket).await;
    let ticket_value = ticket_json["ticket"].as_str().expect("ticket");
    let (ticket_session, ticket_principal) =
        validate_subscribe_ticket(ticket_value).expect("ticket validates");
    assert_eq!(ticket_session, owned_by_static_bearer);
    assert_eq!(ticket_principal, "static-bearer");

    let alice_session = create_owned_session(registry.as_ref(), "alice").await;
    let non_owner_ticket = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/acp/sessions/{alice_session}/subscribe_ticket"))
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(non_owner_ticket.status(), StatusCode::NOT_FOUND);
    let non_owner_json = json_body(non_owner_ticket).await;
    assert_eq!(non_owner_json["kind"], "not_found");
}

#[tokio::test]
async fn http_acp_action_route_uses_shared_action_params_contract() {
    let _test_guard = test_lock().lock().await;
    let _launch_guard = install_fake_provider();

    let (app, _registry) = acp_test_app();

    let provider_list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/acp")
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "action": "provider.list",
                        "params": {}
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(provider_list.status(), StatusCode::OK);
    let provider_list_json = json_body(provider_list).await;
    assert!(provider_list_json["providers"].is_array());

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/acp")
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "action": "session.start",
                        "params": {
                            "principal": "alice",
                            "title": "action route session"
                        }
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(create.status(), StatusCode::OK);
    let created = json_body(create).await;
    let session_id = created["id"].as_str().expect("session id");

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/acp")
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "action": "session.list" }).to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = json_body(list).await;
    let sessions = list_json["sessions"].as_array().expect("sessions array");
    assert!(sessions.iter().any(|session| session["id"] == session_id));
}

#[tokio::test]
async fn http_acp_action_route_returns_shared_error_envelopes() {
    let _test_guard = test_lock().lock().await;

    let (app, _registry) = acp_test_app();

    let unknown = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/acp")
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "action": "not.real",
                        "params": {}
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    let unknown_json = json_body(unknown).await;
    assert_eq!(unknown_json["kind"], "unknown_action");
    assert!(unknown_json["valid"].is_array());

    let missing_confirmation = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/acp")
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "action": "session.cancel",
                        "params": {
                            "session_id": "sess-123"
                        }
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        missing_confirmation.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let missing_confirmation_json = json_body(missing_confirmation).await;
    assert_eq!(missing_confirmation_json["kind"], "confirmation_required");
}

#[tokio::test]
async fn acp_prompt_accepts_local_text_attachment_as_embedded_resource() {
    let _guard = test_lock().lock().await;
    let capture = prompt_capture_path("acp-local-text-attachment");
    let _launch = install_fake_provider_with_prompt_capture(&capture);
    let (app, registry) = acp_test_app();
    let session_id = create_owned_session(&registry, "static-bearer").await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/acp/sessions/{session_id}/prompt"))
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "prompt": "Summarize",
                        "attachments": [{
                            "kind": "local",
                            "id": "local-notes",
                            "name": "notes.txt",
                            "mimeType": "text/plain",
                            "size": 11,
                            "contentKind": "text",
                            "text": "hello world"
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);

    let captured = read_prompt_capture(&capture);
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0][0]["type"], "text");
    assert_eq!(captured[0][0]["text"], "Summarize");
    assert_eq!(captured[0][1]["type"], "resource");
    assert_eq!(
        captured[0][1]["resource"]["uri"],
        "file://local-attachment/notes.txt"
    );
    assert_eq!(captured[0][1]["resource"]["mimeType"], "text/plain");
    assert_eq!(captured[0][1]["resource"]["text"], "hello world");
}

#[tokio::test]
async fn acp_prompt_preserves_workspace_attachment_noop_behavior() {
    let _guard = test_lock().lock().await;
    let capture = prompt_capture_path("acp-workspace-attachment");
    let _launch = install_fake_provider_with_prompt_capture(&capture);
    let (app, registry) = acp_test_app();
    let session_id = create_owned_session(&registry, "static-bearer").await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/acp/sessions/{session_id}/prompt"))
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "prompt": "Summarize",
                        "attachments": [{
                            "kind": "file",
                            "path": "/tmp/a.txt"
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let (status, body) = response_body_text(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let captured = read_prompt_capture(&capture);
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].as_array().expect("prompt blocks").len(), 1);
    assert_eq!(captured[0][0]["text"], "Summarize");
}

#[tokio::test]
async fn acp_prompt_rejects_oversized_local_attachment_metadata() {
    let _guard = test_lock().lock().await;
    let _launch = install_fake_provider();
    let (app, registry) = acp_test_app();
    let session_id = create_owned_session(&registry, "alice").await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/acp/sessions/{session_id}/prompt"))
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "prompt": "Summarize",
                        "attachments": [{
                            "kind": "local",
                            "id": "local-big",
                            "name": "big.txt",
                            "mimeType": "text/plain",
                            "size": 2_097_153,
                            "contentKind": "text",
                            "text": "too big"
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(response).await;
    assert_eq!(body["kind"], "invalid_param");
    assert_eq!(body["param"], "attachments");
}

#[tokio::test]
async fn subscribe_ticket_validation_covers_success_and_failure_paths() {
    let _test_guard = test_lock().lock().await;
    let _launch_guard = install_fake_provider();

    let registry = AcpSessionRegistry::new();
    let session_id = create_owned_session(&registry, "alice").await;

    let issued = dispatch_with_registry(
        &registry,
        "session.subscribe_ticket",
        json!({
            "session_id": session_id,
            "principal": "alice",
        }),
    )
    .await
    .expect("subscribe ticket");

    let ticket = issued["ticket"].as_str().expect("ticket");
    let (validated_session_id, validated_principal) =
        validate_subscribe_ticket(ticket).expect("ticket validates");
    assert_eq!(validated_session_id, session_id);
    assert_eq!(validated_principal, "alice");

    let malformed = validate_subscribe_ticket("%%%").expect_err("malformed ticket must fail");
    assert_eq!(malformed.kind(), "auth_failed");

    let raw = B64.decode(ticket).expect("decode issued ticket");
    let mut tampered = String::from_utf8(raw).expect("ticket utf8");
    let last = tampered.pop().expect("ticket char");
    tampered.push(if last == '0' { '1' } else { '0' });
    let tampered_ticket = B64.encode(tampered.as_bytes());

    let bad_signature =
        validate_subscribe_ticket(&tampered_ticket).expect_err("tampered ticket must fail");
    assert_eq!(bad_signature.kind(), "auth_failed");
}

#[tokio::test]
async fn sse_event_subscription_requires_authenticated_ticketed_access() {
    let _test_guard = test_lock().lock().await;
    let _env_guard = install_fake_provider();

    let (app, registry) = acp_test_app();
    let session_id = create_owned_session(registry.as_ref(), "alice").await;
    let issued = dispatch_with_registry(
        registry.as_ref(),
        "session.subscribe_ticket",
        json!({
            "session_id": session_id,
            "principal": "alice",
        }),
    )
    .await
    .expect("subscribe ticket");
    let ticket = issued["ticket"].as_str().expect("ticket");
    let ticket_query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("ticket", ticket)
        .finish();

    let unauthenticated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/acp/sessions/{session_id}/events"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let unauthenticated_json = json_body(unauthenticated).await;
    assert_eq!(unauthenticated_json["kind"], "auth_failed");

    let invalid_ticket = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/acp/sessions/{session_id}/events?ticket=invalid-ticket"
                ))
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(invalid_ticket.status(), StatusCode::UNAUTHORIZED);
    let invalid_ticket_json = json_body(invalid_ticket).await;
    assert_eq!(invalid_ticket_json["kind"], "auth_failed");

    let mismatched_session = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/acp/sessions/not-{session_id}/events?{ticket_query}"
                ))
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(mismatched_session.status(), StatusCode::UNAUTHORIZED);
    let mismatched_json = json_body(mismatched_session).await;
    assert_eq!(mismatched_json["kind"], "auth_failed");

    let success = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/acp/sessions/{session_id}/events?{ticket_query}"
                ))
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(success.status(), StatusCode::OK);
    assert_eq!(
        success
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
    );
}
