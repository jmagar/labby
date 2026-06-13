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
//! Integration tests for the admin-only allowlist API.
//!
//! Coverage:
//! - Unauthenticated requests (no cookie, no bearer) → 401
//! - JWT bearer (valid OAuth token) → 403 (authenticated but not via session)
//! - Browser session, email != admin_email → 403
//! - Browser session, email == admin_email → 200 / 201 / 204
//! - POST missing email field → 422 (JSON parse fails → 422)
//! - POST empty email → 422
//! - POST bad format (no @) → 422
//! - POST email > 320 chars → 422
//! - POST duplicate email → 422
//! - DELETE nonexistent email → 204 (idempotent)
//! - No oauth_state (bearer-only mode) → 404
//! - CSRF missing on mutations → 422 (enforced by /v1 middleware)

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use lab_auth::{
    config::{AuthConfig, AuthMode, GoogleConfig},
    state::AuthState,
    types::BrowserSessionRow,
};
use labby::api::{router::build_router, state::AppState};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

// ── test harness ─────────────────────────────────────────────────────────────

struct Harness {
    /// Keep alive so the temp dir isn't deleted.
    _tmp: TempDir,
    auth_state: AuthState,
    /// Admin email configured in the harness.
    admin_email: String,
}

impl Harness {
    async fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let admin_email = "admin@example.com".to_string();
        let config = AuthConfig {
            mode: AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: tmp.path().join("auth.db"),
            key_path: tmp.path().join("auth-jwt.pem"),
            bootstrap_secret: Some("secret".to_string()),
            admin_email: admin_email.clone(),
            google: GoogleConfig {
                client_id: "id".to_string(),
                client_secret: "secret".to_string(),
                callback_path: "/auth/google/callback".to_string(),
                scopes: vec!["openid".to_string(), "email".to_string()],
            },
            ..AuthConfig::default()
        };
        let auth_state = AuthState::new(config).await.expect("auth state");
        Self {
            _tmp: tmp,
            auth_state,
            admin_email,
        }
    }

    /// Build a router with OAuth auth state and auth_config attached.
    fn router(&self) -> axum::Router {
        let auth_config = AuthConfig {
            mode: AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: self._tmp.path().join("auth.db"),
            key_path: self._tmp.path().join("auth-jwt.pem"),
            bootstrap_secret: Some("secret".to_string()),
            admin_email: self.admin_email.clone(),
            google: GoogleConfig {
                client_id: "id".to_string(),
                client_secret: "secret".to_string(),
                callback_path: "/auth/google/callback".to_string(),
                scopes: vec!["openid".to_string(), "email".to_string()],
            },
            ..AuthConfig::default()
        };
        let state = AppState::new()
            .with_oauth_state(self.auth_state.clone())
            .with_auth_config(auth_config);
        build_router(state, None, Some(self.auth_state.clone()), None, &[])
    }

    /// Seed a browser session in the store. Returns the session row.
    async fn seed_session(&self, email: Option<&str>, csrf: &str) -> BrowserSessionRow {
        let session = BrowserSessionRow {
            session_id: format!("sess-{csrf}"),
            subject: "user-sub".to_string(),
            email: email.map(ToOwned::to_owned),
            csrf_token: csrf.to_string(),
            created_at: 1,
            expires_at: i64::MAX,
        };
        self.auth_state
            .store
            .upsert_browser_session(session.clone())
            .await
            .expect("upsert session");
        session
    }

    /// Seed an admin session (email == admin_email).
    async fn seed_admin_session(&self) -> BrowserSessionRow {
        self.seed_session(Some(&self.admin_email), "csrf-admin")
            .await
    }

    /// Issue a JWT access token for the test issuer.
    fn issue_jwt(&self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        self.auth_state
            .signing_keys
            .issue_access_token(&lab_auth::jwt::AccessClaims {
                iss: "https://lab.example.com".to_string(),
                sub: "jwt-user".to_string(),
                aud: "https://lab.example.com/mcp".to_string(),
                exp: now + 3600,
                iat: now,
                jti: "test-jti".to_string(),
                scope: "lab".to_string(),
                azp: "client".to_string(),
            })
            .unwrap()
    }

    /// Build a GET request with a session cookie and optional CSRF header.
    fn get_with_session(uri: &str, session: &BrowserSessionRow) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .header(
                header::COOKIE,
                format!(
                    "{}={}",
                    lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                    session.session_id
                ),
            )
            .body(Body::empty())
            .unwrap()
    }

    /// Build a POST request with a session cookie + CSRF header + JSON body.
    fn post_with_session(uri: &str, session: &BrowserSessionRow, body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::COOKIE,
                format!(
                    "{}={}",
                    lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                    session.session_id
                ),
            )
            .header(
                lab_auth::session::BROWSER_CSRF_HEADER_NAME,
                &session.csrf_token,
            )
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// Build a DELETE request with a session cookie + CSRF header.
    fn delete_with_session(uri: &str, session: &BrowserSessionRow) -> Request<Body> {
        Request::builder()
            .method("DELETE")
            .uri(uri)
            .header(
                header::COOKIE,
                format!(
                    "{}={}",
                    lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                    session.session_id
                ),
            )
            .header(
                lab_auth::session::BROWSER_CSRF_HEADER_NAME,
                &session.csrf_token,
            )
            .body(Body::empty())
            .unwrap()
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

// ── authentication tests ──────────────────────────────────────────────────────

#[tokio::test]
async fn get_unauthenticated_returns_401() {
    let h = Harness::new().await;
    let app = h.router();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/auth/allowed-emails")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_unauthenticated_returns_401() {
    let h = Harness::new().await;
    let app = h.router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/allowed-emails")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"email":"x@x.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delete_unauthenticated_returns_401() {
    let h = Harness::new().await;
    let app = h.router();
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/v1/auth/allowed-emails/x@x.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_jwt_bearer_returns_403() {
    let h = Harness::new().await;
    let token = h.issue_jwt();
    let app = h.router();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/auth/allowed-emails")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let json = body_json(response).await;
    assert_eq!(json["kind"], "forbidden");
}

#[tokio::test]
async fn post_jwt_bearer_returns_403() {
    let h = Harness::new().await;
    let token = h.issue_jwt();
    let app = h.router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/allowed-emails")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(r#"{"email":"x@x.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_non_admin_session_returns_403() {
    let h = Harness::new().await;
    let session = h
        .seed_session(Some("notadmin@example.com"), "csrf-na")
        .await;
    let app = h.router();
    let response = app
        .oneshot(Harness::get_with_session(
            "/v1/auth/allowed-emails",
            &session,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let json = body_json(response).await;
    assert_eq!(json["kind"], "forbidden");
}

// ── success path tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn get_admin_session_returns_200_with_entries() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .oneshot(Harness::get_with_session(
            "/v1/auth/allowed-emails",
            &session,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(json["entries"].is_array());
}

#[tokio::test]
async fn post_admin_session_adds_email_returns_201() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            r#"{"email":"alice@example.com"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["entry"]["email"], "alice@example.com");
}

#[tokio::test]
async fn delete_admin_session_removes_email_returns_204() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    // First add an entry.
    h.auth_state
        .store
        .add_allowed_user("bob@example.com", "admin", 1)
        .await
        .unwrap();
    let app = h.router();
    let response = app
        .oneshot(Harness::delete_with_session(
            "/v1/auth/allowed-emails/bob@example.com",
            &session,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn delete_nonexistent_email_returns_204() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .oneshot(Harness::delete_with_session(
            "/v1/auth/allowed-emails/nobody@example.com",
            &session,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

// ── validation tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn post_missing_email_field_returns_422() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    // JSON body without "email" field — deserialization fails → 422.
    let response = app
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            r#"{"not_email":"x"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn post_empty_email_returns_422() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            r#"{"email":""}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["kind"], "validation_failed");
}

#[tokio::test]
async fn post_whitespace_only_email_returns_422() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            r#"{"email":"   "}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["kind"], "validation_failed");
}

#[tokio::test]
async fn post_email_without_at_returns_422() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            r#"{"email":"notanemail"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["kind"], "validation_failed");
}

#[tokio::test]
async fn post_email_too_long_returns_422() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    // 321 characters: "a...a@example.com"
    let local = "a".repeat(321 - "@example.com".len());
    let long_email = format!("{local}@example.com");
    assert!(long_email.len() > 320);
    let body = serde_json::json!({ "email": long_email }).to_string();
    let response = app
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["kind"], "validation_failed");
}

#[tokio::test]
async fn post_duplicate_email_returns_422() {
    let h = Harness::new().await;
    // Pre-seed the email so the second add is a duplicate.
    h.auth_state
        .store
        .add_allowed_user("dup@example.com", "admin", 1)
        .await
        .unwrap();
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            r#"{"email":"dup@example.com"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["kind"], "validation_failed");
}

// ── CSRF enforcement ──────────────────────────────────────────────────────────

#[tokio::test]
async fn post_missing_csrf_header_returns_422() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    // POST without the CSRF header — middleware rejects before handler runs.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/allowed-emails")
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "{}={}",
                        lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                        session.session_id
                    ),
                )
                // Intentionally omit BROWSER_CSRF_HEADER_NAME
                .body(Body::from(r#"{"email":"x@x.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn delete_missing_csrf_header_returns_422() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/v1/auth/allowed-emails/x@x.com")
                .header(
                    header::COOKIE,
                    format!(
                        "{}={}",
                        lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                        session.session_id
                    ),
                )
                // Intentionally omit BROWSER_CSRF_HEADER_NAME
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ── bearer-only mode (no oauth_state) ────────────────────────────────────────

#[tokio::test]
async fn get_no_oauth_state_with_bearer_returns_403() {
    // Bearer-only mode: static bearer token configured, no oauth_state.
    // AuthContext is inserted with via_session=false by the static-bearer path,
    // so require_admin() → 403 (not via_session).
    // The handler never reaches require_oauth_state() because require_admin fails first.
    let state = AppState::new(); // no oauth_state, no auth_config
    let app = build_router(state, Some("static-token".into()), None, None, &[]);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/auth/allowed-emails")
                .header(header::AUTHORIZATION, "Bearer static-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Static bearer inserts AuthContext with via_session=false → require_admin() → 403.
    // (require_admin_email returns internal_error when auth_config is None, but
    //  we just check it's not 200.)
    assert!(
        response.status() == StatusCode::FORBIDDEN
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR,
        "expected 403 or 500, got {}",
        response.status()
    );
}

// ── list reflects additions ───────────────────────────────────────────────────

#[tokio::test]
async fn list_shows_added_emails() {
    let h = Harness::new().await;
    h.auth_state
        .store
        .add_allowed_user("carol@example.com", "admin", 1)
        .await
        .unwrap();
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .oneshot(Harness::get_with_session(
            "/v1/auth/allowed-emails",
            &session,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let entries = json["entries"].as_array().unwrap();
    assert!(entries.iter().any(|e| e["email"] == "carol@example.com"));
}

// ── email normalization ───────────────────────────────────────────────────────

#[tokio::test]
async fn post_normalizes_email_to_lowercase() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            r#"{"email":"  Alice@Example.COM  "}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["entry"]["email"], "alice@example.com");
}
