//! Integration tests for the upstream OAuth manager.
//!
//! These tests stand up a wiremock-backed OAuth authorization server and drive
//! `UpstreamOauthManager` against it. They exist to pin the wire-level behavior
//! of our outbound OAuth client — specifically the `resource` indicator on
//! authorize, issuer binding, S256 enforcement, and CIMD registration.

use std::io;
use std::sync::{Arc, Mutex};

use base64::Engine;
use lab_auth::sqlite::SqliteStore;
use lab_auth::types::UpstreamOauthCredentialRow;
use labby::config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
    canonicalize_upstream_url,
};
use labby::oauth::upstream::encryption::load_key;
use labby::oauth::upstream::manager::UpstreamOauthManager;
use labby::oauth::upstream::types::OauthError;
use serde_json::json;
use tempfile::TempDir;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static TRACING_TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl<'a> MakeWriter<'a> for SharedBuf {
    type Writer = SharedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedWriter(Arc::clone(&self.0))
    }
}

struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn captured_logs(buf: &SharedBuf) -> String {
    String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
}

// ---------- harness ----------

struct Harness {
    mock: MockServer,
    _tmp: TempDir,
    sqlite: SqliteStore,
    key: labby::oauth::upstream::encryption::EncryptionKey,
}

impl Harness {
    async fn new() -> Self {
        let mock = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let sqlite = SqliteStore::open(tmp.path().join("auth.sqlite"))
            .await
            .expect("sqlite open");
        let key_b64 = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        let key = load_key(&key_b64).expect("key");
        Self {
            mock,
            _tmp: tmp,
            sqlite,
            key,
        }
    }

    fn upstream_url(&self) -> String {
        self.mock.uri()
    }

    fn as_url(&self) -> String {
        self.mock.uri()
    }

    /// Mount an AS metadata document with the given fields. `issuer` and
    /// `code_challenge_methods_supported` are nullable so we can exercise
    /// degenerate configurations.
    async fn mount_metadata(
        &self,
        issuer: Option<&str>,
        methods: Option<&[&str]>,
        endpoint_override: Option<(&str, &str)>,
    ) {
        let (auth_ep, token_ep) = endpoint_override
            .map(|(a, t)| (a.to_string(), t.to_string()))
            .unwrap_or_else(|| {
                (
                    format!("{}/authorize", self.as_url()),
                    format!("{}/token", self.as_url()),
                )
            });
        let mut body = json!({
            "authorization_endpoint": auth_ep,
            "token_endpoint": token_ep,
        });
        if let Some(iss) = issuer {
            body["issuer"] = json!(iss);
        }
        if let Some(m) = methods {
            body["code_challenge_methods_supported"] = json!(m);
        }
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.mock)
            .await;
    }

    async fn mount_metadata_with_registration(&self) {
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": self.as_url(),
                "authorization_endpoint": format!("{}/authorize", self.as_url()),
                "token_endpoint": format!("{}/token", self.as_url()),
                "registration_endpoint": format!("{}/register", self.as_url()),
                "code_challenge_methods_supported": ["S256"]
            })))
            .mount(&self.mock)
            .await;
    }

    /// Mock the resource-metadata probe to 404 so rmcp falls through to
    /// direct AS discovery at `.well-known/oauth-authorization-server`.
    async fn mount_no_resource_metadata(&self) {
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&self.mock)
            .await;
    }

    async fn mount_token_endpoint(&self) {
        self.mount_token_endpoint_with_expires(3600).await;
    }

    async fn mount_token_endpoint_with_expires(&self, expires_in: u64) {
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "access-xyz",
                "token_type": "Bearer",
                "expires_in": expires_in,
                "refresh_token": "refresh-xyz",
                "scope": "read"
            })))
            .mount(&self.mock)
            .await;
    }

    fn upstream_cfg(&self, registration: UpstreamOauthRegistration) -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: Some(self.upstream_url()),
            bearer_token_env: None,
            command: None,
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: Some(UpstreamOauthConfig {
                mode: UpstreamOauthMode::AuthorizationCodePkce,
                registration,
                scopes: Some(vec!["read".into()]),
            }),
            imported_from: None,
            tool_search: labby::config::ToolSearchConfig::default(),
        }
    }

    fn manager(&self, cfg: UpstreamConfig) -> UpstreamOauthManager {
        UpstreamOauthManager::new(
            self.sqlite.clone(),
            self.key.clone(),
            cfg,
            "https://lab.example/v1/gateway/oauth/callback".into(),
        )
    }
}

fn preregistered() -> UpstreamOauthRegistration {
    UpstreamOauthRegistration::Preregistered {
        client_id: "lab-client".into(),
        client_secret_env: None,
    }
}

fn dynamic() -> UpstreamOauthRegistration {
    UpstreamOauthRegistration::Dynamic
}

// ---------- tests ----------

#[tokio::test]
async fn canonical_url_strips_default_port_and_lowercases_host() {
    assert_eq!(
        canonicalize_upstream_url("https://Example.COM:443/mcp").unwrap(),
        "https://example.com/mcp"
    );
    assert_eq!(
        canonicalize_upstream_url("http://Example.COM:80/mcp/").unwrap(),
        "http://example.com/mcp/"
    );
}

#[tokio::test]
async fn missing_code_challenge_methods_returns_unsupported() {
    let h = Harness::new().await;
    h.mount_no_resource_metadata().await;
    h.mount_metadata(Some(&h.as_url()), None, None).await;
    let m = h.manager(h.upstream_cfg(preregistered()));

    let err = m.begin_authorization("alice").await.unwrap_err();
    assert!(
        matches!(err, OauthError::UnsupportedMethod(_)),
        "expected UnsupportedMethod, got {err:?}"
    );
}

#[tokio::test]
async fn plain_pkce_only_returns_unsupported() {
    let h = Harness::new().await;
    h.mount_no_resource_metadata().await;
    h.mount_metadata(Some(&h.as_url()), Some(&["plain"]), None)
        .await;
    let m = h.manager(h.upstream_cfg(preregistered()));

    let err = m.begin_authorization("alice").await.unwrap_err();
    assert!(
        matches!(err, OauthError::UnsupportedMethod(_)),
        "expected UnsupportedMethod, got {err:?}"
    );
}

#[tokio::test]
async fn authorize_url_carries_canonical_resource_indicator() {
    let h = Harness::new().await;
    h.mount_no_resource_metadata().await;
    h.mount_metadata(Some(&h.as_url()), Some(&["S256"]), None)
        .await;
    let expected_resource = canonicalize_upstream_url(&h.upstream_url()).unwrap();
    let m = h.manager(h.upstream_cfg(preregistered()));

    let begin = m.begin_authorization("alice").await.expect("begin");
    let u = Url::parse(&begin.authorization_url).expect("authorize url parses");
    let resource = u
        .query_pairs()
        .find(|(k, _)| k == "resource")
        .map(|(_, v)| v.into_owned());
    assert_eq!(
        resource.as_deref(),
        Some(expected_resource.as_str()),
        "authorize url missing canonical resource indicator"
    );
    let method = u
        .query_pairs()
        .find(|(k, _)| k == "code_challenge_method")
        .map(|(_, v)| v.into_owned());
    assert_eq!(method.as_deref(), Some("S256"));
}

#[tokio::test]
async fn token_exchange_carries_canonical_resource_indicator() {
    let h = Harness::new().await;
    h.mount_no_resource_metadata().await;
    h.mount_metadata(Some(&h.as_url()), Some(&["S256"]), None)
        .await;
    h.mount_token_endpoint().await;
    let expected_resource = canonicalize_upstream_url(&h.upstream_url()).unwrap();
    let m = h.manager(h.upstream_cfg(preregistered()));

    let begin = m.begin_authorization("alice").await.expect("begin");
    let authorize_url = Url::parse(&begin.authorization_url).unwrap();
    let state = authorize_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .expect("state");
    m.complete_authorization_callback("alice", "fake-code", &state)
        .await
        .expect("exchange");

    let recorded = h.mock.received_requests().await.expect("record enabled");
    let token_req = recorded
        .iter()
        .find(|r| r.method.as_str() == "POST" && r.url.path() == "/token")
        .expect("token request recorded");
    let body = std::str::from_utf8(&token_req.body).expect("utf8 body");
    let resource_value = url::form_urlencoded::parse(body.as_bytes())
        .find(|(k, _)| k == "resource")
        .map(|(_, v)| v.into_owned());
    assert_eq!(
        resource_value.as_deref(),
        Some(expected_resource.as_str()),
        "token exchange body missing canonical resource indicator; body was: {body}"
    );
}

#[tokio::test]
async fn issuer_missing_returns_issuer_mismatch() {
    let h = Harness::new().await;
    h.mount_no_resource_metadata().await;
    h.mount_metadata(None, Some(&["S256"]), None).await;
    let m = h.manager(h.upstream_cfg(preregistered()));

    let err = m.begin_authorization("alice").await.unwrap_err();
    assert!(
        matches!(err, OauthError::IssuerMismatch(_)),
        "expected IssuerMismatch for missing issuer, got {err:?}"
    );
}

#[tokio::test]
async fn issuer_endpoint_host_mismatch_returns_issuer_mismatch() {
    let h = Harness::new().await;
    h.mount_no_resource_metadata().await;
    h.mount_metadata(
        Some("https://good.example"),
        Some(&["S256"]),
        Some((
            "https://evil.example/authorize",
            "https://evil.example/token",
        )),
    )
    .await;
    let m = h.manager(h.upstream_cfg(preregistered()));

    let err = m.begin_authorization("alice").await.unwrap_err();
    assert!(
        matches!(err, OauthError::IssuerMismatch(_)),
        "expected IssuerMismatch for endpoint host drift, got {err:?}"
    );
}

#[tokio::test]
async fn cimd_registration_uses_metadata_url_as_client_id() {
    let h = Harness::new().await;
    h.mount_no_resource_metadata().await;
    h.mount_metadata(Some(&h.as_url()), Some(&["S256"]), None)
        .await;
    let cimd_url = "https://lab.example/.well-known/oauth-client-metadata";
    let m = h.manager(
        h.upstream_cfg(UpstreamOauthRegistration::ClientMetadataDocument {
            url: cimd_url.into(),
        }),
    );

    let begin = m.begin_authorization("alice").await.expect("begin");
    let u = Url::parse(&begin.authorization_url).unwrap();
    let client_id = u
        .query_pairs()
        .find(|(k, _)| k == "client_id")
        .map(|(_, v)| v.into_owned());
    assert_eq!(client_id.as_deref(), Some(cimd_url));
}

#[tokio::test]
async fn dynamic_begin_authorization_reregisters_when_pending_client_is_stale() {
    let h = Harness::new().await;
    h.mount_no_resource_metadata().await;
    h.mount_metadata_with_registration().await;
    h.sqlite
        .save_dynamic_client_registration("test", "alice", "stale-client")
        .await
        .expect("seed stale dynamic registration");
    h.sqlite
        .upsert_upstream_oauth_credentials(UpstreamOauthCredentialRow {
            upstream_name: "test".into(),
            subject: "alice".into(),
            client_id: "stale-client".into(),
            granted_scopes_json: "[]".into(),
            token_blob: Vec::new(),
            token_blob_nonce: Vec::new(),
            token_received_at: 0,
            access_token_expires_at: 0,
            refresh_token_present: false,
        })
        .await
        .expect("seed stale stored credentials");
    Mock::given(method("POST"))
        .and(path("/register"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "client_id": "fresh-client",
            "redirect_uris": ["https://lab.example/v1/gateway/oauth/callback"],
            "token_endpoint_auth_method": "none"
        })))
        .mount(&h.mock)
        .await;
    let m = h.manager(h.upstream_cfg(dynamic()));

    let begin = m.begin_authorization("alice").await.expect("begin");
    let u = Url::parse(&begin.authorization_url).expect("authorize url parses");
    let client_id = u
        .query_pairs()
        .find(|(k, _)| k == "client_id")
        .map(|(_, v)| v.into_owned());
    let stored_client_id = h
        .sqlite
        .find_dynamic_client_registration("test", "alice")
        .await
        .expect("dynamic registration lookup");

    assert_eq!(client_id.as_deref(), Some("fresh-client"));
    assert_eq!(stored_client_id.as_deref(), Some("fresh-client"));
    let recorded = h.mock.received_requests().await.expect("record enabled");
    let register_count = recorded
        .iter()
        .filter(|request| request.method.as_str() == "POST" && request.url.path() == "/register")
        .count();
    assert_eq!(register_count, 1);
}

#[tokio::test]
async fn subject_lookup_survives_restart_for_saved_state() {
    let h = Harness::new().await;
    h.mount_no_resource_metadata().await;
    h.mount_metadata(Some(&h.as_url()), Some(&["S256"]), None)
        .await;
    let manager = h.manager(h.upstream_cfg(preregistered()));

    let begin = manager.begin_authorization("alice").await.expect("begin");
    let authorize_url = Url::parse(&begin.authorization_url).unwrap();
    let state = authorize_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .expect("state");

    let restarted_manager = h.manager(h.upstream_cfg(preregistered()));
    let subject = restarted_manager
        .subject_for_state(&state)
        .await
        .expect("lookup");
    assert_eq!(subject.as_deref(), Some("alice"));
}

#[tokio::test(flavor = "current_thread")]
async fn build_auth_client_logs_near_expiry_refresh_lifecycle_without_secrets() {
    let _tracing_lock = TRACING_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    let h = Harness::new().await;
    h.mount_no_resource_metadata().await;
    h.mount_metadata(Some(&h.as_url()), Some(&["S256"]), None)
        .await;
    h.mount_token_endpoint_with_expires(10).await;
    let m = h.manager(h.upstream_cfg(preregistered()));

    let begin = m.begin_authorization("alice").await.expect("begin");
    let authorize_url = Url::parse(&begin.authorization_url).unwrap();
    let state = authorize_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .expect("state");
    m.complete_authorization_callback("alice", "fake-code", &state)
        .await
        .expect("exchange");

    let _client = m.build_auth_client("alice").await.expect("auth client");

    drop(_guard);
    let logs = captured_logs(&buf);
    assert!(logs.contains("upstream oauth: access token nearing expiry"));
    assert!(logs.contains("upstream oauth: token refresh attempt"));
    assert!(logs.contains("upstream oauth: token refresh succeeded"));
    assert!(logs.contains("\"provider\":\"test\""));
    assert!(logs.contains("\"scope\":\"read\""));
    assert!(!logs.contains("access-xyz"), "access token leaked: {logs}");
    assert!(
        !logs.contains("refresh-xyz"),
        "refresh token leaked: {logs}"
    );
    assert!(
        !logs.contains("fake-code"),
        "authorization code leaked: {logs}"
    );
    assert!(!logs.contains(&state), "csrf state leaked: {logs}");
}
