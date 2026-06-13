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
    http::{Request, StatusCode},
};
use lab_auth::config::{AuthConfig, AuthMode, GoogleConfig};
use labby::{
    api::{router::build_router_with_bearer, state::AppState},
    config::NodeRole,
    node::store::NodeStore,
};
use tower::ServiceExt;

#[tokio::test]
async fn non_master_router_rejects_gateway_api_surface() {
    let fixture = test_non_master_router();
    let app = fixture.router;
    let response = app.oneshot(gateway_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn non_master_router_rejects_web_ui_surface() {
    let fixture = test_non_master_router();
    let app = fixture.router;
    let response = app.oneshot(web_request()).await.unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
    ));
}

#[tokio::test]
async fn non_master_router_does_not_mount_oauth_metadata_surface() {
    let fixture = test_lab_auth_state().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("index.html"),
        "<html><body>Labby</body></html>",
    )
    .unwrap();

    let state = AppState::new()
        .with_node_store(Arc::new(NodeStore::default()))
        .with_node_role(NodeRole::NonMaster)
        .with_web_assets_dir(dir.path().to_path_buf());
    let app = labby::api::router::build_router(state, None, Some(fixture.auth_state), None, &[]);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/oauth-authorization-server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
    ));
}

struct NonMasterRouterFixture {
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    router: axum::Router,
}

fn test_non_master_router() -> NonMasterRouterFixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("index.html"),
        "<html><body>Labby</body></html>",
    )
    .unwrap();

    let state = AppState::new()
        .with_node_store(Arc::new(NodeStore::default()))
        .with_node_role(NodeRole::NonMaster)
        .with_web_assets_dir(dir.path().to_path_buf());
    NonMasterRouterFixture {
        dir,
        router: build_router_with_bearer(state, None, None),
    }
}

fn gateway_request() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/gateway")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"action":"gateway.list","params":{}}"#.to_string(),
        ))
        .unwrap()
}

fn web_request() -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/gateways/")
        .body(Body::empty())
        .unwrap()
}

struct TestAuthFixture {
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    auth_state: lab_auth::state::AuthState,
}

async fn test_lab_auth_state() -> TestAuthFixture {
    let dir = tempfile::tempdir().unwrap();
    let config = AuthConfig {
        mode: AuthMode::OAuth,
        public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
        sqlite_path: dir.path().join("auth.db"),
        key_path: dir.path().join("auth-jwt.pem"),
        bootstrap_secret: Some("bootstrap-secret".to_string()),
        google: GoogleConfig {
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            callback_path: "/auth/google/callback".to_string(),
            scopes: vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ],
        },
        ..AuthConfig::default()
    };
    let auth_state = lab_auth::state::AuthState::new(config).await.unwrap();
    TestAuthFixture { dir, auth_state }
}
