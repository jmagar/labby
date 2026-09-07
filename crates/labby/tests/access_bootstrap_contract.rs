#![cfg(feature = "gateway")]

#[path = "support/lib.rs"]
mod support;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use labby::api::{router::build_router, state::AppState};
use tower::ServiceExt as _;

use support::{LiveLabbyBuilder, isolated_command};

const PROJECT_ID: &str = "bootstrap-default";
const LOADOUT_ID: &str = "production";
const ROUTE_ID: &str = "operator";
const RESOURCE: &str = "https://mcp.example.test/operator";

fn published_policy_config(scopes: &[&str]) -> String {
    let scopes = scopes
        .iter()
        .map(|scope| format!("\"{scope}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
[[loadouts]]
name = "{LOADOUT_ID}"
upstreams = []
services = ["gateway"]

[[protected_mcp_routes]]
name = "{ROUTE_ID}"
enabled = true
public_host = "mcp.example.test"
public_path = "/operator"
scopes = [{scopes}]

[protected_mcp_routes.target]
kind = "gateway_subset"
project_id = "{PROJECT_ID}"
loadout = "{LOADOUT_ID}"
"#
    )
}

fn installation_command(root: &std::path::Path) -> std::process::Command {
    let home = root.join("home");
    let labby_home = root.join("labby-home");
    for path in [&home, &labby_home, &root.join("tmp"), &root.join("logs")] {
        std::fs::create_dir_all(path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let mut command = isolated_command(&home);
    command
        .env("LABBY_HOME", labby_home)
        .env("LABBY_LOG_DIR", root.join("logs"))
        .env("TMPDIR", root.join("tmp"));
    command
}

fn prepare(root: &std::path::Path) -> serde_json::Value {
    let output = installation_command(root)
        .args(["setup", "access-bootstrap", "prepare", "--proof-file"])
        .arg(root.join("proof.json"))
        .arg("--credential-file")
        .arg(root.join("credential.txt"))
        .args([
            "--organization-name",
            "Hermetic Org",
            "--project-name",
            "Hermetic Project",
            "--subject",
            "owner@example.test",
            "--loadout-id",
            LOADOUT_ID,
            "--route-id",
            ROUTE_ID,
            "--resource",
            RESOURCE,
            "--scope",
            "lab:read",
            "--scope",
            "lab:admin",
            "--ttl",
            "300",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "prepare failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn proof() -> String {
    format!("lby_bp_v1_proof-id_{}", "A".repeat(43))
}

fn request(path: &str, body: &str) -> Request<Body> {
    let mut request = Request::post(path)
        .header(header::HOST, "127.0.0.1:8765")
        .header("x-labby-bootstrap-proof", proof())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:42000".parse::<std::net::SocketAddr>().unwrap(),
    ));
    request
}

#[tokio::test]
async fn unavailable_or_unknown_proof_is_uniform_hardened_and_secret_free() {
    let router = build_router(AppState::new(), None, None, None, &[]);
    for path in ["/auth/bootstrap/status", "/auth/bootstrap/cleanup"] {
        let response = router
            .clone()
            .oneshot(request(path, r#"{"prepare_id":"prepare-id"}"#))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        assert!(
            !response
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(
            text,
            r#"{"error":{"kind":"access_denied","message":"bootstrap request denied"}}"#
        );
        assert!(!text.contains(&proof()));
        assert!(!text.contains("prepare-id"));
    }
}

#[tokio::test]
async fn forwarded_mixed_authority_and_oversized_inputs_fail_before_semantics() {
    let router = build_router(AppState::new(), None, None, None, &[]);
    let mut forwarded = request("/auth/bootstrap/status", r#"{"prepare_id":"prepare-id"}"#);
    forwarded
        .headers_mut()
        .insert("forwarded", "for=127.0.0.1".parse().unwrap());
    let mut mixed = request("/auth/bootstrap/status", r#"{"prepare_id":"prepare-id"}"#);
    mixed
        .headers_mut()
        .insert(header::ORIGIN, "http://localhost:8765".parse().unwrap());
    let oversized = request("/auth/bootstrap/status", &"x".repeat(9 * 1024));
    for request in [forwarded, mixed, oversized] {
        let response = router.clone().oneshot(request).await.unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::PAYLOAD_TOO_LARGE
        ));
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
    }
}

#[tokio::test]
async fn replay_and_concurrency_never_turn_denial_into_success() {
    let router = build_router(AppState::new(), None, None, None, &[]);
    let mut tasks = Vec::new();
    for _ in 0..24 {
        let router = router.clone();
        tasks.push(tokio::spawn(async move {
            router
                .oneshot(request(
                    "/auth/bootstrap/status",
                    r#"{"prepare_id":"prepare-id"}"#,
                ))
                .await
                .unwrap()
                .status()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn shipped_bootstrap_survives_restart_and_cleanup_tombstones_all_access() {
    let parent = std::env::temp_dir().join("labby-live-e2e");
    std::fs::create_dir_all(&parent).unwrap();
    let owned = tempfile::Builder::new()
        .prefix("bootstrap-contract-")
        .tempdir_in(&parent)
        .unwrap();
    let root = owned.path().canonicalize().unwrap();
    let prepared = prepare(&root);
    let prepare_id = prepared["prepare_id"].as_str().unwrap().to_owned();
    let bundle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("proof.json")).unwrap()).unwrap();
    let proof = bundle["proof"].as_str().unwrap().to_owned();
    let manifest = bundle["manifest"].clone();
    let credential = std::fs::read_to_string(root.join("credential.txt")).unwrap();
    let mut daemon = LiveLabbyBuilder::new()
        .existing_root(&root)
        .config(published_policy_config(&["lab:admin", "lab:read"]))
        .env("LABBY_WEB_UI_AUTH_DISABLED", "false")
        .env(
            "LABBY_MCP_HTTP_TOKEN",
            format!("contract-static-{}", ulid::Ulid::new()),
        )
        .env("LABBY_PUBLIC_URL", "https://lab.example.test")
        .start()
        .await
        .unwrap();
    let base = daemon.connection().base_url.clone();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let status = client
        .post(format!("{base}/auth/bootstrap/status"))
        .header("x-labby-bootstrap-proof", &proof)
        .json(&serde_json::json!({"prepare_id": prepare_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK, "proof journal not visible");

    let consumed = client
        .post(format!("{base}/auth/bootstrap/consume"))
        .header("x-labby-bootstrap-proof", &proof)
        .json(&manifest)
        .send()
        .await
        .unwrap();
    let consumed_status = consumed.status();
    let consumed_body = consumed.text().await.unwrap();
    assert_eq!(
        consumed_status,
        StatusCode::OK,
        "consume body={consumed_body}; diagnostics={}",
        daemon.diagnostics(Some("bootstrap consume denied"))
    );

    let introspect = || {
        client
            .get(format!("{base}/v1/access/credentials/self"))
            .bearer_auth(&credential)
            .send()
    };
    daemon.restart().await.unwrap();
    let after_restart = introspect().await.unwrap();
    assert_eq!(
        after_restart.status(),
        StatusCode::OK,
        "{}",
        daemon.diagnostics(Some("credential unavailable after restart"))
    );

    let session = client
        .post(format!("{base}/auth/local-session"))
        .header(header::HOST, "lab.example.test")
        .header(header::ORIGIN, "https://lab.example.test")
        .bearer_auth(&credential)
        .send()
        .await
        .unwrap();
    let session_status = session.status();
    let cookie = session
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_owned);
    let session_body = session.text().await.unwrap();
    assert_eq!(
        session_status,
        StatusCode::CREATED,
        "local-session body={session_body}; diagnostics={}",
        daemon.diagnostics(Some("local session creation failed"))
    );
    let cookie = cookie.expect("successful local session must set its opaque host-only cookie");

    let reload = client
        .post(format!("{base}/v1/gateway"))
        .bearer_auth(&credential)
        .json(&serde_json::json!({
            "action": "gateway.reload",
            "params": {"confirm": true}
        }))
        .send()
        .await
        .unwrap();
    assert!(reload.status().is_success(), "reload: {reload:?}");
    assert_eq!(
        introspect().await.unwrap().status(),
        StatusCode::OK,
        "observational gateway reload must preserve the same durable policy epoch"
    );

    daemon.restart().await.unwrap();
    std::fs::write(
        root.join("labby-home/config.toml"),
        published_policy_config(&["lab:admin"]),
    )
    .unwrap();
    daemon.restart().await.unwrap();
    assert_eq!(
        introspect().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    let wrong_route = published_policy_config(&["lab:read"]).replace(
        &format!("name = \"{ROUTE_ID}\"\nenabled"),
        "name = \"other-route\"\nenabled",
    );
    std::fs::write(root.join("labby-home/config.toml"), wrong_route).unwrap();
    daemon.restart().await.unwrap();
    assert_eq!(
        introspect().await.unwrap().status(),
        StatusCode::BAD_GATEWAY
    );

    std::fs::write(
        root.join("labby-home/config.toml"),
        published_policy_config(&["lab:read"]),
    )
    .unwrap();
    daemon.restart().await.unwrap();
    assert_eq!(
        introspect().await.unwrap().status(),
        StatusCode::UNAUTHORIZED,
        "A-B-A must not resurrect a credential bound to the first A epoch"
    );

    let cleanup = client
        .post(format!("{base}/auth/bootstrap/cleanup"))
        .header("x-labby-bootstrap-proof", &proof)
        .json(&serde_json::json!({"prepare_id": prepare_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(cleanup.status(), StatusCode::OK);
    assert!(!root.join("proof.json").exists());
    assert!(!root.join("credential.txt").exists());

    daemon.restart().await.unwrap();
    assert_eq!(
        introspect().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let session_after_tombstone = client
        .get(format!("{base}/v1/access/credentials/self"))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(session_after_tombstone.status(), StatusCode::UNAUTHORIZED);

    let cleanup_result = daemon.finish().await;
    assert!(cleanup_result.is_clean(), "{cleanup_result:?}");
}
