//! Upstream OAuth manager/cache reconciliation tests across reloads.

use crate::gateway::config::write_gateway_config;
use lab_auth::upstream::cache::OauthClientCache;
use lab_auth::upstream::manager::UpstreamOauthManager;
use lab_runtime::gateway_config::{
    UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};

use super::*;

#[tokio::test]
async fn reload_evicts_removed_upstream_oauth_clients() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut kept_upstream = fixture_http_upstream("kept");
    kept_upstream.url = Some("https://fixture.example.com:7001".to_string());
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![kept_upstream],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let cache = OauthClientCache::new(Arc::new(dashmap::DashMap::new()));
    cache.insert_for_tests(
        "removed",
        "alice",
        "preregistered:client-a",
        dummy_auth_client().await,
    );

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default())
        .with_oauth_client_cache(cache.clone());
    let mut removed_upstream = fixture_http_upstream("removed");
    removed_upstream.url = Some("http://127.0.0.1:7000".to_string());
    removed_upstream.oauth = Some(UpstreamOauthConfig {
        mode: UpstreamOauthMode::AuthorizationCodePkce,
        registration: UpstreamOauthRegistration::Dynamic,
        scopes: None,
        prefer_client_metadata_document: None,
    });
    manager
        .seed_config(GatewayConfig {
            upstream: vec![removed_upstream],
            ..GatewayConfig::default()
        })
        .await;

    assert_eq!(cache.len(), 1);
    manager
        .reload_with_origin(None, None)
        .await
        .expect("reload");
    assert!(cache.is_empty());
}

#[tokio::test]
async fn reload_registers_new_upstream_oauth_manager() {
    let dir = tempfile::tempdir().expect("tempdir");
    let managers = Arc::new(dashmap::DashMap::new());
    let cache = OauthClientCache::new(Arc::clone(&managers));
    let (sqlite, key, redirect_uri) = fixture_oauth_resources(&dir).await;
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_upstream_oauth_managers(Arc::clone(&managers))
    .with_oauth_client_cache(cache)
    .with_oauth_resources(sqlite, key, redirect_uri);

    manager.reconcile_upstream_oauth_managers(&GatewayConfig {
        upstream: vec![fixture_oauth_upstream(
            "new-oauth",
            "https://127.0.0.1:9/mcp",
        )],
        ..GatewayConfig::default()
    });

    assert!(managers.contains_key("new-oauth"));
    assert_eq!(
        managers
            .get("new-oauth")
            .expect("oauth manager")
            .upstream_config()
            .url
            .as_deref(),
        Some("https://127.0.0.1:9/mcp")
    );
}

#[tokio::test]
async fn reload_replaces_changed_upstream_oauth_manager_and_evicts_cache() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (sqlite, key, redirect_uri) = fixture_oauth_resources(&dir).await;
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        "changed-oauth".to_string(),
        UpstreamOauthManager::new(
            sqlite.clone(),
            key.clone(),
            fixture_oauth_upstream("changed-oauth", "https://old.example.com/mcp"),
            redirect_uri.clone(),
        ),
    );
    let cache = OauthClientCache::new(Arc::clone(&managers));
    cache.insert_for_tests(
        "changed-oauth",
        "alice",
        "dynamic",
        dummy_auth_client().await,
    );
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_upstream_oauth_managers(Arc::clone(&managers))
    .with_oauth_client_cache(cache.clone())
    .with_oauth_resources(sqlite, key, redirect_uri);

    assert_eq!(cache.len(), 1);
    manager.reconcile_upstream_oauth_managers(&GatewayConfig {
        upstream: vec![fixture_oauth_upstream(
            "changed-oauth",
            "https://new.example.com/mcp",
        )],
        ..GatewayConfig::default()
    });

    assert!(cache.is_empty());
    assert_eq!(
        managers
            .get("changed-oauth")
            .expect("oauth manager")
            .upstream_config()
            .url
            .as_deref(),
        Some("https://new.example.com/mcp")
    );
}
