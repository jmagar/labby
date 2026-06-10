//! Service config + upstream add/update persistence tests.

use std::collections::BTreeSet;

use crate::config::{VirtualServerConfig, VirtualServerSurfacesConfig};
use crate::dispatch::clients::SharedServiceClients;
use crate::dispatch::gateway::config::load_gateway_config;
use crate::dispatch::gateway::config_mutation::read_env_values;

use super::*;

#[tokio::test]
#[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
async fn service_config_get_redacts_secret_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    let mut values = BTreeMap::new();
    values.insert("PLEX_URL".to_string(), "http://127.0.0.1:32400".to_string());
    values.insert("PLEX_TOKEN".to_string(), "super-secret".to_string());

    let config = manager
        .set_service_config("deploy", &values)
        .await
        .expect("set service config");

    let token = config
        .fields
        .iter()
        .find(|field| field.name == "PLEX_TOKEN")
        .expect("token field");
    assert!(token.present);
    assert!(token.secret);
    assert_eq!(token.value_preview, None);
}

#[tokio::test]
#[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
async fn service_config_get_treats_empty_values_as_not_present() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    let mut values = BTreeMap::new();
    values.insert("OPENAI_API_KEY".to_string(), "token".to_string());
    values.insert("OPENAI_URL".to_string(), String::new());

    let config = manager
        .set_service_config("setup", &values)
        .await
        .expect("set service config");

    let url = config
        .fields
        .iter()
        .find(|field| field.name == "OPENAI_URL")
        .expect("url field");
    assert!(!url.present);
    assert_eq!(url.value_preview, None);
}

#[tokio::test]
#[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
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

#[tokio::test]
#[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
async fn service_config_get_marks_service_configured_when_required_fields_are_present() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    let mut values = BTreeMap::new();
    values.insert("OPENAI_API_KEY".to_string(), "token".to_string());
    values.insert(
        "OPENAI_URL".to_string(),
        "https://api.openai.com/v1".to_string(),
    );

    let config = manager
        .set_service_config("setup", &values)
        .await
        .expect("set service config");

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

#[tokio::test]
#[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
async fn concurrent_root_and_virtual_server_mutations_both_persist() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .seed_config(LabConfig {
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
            ..LabConfig::default()
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

#[tokio::test]
#[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
async fn service_clients_refresh_after_service_config_update() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let shared_clients =
        SharedServiceClients::from_clients(crate::dispatch::clients::ServiceClients::default());
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default())
        .with_service_clients(shared_clients.clone());

    let mut values = BTreeMap::new();
    values.insert("PLEX_URL".to_string(), "http://127.0.0.1:32400".to_string());
    values.insert("PLEX_TOKEN".to_string(), "token".to_string());

    manager
        .set_service_config("deploy", &values)
        .await
        .expect("set service config");

    assert_eq!(shared_clients.refresh_count(), 1);
}
