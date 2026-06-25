//! Service config + upstream add/update persistence tests.

use std::collections::BTreeSet;

use crate::gateway::config::load_gateway_config;
use crate::gateway::config_mutation::read_env_values;
use labby_runtime::gateway_config::{VirtualServerConfig, VirtualServerSurfacesConfig};

use super::*;

// CWE-532 secret-redaction guard, re-fixtured post-gateway-pivot.
//
// Why not the original manager end-to-end path: `set_service_config` only accepts a
// service that `registry::service_meta` resolves to a `PluginMeta`, and post-pivot
// that arm resolves ONLY `deploy` — which declares zero env fields. No
// `service_meta`-resolvable service exposes a `secret: true` field, so the secret
// branch of `service_config_view` is unreachable through `set_service_config`
// without a production change (adding e.g. `acp` to `service_meta`).
//
// Instead we exercise the actual redaction unit — `service_config_view`, the
// projection that `set_service_config` returns verbatim — directly against the kept
// `acp` service's real `PluginMeta`, which declares both a secret field
// (`LAB_ACP_HMAC_SECRET`, `secret: true`) and a non-secret one (`LAB_ACP_DB`). This
// is the function that enforces the redaction contract; pinning it here keeps the
// CWE-532 guard live in CI.
#[test]
fn service_config_get_redacts_secret_values() {
    let mut values = HashMap::new();
    values.insert("LAB_ACP_DB".to_string(), "/tmp/acp.db".to_string());
    values.insert(
        "LAB_ACP_HMAC_SECRET".to_string(),
        "super-secret".to_string(),
    );

    let config = crate::gateway::projection::service_config_view(&labby_apis::acp::META, &values);

    let secret = config
        .fields
        .iter()
        .find(|field| field.name == "LAB_ACP_HMAC_SECRET")
        .expect("secret field");
    assert!(secret.present);
    assert!(secret.secret);
    assert_eq!(
        secret.value_preview, None,
        "secret values must never be echoed back in a config read (CWE-532)"
    );

    // The non-secret companion field IS previewed — redaction is targeted, not a
    // blanket suppression of every field value.
    let non_secret = config
        .fields
        .iter()
        .find(|field| field.name == "LAB_ACP_DB")
        .expect("non-secret field");
    assert!(non_secret.present);
    assert!(!non_secret.secret);
    assert_eq!(non_secret.value_preview.as_deref(), Some("/tmp/acp.db"));
}

// Re-fixtured post-gateway-pivot via `service_config_view` directly against the
// kept `acp` `PluginMeta` (see `service_config_get_redacts_secret_values` for why
// the manager end-to-end path isn't usable). An empty value for a declared field
// must be reported as not-present and never previewed.
#[test]
fn service_config_get_treats_empty_values_as_not_present() {
    let mut values = HashMap::new();
    values.insert("LAB_ACP_HMAC_SECRET".to_string(), "token".to_string());
    values.insert("LAB_ACP_DB".to_string(), String::new());

    let config = crate::gateway::projection::service_config_view(&labby_apis::acp::META, &values);

    let db = config
        .fields
        .iter()
        .find(|field| field.name == "LAB_ACP_DB")
        .expect("db field");
    assert!(!db.present);
    assert_eq!(db.value_preview, None);
}

// CANNOT be re-fixtured without production-code changes (out of test-only scope).
// This test asserts `configured == false` when a *required* field is missing, but
// post-gateway-pivot NO surviving/kept service declares any `required_env` (acp,
// stash, deploy, setup, doctor, marketplace all have `required_env: &[]`). With no
// required fields, `service_config_view` reports `configured: true` unconditionally,
// so the assertion can never hold. Re-enabling this requires either a kept service
// that declares a required env var, or a synthetic `PluginMeta` reachable through
// `registered_service_meta` (which resolves via the static `service_meta` table) —
// both are production-code changes. Leaving ignored per the restoration spec.
#[tokio::test]
#[ignore = "no kept service declares required_env post-pivot; re-fixturing needs a prod PluginMeta change"]
async fn service_config_get_marks_service_unconfigured_when_required_fields_are_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    let mut values = BTreeMap::new();
    values.insert("PLEX_TOKEN".to_string(), "token".to_string());

    let config = manager
        .set_service_config("deploy", &values)
        .await
        .expect("set service config");

    assert!(
        !config.configured,
        "plex should remain unconfigured until every required field is present"
    );
}

// Re-fixtured post-gateway-pivot via `service_config_view` directly against the
// kept `acp` `PluginMeta`. acp declares no required env (only optional), so the
// all-required-present predicate holds and the service reports `configured == true`
// once its fields are populated. Exercises the `configured == true` branch of
// `service_config_view` for a real registered service.
#[test]
fn service_config_get_marks_service_configured_when_required_fields_are_present() {
    let mut values = HashMap::new();
    values.insert("LAB_ACP_DB".to_string(), "/tmp/acp.db".to_string());
    values.insert("LAB_ACP_HMAC_SECRET".to_string(), "token".to_string());

    let config = crate::gateway::projection::service_config_view(&labby_apis::acp::META, &values);

    assert!(config.configured);
}

#[tokio::test]
async fn add_with_bearer_token_value_writes_env_and_references_generated_env_var() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let env_path = dir.path().join(".env");
    let manager =
        GatewayManager::new(path, GatewayRuntimeHandle::default()).with_env_path(env_path);

    let gateway = manager
        .add(
            UpstreamConfig {
                enabled: true,
                name: "github".to_string(),
                url: Some("https://api.githubcopilot.com/mcp/".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            },
            Some("ghp_secret".to_string()),
            None,
            None,
        )
        .await
        .expect("add gateway");

    assert_eq!(
        gateway.config.bearer_token_env.as_deref(),
        Some("LAB_GW_GITHUB_AUTH_HEADER")
    );

    let values = read_env_values(&dir.path().join(".env")).expect("read env");
    assert_eq!(
        values.get("LAB_GW_GITHUB_AUTH_HEADER").map(String::as_str),
        Some("Bearer ghp_secret")
    );
}

#[tokio::test]
async fn concurrent_gateway_adds_persist_both_gateways() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());

    let first = manager.clone();
    let second = manager.clone();
    let (first_result, second_result) = tokio::join!(
        first.add(fixture_stdio_upstream("alpha"), None, None, None),
        second.add(fixture_stdio_upstream("bravo"), None, None, None),
    );

    first_result.expect("add alpha");
    second_result.expect("add bravo");

    let persisted = load_gateway_config(&path).expect("load persisted config");
    let names = persisted
        .upstream
        .iter()
        .map(|upstream| upstream.name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(names, BTreeSet::from(["alpha", "bravo"]));
}

// Re-fixtured post-gateway-pivot: the virtual server is backed by the kept
// `deploy` service (no plex/radarr env fixtures involved). Asserts a concurrent
// root config mutation and a virtual-server surface mutation both persist.
#[tokio::test]
async fn concurrent_root_and_virtual_server_mutations_both_persist() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default())
        .with_builtin_service_registry(deploy_known_registry());
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            virtual_servers: vec![VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    cli: false,
                    api: false,
                    mcp: false,
                    webui: false,
                },
                mcp_policy: None,
            }],
            ..GatewayConfig::default()
        })
        .await;

    let root = manager.clone();
    let virtual_server = manager.clone();
    let (root_result, virtual_result) = tokio::join!(
        root.set_code_mode_config(
            CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            None,
            None,
        ),
        virtual_server.set_virtual_server_surface("deploy", "mcp", true),
    );

    root_result.expect("set root code mode config");
    virtual_result.expect("set virtual server surface");

    let persisted = load_gateway_config(&path).expect("load persisted config");
    assert!(persisted.code_mode.enabled);
    let plex = persisted
        .virtual_servers
        .iter()
        .find(|server| server.id == "deploy")
        .expect("plex virtual server persisted");
    assert!(plex.surfaces.mcp);
}

// Store-seam env persistence guard (rewritten in the gateway extraction).
//
// The host-owned service-client cache + `refresh_count()` instrumentation moved
// out of `lab-gateway` into `lab`'s `LabConfigStore`, so the manager no longer
// exposes `with_service_clients`. The credential-write half of that contract is
// now owned by the `GatewayConfigStore` seam: env vars are persisted through
// `store.persist_*`, exercised here against the default `FsGatewayConfigStore`
// (injected via `with_env_path`). This asserts a real env-credential write lands
// in the backing `.env` file through the store seam.
#[tokio::test]
async fn bearer_token_credential_write_persists_through_store_seam() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let env_path = dir.path().join(".env");
    let manager =
        GatewayManager::new(path, GatewayRuntimeHandle::default()).with_env_path(env_path.clone());

    manager
        .add(
            fixture_stdio_upstream("plex"),
            Some("plex-token".to_string()),
            None,
            None,
        )
        .await
        .expect("add gateway with bearer token");

    let values = read_env_values(&env_path).expect("read env values written via store seam");
    assert_eq!(
        values.get("LAB_GW_PLEX_AUTH_HEADER").map(String::as_str),
        Some("Bearer plex-token"),
        "bearer credential must be persisted to the .env file through the store seam"
    );
}
