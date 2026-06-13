#![allow(clippy::panic)]

use clap::Parser;
use labby::cli::nodes::NodesCommand;
use labby::cli::{Cli, Command};
use labby::config::{LabConfig, NodePreferences};
use labby::node::master_client::MasterClient;
use url::Url;

#[tokio::test]
async fn device_list_command_reads_from_master_api() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/v1/nodes"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"node_id":"dookie","connected":true}
            ])),
        )
        .mount(&server)
        .await;

    let config = config_for_master(&server.uri());
    let value = labby::cli::nodes::fetch_nodes(&config).await.unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn device_enrollments_list_command_reads_from_master_api() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/v1/nodes/enrollments"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "pending": {"device-1": {"node_id":"device-1"}},
                "approved": {},
                "denied": {}
            })),
        )
        .mount(&server)
        .await;

    let config = config_for_master(&server.uri());
    let value = labby::cli::nodes::fetch_enrollments(&config).await.unwrap();
    assert!(value["pending"]["device-1"].is_object());
}

#[tokio::test]
async fn device_enrollments_approve_command_calls_master_api() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(
            "/v1/nodes/enrollments/device%2D1/approve",
        ))
        .and(wiremock::matchers::body_string_contains("\"note\":\"ok\""))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"node_id":"device-1"})),
        )
        .mount(&server)
        .await;

    let config = config_for_master(&server.uri());
    let value = labby::cli::nodes::approve_enrollment(&config, "device-1", Some("ok"))
        .await
        .unwrap();
    assert_eq!(value["node_id"], "device-1");
}

#[tokio::test]
async fn device_enrollments_deny_command_calls_master_api() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(
            "/v1/nodes/enrollments/device%2D1/deny",
        ))
        .and(wiremock::matchers::body_string_contains(
            "\"reason\":\"no\"",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"node_id":"device-1"})),
        )
        .mount(&server)
        .await;

    let config = config_for_master(&server.uri());
    let value = labby::cli::nodes::deny_enrollment(&config, "device-1", Some("no"))
        .await
        .unwrap();
    assert_eq!(value["node_id"], "device-1");
}

#[tokio::test]
async fn logs_search_command_reads_from_master_api() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/nodes/logs/search"))
        .and(wiremock::matchers::body_string_contains(
            "\"node_id\":\"dookie\"",
        ))
        .and(wiremock::matchers::body_string_contains(
            "\"query\":\"hello\"",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"node_id":"dookie","message":"hello"}
            ])),
        )
        .mount(&server)
        .await;

    let config = config_for_master(&server.uri());
    let value = labby::cli::logs::search_logs(&config, "dookie", "hello")
        .await
        .unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn master_client_applies_bearer_token_to_master_requests() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/v1/nodes"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer shared-secret",
        ))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;

    let value = MasterClient::with_bearer_token(server.uri(), Some("shared-secret".into()))
        .unwrap()
        .fetch_devices()
        .await
        .unwrap();
    assert!(value.as_array().unwrap().is_empty());
}

#[test]
fn nodes_update_parses_all_flag() {
    let cli = Cli::try_parse_from(["lab", "nodes", "update", "--all"]).expect("parse nodes update");
    match cli.command {
        Command::Nodes(args) => match args.command {
            NodesCommand::Update(update) => {
                assert!(update.all);
                assert!(update.targets.is_empty());
            }
            other => panic!("unexpected nodes command: {other:?}"),
        },
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn nodes_update_parses_explicit_targets() {
    let cli =
        Cli::try_parse_from(["lab", "nodes", "update", "mini1", "mini2"]).expect("parse targets");
    match cli.command {
        Command::Nodes(args) => match args.command {
            NodesCommand::Update(update) => {
                assert!(!update.all);
                assert_eq!(update.targets, vec!["mini1", "mini2"]);
            }
            other => panic!("unexpected nodes command: {other:?}"),
        },
        other => panic!("unexpected command: {other:?}"),
    }
}

/// These tests verify the `--role` flag parses for both values.
#[test]
fn serve_role_node_parses() {
    Cli::try_parse_from(["lab", "serve", "--role", "node"]).unwrap();
}

#[test]
fn serve_role_controller_parses() {
    Cli::try_parse_from(["lab", "serve", "--role", "controller"]).unwrap();
}

fn config_for_master(uri: &str) -> LabConfig {
    let parsed = Url::parse(uri).unwrap();
    let mut config = LabConfig {
        node: Some(NodePreferences {
            controller: parsed.host_str().map(str::to_string),
            ..Default::default()
        }),
        ..Default::default()
    };
    config.mcp.port = parsed.port();
    config
}
