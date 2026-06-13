#![allow(
    clippy::await_holding_lock,
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::panic,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
use clap::Parser;

use labby::cli::Cli;
use labby::dispatch::logs::types::{LogLevel, LogQuery, RawLogEvent, Subsystem};

mod support;

use support::log_system::{InstalledLogSystemGuard, test_lock};

fn raw_gateway_event(message: &str) -> RawLogEvent {
    RawLogEvent {
        ts: Some(1_713_225_600_000),
        level: Some("warn".to_string()),
        subsystem: Some("gateway".to_string()),
        surface: Some("api".to_string()),
        action: Some("gateway.list".to_string()),
        message: message.to_string(),
        request_id: Some("req-gateway".to_string()),
        session_id: None,
        correlation_id: None,
        trace_id: None,
        span_id: None,
        instance: Some("default".to_string()),
        auth_flow: None,
        outcome_kind: Some("ok".to_string()),
        fields_json: serde_json::json!({"route":"gateway.list"}),
        source_kind: None,
        source_node_id: None,
        source_device_id: None,
        actor_key: None,
        ingest_path: None,
        upstream_event_id: None,
    }
}

#[test]
fn logs_cli_parses_existing_fleet_search() {
    let cli = Cli::try_parse_from(["lab", "logs", "search", "node-a", "timeout"])
        .expect("fleet search parses");
    assert!(matches!(cli.command, labby::cli::Command::Logs(_)));
}

#[test]
fn logs_cli_parses_local_search() {
    let cli = Cli::try_parse_from([
        "lab",
        "logs",
        "local",
        "search",
        "--subsystem",
        "gateway",
        "--level",
        "warn",
        "--limit",
        "10",
    ])
    .expect("local search parses");
    assert!(matches!(cli.command, labby::cli::Command::Logs(_)));
}

#[tokio::test]
async fn logs_local_search_uses_shared_dispatch_contract() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let system = labby::dispatch::logs::client::bootstrap_running_log_system_for_test(16)
        .await
        .expect("runtime");
    system
        .ingest(raw_gateway_event("local cli query"))
        .await
        .expect("seed event");

    let value = labby::dispatch::logs::dispatch::dispatch_with_system(
        &system,
        "logs.search",
        serde_json::json!({
            "query": LogQuery {
                subsystems: vec![Subsystem::Gateway],
                levels: vec![LogLevel::Warn],
                ..LogQuery::default()
            }
        }),
    )
    .await
    .expect("search response");

    let events = value["events"].as_array().expect("events array");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["message"], "local cli query");
}
