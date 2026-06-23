#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use labby::{
    api::{router::build_router_with_bearer, state::AppState},
    node::checkin::NodeHello,
    node::enrollment::store::{EnrollmentAttempt, EnrollmentStore, TailnetIdentity},
    node::log_event::NodeLogEvent,
    node::store::NodeStore,
};
use tower::ServiceExt;

#[tokio::test]
async fn hello_endpoint_updates_master_store() {
    let (app, _store, _enrollment_store) = test_device_router();
    let response = app
        .oneshot(hello_request(
            r#"{"node_id":"dookie","role":"non-master","version":"1.0.0"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn hello_endpoint_normalizes_node_id_before_storage() {
    let (app, store, _enrollment_store) = test_device_router();
    let response = app
        .oneshot(hello_request(
            r#"{"node_id":"  dookie  ","role":"non-master","version":"1.0.0"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(store.node("dookie").await.is_some());
}

#[tokio::test]
async fn syslog_batch_endpoint_accepts_normalized_events() {
    let (app, store, _enrollment_store) = test_device_router();
    let response = app
        .oneshot(syslog_request(
            r#"{"node_id":"dookie","events":[{"node_id":"dookie","source":"journald","timestamp_unix_ms":1,"message":"hello","fields":{}}]}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = store.node("dookie").await.unwrap();
    assert_eq!(snapshot.logs.len(), 1);
}

#[tokio::test]
async fn get_device_rejects_invalid_node_id() {
    let (app, _store, _enrollment_store) = test_device_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/nodes/%20%20%20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn syslog_batch_endpoint_rejects_invalid_node_id() {
    let (app, _store, _enrollment_store) = test_device_router();
    let response = app
        .oneshot(syslog_request(
            r#"{"node_id":"   ","events":[{"node_id":"dookie","source":"journald","timestamp_unix_ms":1,"message":"hello","fields":{}}]}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn syslog_batch_endpoint_rejects_mismatched_event_node_id() {
    let (app, _store, _enrollment_store) = test_device_router();
    let response = app
        .oneshot(syslog_request(
            r#"{"node_id":"dookie","events":[{"node_id":"tootie","source":"journald","timestamp_unix_ms":1,"message":"hello","fields":{}}]}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn device_oauth_route_calls_runtime_wrapper() {
    let (app, _store, _enrollment_store) = test_device_router();
    let response = app
        .oneshot(oauth_relay_start_request(
            r#"{"bind_addr":"127.0.0.1:0","target_url":"http://127.0.0.1:9/callback","request_timeout_ms":100}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["ok"], true);
    assert_ne!(payload["bind_addr"], "127.0.0.1:0");
}

#[tokio::test]
async fn device_oauth_route_rejects_non_loopback_bind_addr() {
    let (app, _store, _enrollment_store) = test_device_router();
    let response = app
        .oneshot(oauth_relay_start_request(
            r#"{"bind_addr":"10.0.0.5:9876","target_url":"http://127.0.0.1:9/callback","request_timeout_ms":100}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn device_oauth_route_rejects_invalid_target_url() {
    let (app, _store, _enrollment_store) = test_device_router();
    let response = app
        .oneshot(oauth_relay_start_request(
            r#"{"bind_addr":"127.0.0.1:0","target_url":"not-a-url","request_timeout_ms":100}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn existing_fleet_logs_search_still_works() {
    let (app, store, _enrollment_store) = test_device_router();
    store
        .record_hello(NodeHello {
            node_id: "dookie".to_string(),
            role: "non-master".to_string(),
            version: "1.0.0".to_string(),
        })
        .await;
    store
        .record_logs(
            "dookie",
            vec![NodeLogEvent {
                node_id: "dookie".to_string(),
                timestamp_unix_ms: 1,
                source: "journald".to_string(),
                level: Some("info".to_string()),
                message: "hello from fleet search".to_string(),
                fields: serde_json::Map::new(),
            }],
        )
        .await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/nodes/logs/search")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "node_id":"dookie",
                        "query":"hello"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: Vec<NodeLogEvent> = serde_json::from_slice(&body).unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].message.contains("fleet search"));
}

#[tokio::test]
async fn list_enrollments_returns_pending_and_approved_records() {
    let (app, _store, enrollment_store) = test_device_router();
    enrollment_store
        .record_pending(EnrollmentAttempt {
            node_id: "pending-1".to_string(),
            token: "token-1".to_string(),
            tailnet_identity: TailnetIdentity {
                node_key: "node".to_string(),
                login_name: "user@example.com".to_string(),
                hostname: "pending-1".to_string(),
            },
            client_version: "0.7.3".to_string(),
            metadata: None,
        })
        .await
        .unwrap();
    enrollment_store.approve("pending-1", None).await.unwrap();
    enrollment_store
        .record_pending(EnrollmentAttempt {
            node_id: "pending-2".to_string(),
            token: "token-2".to_string(),
            tailnet_identity: TailnetIdentity {
                node_key: "node2".to_string(),
                login_name: "user@example.com".to_string(),
                hostname: "pending-2".to_string(),
            },
            client_version: "0.7.3".to_string(),
            metadata: None,
        })
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/nodes/enrollments")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["pending"]["pending-2"].is_object());
    assert!(payload["approved"]["pending-1"].is_object());
}

#[tokio::test]
async fn approve_enrollment_promotes_pending_record() {
    let (app, _store, enrollment_store) = test_device_router();
    enrollment_store
        .record_pending(EnrollmentAttempt {
            node_id: "pending-1".to_string(),
            token: "token-1".to_string(),
            tailnet_identity: TailnetIdentity {
                node_key: "node".to_string(),
                login_name: "user@example.com".to_string(),
                hostname: "pending-1".to_string(),
            },
            client_version: "0.7.3".to_string(),
            metadata: None,
        })
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/nodes/enrollments/pending-1/approve")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"note":"ok"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = enrollment_store.list().await.unwrap();
    assert!(snapshot.pending.is_empty());
    assert!(snapshot.approved.contains_key("pending-1"));
}

#[tokio::test]
async fn deny_enrollment_marks_record_denied() {
    let (app, _store, enrollment_store) = test_device_router();
    enrollment_store
        .record_pending(EnrollmentAttempt {
            node_id: "pending-1".to_string(),
            token: "token-1".to_string(),
            tailnet_identity: TailnetIdentity {
                node_key: "node".to_string(),
                login_name: "user@example.com".to_string(),
                hostname: "pending-1".to_string(),
            },
            client_version: "0.7.3".to_string(),
            metadata: None,
        })
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/nodes/enrollments/pending-1/deny")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"reason":"no"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = enrollment_store.list().await.unwrap();
    assert!(snapshot.pending.is_empty());
    assert!(snapshot.denied.contains_key("pending-1"));
}

fn test_device_router() -> (axum::Router, Arc<NodeStore>, Arc<EnrollmentStore>) {
    let store = Arc::new(NodeStore::default());
    let enrollment_store = Arc::new(
        futures::executor::block_on(EnrollmentStore::open(
            std::env::temp_dir().join(format!("lab-device-api-{}.json", uuid::Uuid::new_v4())),
        ))
        .unwrap(),
    );
    let state = AppState::new()
        .with_node_store(Arc::clone(&store))
        .with_enrollment_store(Arc::clone(&enrollment_store));
    (
        build_router_with_bearer(state, None, None),
        store,
        enrollment_store,
    )
}

fn hello_request(body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/nodes/hello")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn syslog_request(body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/nodes/syslog/batch")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn oauth_relay_start_request(body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/nodes/oauth/relay/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}
