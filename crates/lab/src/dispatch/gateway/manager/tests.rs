//! Shared fixtures for the `GatewayManager` test suite. The tests themselves
//! live in the `tests/` child modules, split by concern; each child does
//! `use super::*;` to inherit these fixtures and imports.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use base64::Engine as _;
use lab_auth::sqlite::SqliteStore;
use rmcp::transport::{AuthClient, AuthorizationManager};

use crate::config::{
    CodeModeConfig, ImportSource, LabConfig, ProtectedMcpRouteConfig, UpstreamConfig,
    UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};
use crate::dispatch::gateway::discovery::DiscoveredServer;
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::dispatch::upstream::types::{
    ToolExposurePolicy, UpstreamEntry, UpstreamHealth, UpstreamTool,
};
use crate::oauth::upstream::encryption::{EncryptionKey, load_key};

use super::{GatewayManager, GatewayRuntimeHandle};

mod cleanup;
mod code_mode;
mod config_ops;
mod imports;
mod inspection;
mod lifecycle;
mod oauth;
mod views;
mod virtual_servers;

async fn dummy_auth_client() -> Arc<AuthClient<reqwest::Client>> {
    let manager = AuthorizationManager::new("http://localhost")
        .await
        .expect("authorization manager");
    Arc::new(AuthClient::new(reqwest::Client::new(), manager))
}

async fn fixture_oauth_resources(dir: &tempfile::TempDir) -> (SqliteStore, EncryptionKey, String) {
    let sqlite = SqliteStore::open(dir.path().join("auth.sqlite"))
        .await
        .expect("sqlite store");
    let key_b64 = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
    let key = load_key(&key_b64).expect("encryption key");
    (
        sqlite,
        key,
        "https://lab.example.com/v1/upstream-oauth/callback".to_string(),
    )
}

fn fixture_stdio_upstream(name: &str) -> UpstreamConfig {
    UpstreamConfig {
        enabled: true,
        name: name.to_string(),
        url: None,
        bearer_token_env: None,
        command: Some("npx".to_string()),
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
    }
}

fn fixture_http_upstream(name: &str) -> UpstreamConfig {
    UpstreamConfig {
        enabled: true,
        name: name.to_string(),
        url: Some("http://127.0.0.1:9/mcp".to_string()),
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
    }
}

fn fixture_import_source(server_name: &str) -> ImportSource {
    ImportSource::new(
        "codex",
        "/home/alice/.codex/config.toml",
        "2026-05-15T00:00:00Z",
    )
    .with_server_name(server_name)
}

fn fixture_discovered_http(name: &str) -> DiscoveredServer {
    let mut spec = fixture_http_upstream(name);
    spec.enabled = false;
    spec.imported_from = Some(fixture_import_source(name));
    DiscoveredServer {
        name: name.to_string(),
        spec,
        source_client: "codex".to_string(),
        source_path: "/home/alice/.codex/config.toml".to_string(),
        env_key_count: 0,
    }
}

fn fixture_oauth_upstream(name: &str, url: &str) -> UpstreamConfig {
    let mut upstream = fixture_http_upstream(name);
    upstream.url = Some(url.to_string());
    upstream.oauth = Some(UpstreamOauthConfig {
        mode: UpstreamOauthMode::AuthorizationCodePkce,
        registration: UpstreamOauthRegistration::Dynamic,
        scopes: None,
        prefer_client_metadata_document: None,
    });
    upstream
}

async fn code_mode_manager_with_pool(
    upstream: UpstreamConfig,
) -> (GatewayManager, Arc<UpstreamPool>) {
    code_mode_manager_with_upstreams(vec![upstream]).await
}

async fn code_mode_manager_with_upstreams(
    upstream: Vec<UpstreamConfig>,
) -> (GatewayManager, Arc<UpstreamPool>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(path, runtime);
    manager
        .seed_config(LabConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream,
            ..LabConfig::default()
        })
        .await;
    (manager, pool)
}

fn healthy_entry_with_tool(upstream: &str, tool_name: &str) -> UpstreamEntry {
    let upstream_name: Arc<str> = Arc::from(upstream);
    let schema = Arc::new(serde_json::Map::new());
    let tool = rmcp::model::Tool::new(
        tool_name.to_string(),
        format!("{tool_name} description"),
        schema,
    );
    let upstream_tool = UpstreamTool {
        tool,
        input_schema: None,
        output_schema: None,
        upstream_name: Arc::clone(&upstream_name),
        destructive: false,
    };
    fixture_upstream_entry(
        upstream,
        HashMap::from([(tool_name.to_string(), upstream_tool)]),
    )
}

fn fixture_upstream_entry(upstream: &str, tools: HashMap<String, UpstreamTool>) -> UpstreamEntry {
    UpstreamEntry {
        name: Arc::from(upstream),
        tools,
        exposure_policy: ToolExposurePolicy::All,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
        tool_health: UpstreamHealth::Healthy,
        prompt_health: UpstreamHealth::Healthy,
        resource_health: UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
    }
}

fn fixture_protected_route(name: &str) -> ProtectedMcpRouteConfig {
    ProtectedMcpRouteConfig {
        name: name.to_string(),
        enabled: true,
        public_host: "mcp.tootie.tv".to_string(),
        public_path: "/syslog".to_string(),
        upstream: None,
        backend_url: "http://100.88.16.79:3100".to_string(),
        backend_mcp_path: "/mcp".to_string(),
        scopes: vec!["mcp:read".to_string(), "mcp:write".to_string()],
        health_path: Some("/health".to_string()),
    }
}
