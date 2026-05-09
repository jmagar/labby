use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    body::{Body, HttpBody},
    http::{Request, StatusCode, header},
};
use futures::future::poll_fn;
use tower::ServiceExt;

mod support;

use support::log_system::{InstalledLogSystemGuard, test_lock};

fn logs_registry() -> labby::registry::ToolRegistry {
    let mut registry = labby::registry::ToolRegistry::new();
    registry.register(labby::registry::RegisteredService {
        name: "logs",
        description: "Search and stream local-master runtime logs",
        category: "bootstrap",
        kind: labby::registry::RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: labby::dispatch::logs::ACTIONS,
        dispatch: |action, params| {
            Box::pin(async move { labby::dispatch::logs::dispatch(&action, params).await })
        },
    });
    registry
}

fn raw_gateway_event(message: &str) -> labby::dispatch::logs::types::RawLogEvent {
    labby::dispatch::logs::types::RawLogEvent {
        ts: Some(1_713_225_600_000),
        level: Some("warn".to_string()),
        subsystem: Some("gateway".to_string()),
        surface: Some("api".to_string()),
        action: Some("gateway.list".to_string()),
        message: message.to_string(),
        request_id: Some("req-gateway".to_string()),
        session_id: None,
        correlation_id: None,
        trace_id: None,
        span_id: None,
        instance: Some("default".to_string()),
        auth_flow: None,
        outcome_kind: Some("ok".to_string()),
        fields_json: serde_json::json!({"route":"gateway.list"}),
        source_kind: None,
        source_node_id: None,
        source_device_id: None,
        actor_key: None,
        ingest_path: None,
        upstream_event_id: None,
    }
}

async fn test_app() -> (Router, Arc<labby::dispatch::logs::types::LogSystem>) {
    let logs_system = labby::dispatch::logs::client::bootstrap_running_log_system_for_test(16)
        .await
        .expect("log system");
    let state = labby::api::state::AppState::from_registry(logs_registry())
        .with_log_system(Arc::clone(&logs_system));
    (
        labby::api::router::build_router_with_bearer(state, None, None),
        logs_system,
    )
}

async fn read_next_body_chunk(mut body: std::pin::Pin<&mut Body>) -> Option<axum::body::Bytes> {
    loop {
        let frame = match poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
            Some(Ok(frame)) => frame,
            Some(Err(error)) => panic!("body frame result: {error}"),
            None => return None,
        };
        if let Ok(data) = frame.into_data() {
            return Some(data);
        }
    }
}

async fn wait_for_substring(body: Body, needle: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut body = pin!(body);
    let mut collected = String::new();

    loop {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            panic!("never observed {needle}; got {collected:?}");
        };

        let next_chunk = tokio::time::timeout(remaining, read_next_body_chunk(body.as_mut()))
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {needle}; got {collected:?}"));
        let Some(chunk) = next_chunk else {
            panic!("stream closed before {needle}; got {collected:?}");
        };

        collected.push_str(std::str::from_utf8(&chunk).unwrap_or(""));
        if collected.contains(needle) {
            return collected;
        }
    }
}

#[tokio::test]
async fn post_logs_search_route_exists() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let (app, _logs_system) = test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/logs")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"action":"logs.search","params":{"query":{}}}).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn logs_stream_sse_route_emits_event_stream_content_type() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let (app, _logs_system) = test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/logs/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
}

#[tokio::test]
async fn logs_sse_subscribers_receive_events_after_subscribe() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let (app, logs_system) = test_app().await;

    let first_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/logs/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("first response");

    logs_system
        .ingest(raw_gateway_event("first sse payload"))
        .await
        .expect("first event");

    let first_text = wait_for_substring(first_response.into_body(), "first sse payload").await;
    assert!(first_text.contains("first sse payload"));

    let second_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/logs/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("second response");

    logs_system
        .ingest(raw_gateway_event("second sse payload"))
        .await
        .expect("second event");

    let second_text = wait_for_substring(second_response.into_body(), "second sse payload").await;
    assert!(second_text.contains("second sse payload"));
}

#[tokio::test]
async fn logs_mcp_tail_matches_api_query_semantics() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let (app, logs_system) = test_app().await;
    logs_system
        .ingest(raw_gateway_event("shared tail semantics"))
        .await
        .expect("seed event");

    let mcp_value = labby::dispatch::logs::dispatch(
        "logs.tail",
        serde_json::json!({ "after_ts": 0, "limit": 10 }),
    )
    .await
    .expect("mcp tail");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/logs")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "action":"logs.tail",
                        "params":{"after_ts":0,"limit":10}
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response bytes");
    let api_value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(api_value, mcp_value);
}

#[tokio::test]
async fn logs_routes_respect_runtime_service_filtering() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let logs_system = labby::dispatch::logs::client::bootstrap_running_log_system_for_test(16)
        .await
        .expect("log system");
    let state = labby::api::state::AppState::from_registry(labby::registry::ToolRegistry::new())
        .with_log_system(logs_system);
    let app = labby::api::router::build_router_with_bearer(state, None, None);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/logs")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"action":"logs.search","params":{"query":{}}}).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
