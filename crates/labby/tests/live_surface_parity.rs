#![allow(clippy::panic, dead_code)]

#[path = "support/action_matrix.rs"]
mod action_matrix;
#[path = "support/action_scenarios.rs"]
mod action_scenarios;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_identity.rs"]
mod live_identity;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/parity_normalize.rs"]
mod parity_normalize;
#[path = "support/state_snapshot.rs"]
mod state_snapshot;

mod support {
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use std::collections::{BTreeMap, BTreeSet};

use action_matrix::Surface;
use reqwest::StatusCode;
use state_snapshot::PublicCatalogObservation;

#[test]
fn every_current_multi_surface_service_has_a_parity_partition() {
    let mut surfaces = BTreeMap::<String, BTreeSet<Surface>>::new();
    for intent in action_matrix::compiled_intents() {
        surfaces
            .entry(intent.service.clone())
            .or_default()
            .extend(intent.applicable_surfaces.iter().copied());
    }
    let multi = surfaces
        .iter()
        .filter(|(_, surfaces)| surfaces.len() > 1)
        .map(|(service, _)| service.as_str())
        .collect::<BTreeSet<_>>();
    if cfg!(feature = "all") {
        assert_eq!(
            multi,
            BTreeSet::from([
                "artifacts",
                "browser",
                "bundles",
                "doctor",
                "fs",
                "gateway",
                "jobs",
                "server_logs",
                "setup",
                "snippets",
                "sources",
                "stash",
                "uploads"
            ])
        );
    }
    for service in multi {
        assert!(
            action_scenarios::fixtures().contains_key(service),
            "{service} lacks a deterministic parity namespace"
        );
    }
}

#[tokio::test]
async fn one_public_identity_has_equivalent_api_browser_and_mcp_discovery() {
    let mut identity = live_identity::LiveIdentity::bootstrap("parity-shared-subject")
        .await
        .expect("public identity");
    identity.create_session().await.expect("browser session");
    let (bearer_status, bearer_body) = identity.bearer_catalog_response().await.unwrap();
    let csrf = identity.session.as_ref().unwrap().csrf.clone();
    let (browser_status, browser_body) = identity
        .browser_catalog_response(Some(&csrf))
        .await
        .unwrap();
    assert_eq!(bearer_status, StatusCode::OK);
    assert_eq!(browser_status, StatusCode::OK);
    let bearer = PublicCatalogObservation::from_json(bearer_status.as_u16(), &bearer_body);
    let browser = PublicCatalogObservation::from_json(browser_status.as_u16(), &browser_body);
    assert_eq!(
        bearer, browser,
        "browser and bearer capability views drifted"
    );

    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK,
        "the same identity was not accepted by protected MCP discovery"
    );

    let cli = action_scenarios::run_cli_in_install(
        &identity.root().join("home"),
        &identity.root().join("labby-home"),
        &["gateway", "list", "--json"],
    )
    .await
    .expect("CLI shared-state observation");
    action_scenarios::assert_success_json(&cli, "parity gateway.list");
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK
    );

    let cleanup = identity.cleanup().await.expect("cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[test]
fn stable_error_normalization_retains_recovery_and_side_effect_contracts() {
    let cli = serde_json::json!({"error":{"kind":"invalid_params","recovery":"fix request","side_effects":"none","requires_admin":false,"destructive":false},"request_id":"cli"});
    let api = serde_json::json!({"error":{"kind":"invalid_params","recovery":"fix request","side_effects":"none","requires_admin":false,"destructive":false},"correlation_id":"api"});
    parity_normalize::assert_equivalent(&cli, &api);
}
