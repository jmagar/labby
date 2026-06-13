#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::panic,
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
    config::{LabConfig, NodePreferences, NodeRole, NodeRuntimeRole},
    node::{
        identity::{resolve_runtime_role, resolve_runtime_role_from_config},
        store::NodeStore,
    },
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

// ── Role resolution tests: these validate the early-return gate in serve.rs ──
//
// A node (NonMaster) resolving its role is the condition that triggers the
// `run_node_mode` early return before `build_default_registry()`.

#[test]
fn role_resolution_node_returns_non_master() {
    let resolved = resolve_runtime_role("worker-01", Some("controller")).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::NonMaster),
        "expected NonMaster but got {:?}",
        resolved.role
    );
    assert_eq!(resolved.local_host, "worker-01");
    assert_eq!(resolved.master_host, "controller");
}

#[test]
fn role_resolution_controller_returns_master() {
    let resolved = resolve_runtime_role("controller", Some("controller")).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::Master),
        "expected Master but got {:?}",
        resolved.role
    );
}

#[test]
fn role_resolution_no_controller_defaults_to_master() {
    let resolved = resolve_runtime_role("any-host", None).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::Master),
        "expected Master (no controller configured) but got {:?}",
        resolved.role
    );
}

#[test]
fn config_with_controller_makes_different_host_a_node() {
    let config = LabConfig {
        node: Some(NodePreferences {
            controller: Some("controller.lab".to_string()),
            log_retention_days: None,
            role: None,
        }),
        ..LabConfig::default()
    };
    let resolved = resolve_runtime_role_from_config("worker-02", &config, None).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::NonMaster),
        "host different from configured controller should be NonMaster, got {:?}",
        resolved.role
    );
}

#[test]
fn config_with_controller_makes_same_host_the_master() {
    let config = LabConfig {
        node: Some(NodePreferences {
            controller: Some("controller.lab".to_string()),
            log_retention_days: None,
            role: None,
        }),
        ..LabConfig::default()
    };
    let resolved = resolve_runtime_role_from_config("controller.lab", &config, None).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::Master),
        "host matching configured controller should be Master, got {:?}",
        resolved.role
    );
}

#[test]
fn explicit_role_node_override_with_different_controller_is_non_master() {
    let config = LabConfig {
        node: Some(NodePreferences {
            controller: Some("controller.lab".to_string()),
            log_retention_days: None,
            role: None,
        }),
        ..LabConfig::default()
    };
    // --role node with a different controller host resolves to NonMaster.
    let resolved =
        resolve_runtime_role_from_config("worker-04", &config, Some(NodeRuntimeRole::Node))
            .unwrap();
    assert!(
        matches!(resolved.role, NodeRole::NonMaster),
        "explicit --role node with different controller should be NonMaster, got {:?}",
        resolved.role
    );
}

#[test]
fn explicit_role_controller_override_forces_master() {
    let config = LabConfig {
        node: Some(NodePreferences {
            controller: Some("controller.lab".to_string()),
            log_retention_days: None,
            role: None,
        }),
        ..LabConfig::default()
    };
    // Even if the hostname differs, --role controller forces Master.
    let resolved =
        resolve_runtime_role_from_config("worker-03", &config, Some(NodeRuntimeRole::Controller))
            .unwrap();
    assert!(
        matches!(resolved.role, NodeRole::Master),
        "explicit --role controller should force Master, got {:?}",
        resolved.role
    );
}

#[test]
fn no_node_config_defaults_to_master() {
    let config = LabConfig::default();
    let resolved = resolve_runtime_role_from_config("any-host", &config, None).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::Master),
        "no node config should default to Master, got {:?}",
        resolved.role
    );
}

#[test]
fn role_node_without_controller_host_returns_error() {
    // --role node with no [node].controller configured must fail
    let config = LabConfig::default();
    let result = resolve_runtime_role_from_config("somehost", &config, Some(NodeRuntimeRole::Node));
    assert!(
        result.is_err(),
        "expected error when --role node has no controller host"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("controller host") || msg.contains("[node].controller"),
        "error message should mention controller host config, got: {msg}"
    );
}

#[test]
fn config_role_node_without_controller_host_returns_error() {
    // [node].role = "node" but no [node].controller configured must fail
    let config = LabConfig {
        node: Some(NodePreferences {
            role: Some(NodeRuntimeRole::Node),
            controller: None,
            log_retention_days: None,
        }),
        ..LabConfig::default()
    };
    let result = resolve_runtime_role_from_config("somehost", &config, None);
    assert!(
        result.is_err(),
        "expected error when [node].role=node but no controller host"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("controller host") || msg.contains("[node].controller"),
        "error message should mention controller host config, got: {msg}"
    );
}

#[test]
fn role_node_with_controller_host_succeeds() {
    // Success case: verifies the error is ONLY about missing host, not the role itself
    let config = LabConfig {
        node: Some(NodePreferences {
            role: None,
            controller: Some("dookie".to_string()),
            log_retention_days: None,
        }),
        ..LabConfig::default()
    };
    let result = resolve_runtime_role_from_config("somehost", &config, Some(NodeRuntimeRole::Node));
    assert!(
        result.is_ok(),
        "should succeed when controller host is configured: {result:?}"
    );
}
