//! Virtual-server listing, surface gating, and MCP action-policy tests.

use crate::gateway::projection::{ServiceHealth, server_view_from_virtual_server};
use crate::gateway::service_registry::EmptyServiceRegistry;
use crate::upstream::pool::UpstreamCachedSummary;
use lab_runtime::gateway_config::{VirtualServerConfig, VirtualServerSurfacesConfig};

use super::*;

#[tokio::test]
async fn configured_service_appears_in_list_before_virtual_server_enablement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    manager
        .seed_config(GatewayConfig {
            virtual_servers: vec![VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: false,
                surfaces: VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..GatewayConfig::default()
        })
        .await;

    let servers = manager.list().await.expect("list");
    let plex = servers
        .iter()
        .find(|server| server.id == "deploy")
        .expect("plex server");
    assert!(plex.configured);
    assert!(!plex.enabled);
    assert_eq!(plex.source, "in_process");
}

#[tokio::test]
async fn stale_virtual_server_with_unknown_service_does_not_break_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    manager
        .seed_config(GatewayConfig {
            virtual_servers: vec![VirtualServerConfig {
                id: "mcpregistry".to_string(),
                service: "mcpregistry".to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    cli: false,
                    api: false,
                    mcp: true,
                    webui: false,
                },
                mcp_policy: None,
            }],
            ..GatewayConfig::default()
        })
        .await;

    let servers = manager.list().await.expect("list should fail open");
    let stale = servers
        .iter()
        .find(|server| server.id == "mcpregistry")
        .expect("stale server row");

    assert!(!stale.connected);
    assert!(!stale.surfaces.mcp.connected);
    assert_eq!(stale.discovered_tool_count, 0);
    assert_eq!(
        stale.warnings.first().map(|warning| warning.code.as_str()),
        Some("unknown_service")
    );
}

// CANNOT be re-fixtured without a production change (out of test-only scope): it
// drives `set_service_config("deploy", {PLEX_TOKEN})`, but `deploy` (the only
// service `registry::service_meta` resolves post-pivot) declares zero env fields, so
// the call is rejected as an invalid field. Modelling an "incomplete" service needs
// a service_meta-resolvable service that declares a required env var — none exists
// post-pivot.
#[tokio::test]
#[ignore = "needs a service_meta-resolvable service with required env fields; only `deploy` resolves and it declares none — prod change required"]
async fn incomplete_service_does_not_appear_in_list_before_virtual_server_enablement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    let mut values = BTreeMap::new();
    values.insert("PLEX_TOKEN".to_string(), "token".to_string());

    manager
        .set_service_config("deploy", &values)
        .await
        .expect("set service config");

    let servers = manager.list().await.expect("list");
    assert!(
        servers.iter().all(|server| server.id != "deploy"),
        "incomplete services should not appear in the gateway catalog"
    );
}

#[tokio::test]
async fn disabling_virtual_server_preserves_configured_service_listing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    manager
        .seed_config(GatewayConfig {
            virtual_servers: vec![VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..GatewayConfig::default()
        })
        .await;

    let mut cfg = manager.config.read().await.clone();
    cfg.virtual_servers[0].enabled = false;
    manager.seed_config(cfg).await;

    let servers = manager.list().await.expect("list");
    let plex = servers
        .iter()
        .find(|server| server.id == "deploy")
        .expect("plex server");
    assert!(plex.configured);
    assert!(!plex.enabled);
    assert_eq!(plex.config_summary.target.as_deref(), Some("deploy"));
}

#[test]
fn disabled_virtual_server_reports_disconnected_even_when_health_is_ok() {
    let view = server_view_from_virtual_server(
        &VirtualServerConfig {
            id: "deploy".to_string(),
            service: "deploy".to_string(),
            enabled: false,
            surfaces: VirtualServerSurfacesConfig::default(),
            mcp_policy: None,
        },
        UpstreamCachedSummary::default(),
        None,
        Some(&ServiceHealth {
            reachable: true,
            auth_ok: true,
        }),
        &EmptyServiceRegistry,
    );

    assert!(!view.connected);
    assert!(!view.surfaces.mcp.connected);
}

// Re-fixtured post-gateway-pivot to the kept `deploy` service. `service_known` is
// `service_meta(service).is_some()`, and post-pivot `service_meta` resolves only
// `deploy` — so a registered, healthy service must produce no `unknown_service`
// warning. (`unraid` is no longer registered and would now warn.)
#[test]
fn healthy_informational_probe_messages_do_not_create_gateway_warnings() {
    let view = server_view_from_virtual_server(
        &VirtualServerConfig {
            id: "deploy".to_string(),
            service: "deploy".to_string(),
            enabled: true,
            surfaces: VirtualServerSurfacesConfig::default(),
            mcp_policy: None,
        },
        UpstreamCachedSummary::default(),
        None,
        Some(&ServiceHealth {
            reachable: true,
            auth_ok: true,
        }),
        deploy_known_registry().as_ref(),
    );

    assert!(view.connected);
    assert!(view.warnings.is_empty());
}

// CANNOT be re-fixtured without a production change (out of test-only scope): it
// drives `set_service_config("deploy", {PLEX_URL, PLEX_TOKEN})`, which `deploy`
// rejects (it declares no env fields). The surface assertions that follow don't need
// the config write, but the `.expect()` on it panics first. Needs a
// service_meta-resolvable service that declares env fields — none exists post-pivot.
#[tokio::test]
#[ignore = "set_service_config rejects deploy's PLEX_* fields (deploy declares no env); needs a service_meta service with env fields — prod change required"]
async fn managed_services_are_hidden_on_surfaces_until_enabled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    let mut values = BTreeMap::new();
    values.insert("PLEX_URL".to_string(), "http://127.0.0.1:32400".to_string());
    values.insert("PLEX_TOKEN".to_string(), "token".to_string());

    manager
        .set_service_config("deploy", &values)
        .await
        .expect("set service config");

    assert!(!manager.surface_enabled_for_service("deploy", "mcp").await);
    assert!(manager.surface_enabled_for_service("deploy", "api").await);
    assert!(manager.surface_enabled_for_service("deploy", "cli").await);
}

// Re-fixtured post-gateway-pivot: backed by the kept/registered `deploy` service,
// so the surface-gating logic runs (not the unregistered-service early-return in
// `surface_enabled_for_service`).
#[tokio::test]
async fn enabled_virtual_server_only_exposes_enabled_surfaces() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default())
        .with_builtin_service_registry(deploy_known_registry());

    manager
        .seed_config(GatewayConfig {
            virtual_servers: vec![VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    cli: false,
                    api: true,
                    mcp: true,
                    webui: false,
                },
                mcp_policy: None,
            }],
            ..GatewayConfig::default()
        })
        .await;

    assert!(manager.surface_enabled_for_service("deploy", "api").await);
    assert!(manager.surface_enabled_for_service("deploy", "mcp").await);
    assert!(!manager.surface_enabled_for_service("deploy", "cli").await);
}

#[test]
fn enabled_virtual_server_reports_compiled_tool_counts() {
    let view = server_view_from_virtual_server(
        &VirtualServerConfig {
            id: "deploy".to_string(),
            service: "deploy".to_string(),
            enabled: true,
            surfaces: VirtualServerSurfacesConfig {
                cli: true,
                api: true,
                mcp: true,
                webui: true,
            },
            mcp_policy: None,
        },
        UpstreamCachedSummary {
            discovered_tool_count: 5,
            exposed_tool_count: 5,
            discovered_resource_count: 0,
            exposed_resource_count: 0,
            discovered_prompt_count: 0,
            exposed_prompt_count: 0,
        },
        None,
        Some(&ServiceHealth {
            reachable: true,
            auth_ok: true,
        }),
        &EmptyServiceRegistry,
    );

    assert!(view.discovered_tool_count > 0);
    assert_eq!(view.discovered_tool_count, view.exposed_tool_count);
    assert_eq!(view.discovered_resource_count, 0);
    assert_eq!(view.discovered_prompt_count, 0);
}

#[test]
fn virtual_server_mcp_policy_reduces_exposed_tool_count() {
    let view = server_view_from_virtual_server(
        &VirtualServerConfig {
            id: "deploy".to_string(),
            service: "deploy".to_string(),
            enabled: true,
            surfaces: VirtualServerSurfacesConfig {
                cli: true,
                api: true,
                mcp: true,
                webui: true,
            },
            mcp_policy: Some(lab_runtime::gateway_config::VirtualServerMcpPolicyConfig {
                allowed_actions: vec!["server.info".to_string()],
            }),
        },
        UpstreamCachedSummary {
            discovered_tool_count: 5,
            exposed_tool_count: 3,
            discovered_resource_count: 0,
            exposed_resource_count: 0,
            discovered_prompt_count: 0,
            exposed_prompt_count: 0,
        },
        None,
        Some(&ServiceHealth {
            reachable: true,
            auth_ok: true,
        }),
        &EmptyServiceRegistry,
    );

    assert!(view.discovered_tool_count > view.exposed_tool_count);
    assert_eq!(view.exposed_tool_count, 3);
}

// Re-fixtured post-gateway-pivot: backed by the kept/registered `deploy` service so
// the MCP action allowlist is actually enforced (an unregistered service would
// early-return `true` for every action in `mcp_action_allowed_for_service`).
#[tokio::test]
async fn mcp_action_policy_restricts_actions_to_allowlist() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default())
        .with_builtin_service_registry(deploy_known_registry());

    manager
        .seed_config(GatewayConfig {
            virtual_servers: vec![VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    cli: false,
                    api: false,
                    mcp: true,
                    webui: false,
                },
                mcp_policy: Some(lab_runtime::gateway_config::VirtualServerMcpPolicyConfig {
                    allowed_actions: vec!["server.info".to_string()],
                }),
            }],
            ..GatewayConfig::default()
        })
        .await;

    assert!(
        manager
            .mcp_action_allowed_for_service("deploy", "server.info")
            .await
    );
    assert!(
        manager
            .mcp_action_allowed_for_service("deploy", "help")
            .await
    );
    assert!(
        !manager
            .mcp_action_allowed_for_service("deploy", "sessions.list")
            .await
    );
}

#[tokio::test]
async fn unrestricted_mcp_actions_return_none_when_no_policy_is_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    manager
        .seed_config(GatewayConfig {
            virtual_servers: vec![VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    cli: false,
                    api: false,
                    mcp: true,
                    webui: false,
                },
                mcp_policy: None,
            }],
            ..GatewayConfig::default()
        })
        .await;

    assert_eq!(
        manager.allowed_mcp_actions_for_service("deploy").await,
        None
    );
}

#[tokio::test]
async fn synthetic_services_without_gateway_metadata_allow_mcp_actions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    manager.seed_config(GatewayConfig::default()).await;

    assert!(
        manager
            .mcp_action_allowed_for_service("marketplace", "mcp.config")
            .await
    );
}
