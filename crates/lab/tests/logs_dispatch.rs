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
use std::time::{SystemTime, UNIX_EPOCH};

mod support;

use support::log_system::{InstalledLogSystemGuard, SqlitePathCleanup, test_lock};

fn event_with(
    event_id: &str,
    ts: i64,
    subsystem: labby::dispatch::logs::types::Subsystem,
    level: labby::dispatch::logs::types::LogLevel,
    message: &str,
) -> labby::dispatch::logs::types::LogEvent {
    let mut event = labby::dispatch::logs::types::LogEvent::fixture();
    event.event_id = event_id.to_string();
    event.ts = ts;
    event.subsystem = subsystem;
    event.level = level;
    event.message = message.to_string();
    event
}

fn raw_gateway_event(message: &str) -> labby::dispatch::logs::types::RawLogEvent {
    labby::dispatch::logs::types::RawLogEvent {
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

fn raw_event_with_bearer_token() -> labby::dispatch::logs::types::RawLogEvent {
    let mut event = raw_gateway_event("Authorization: Bearer secret-value");
    event.fields_json = serde_json::json!({"authorization":"Bearer secret-value"});
    event
}

fn unique_store_path(prefix: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{unique}.db", std::process::id()))
}

#[test]
fn default_registry_includes_logs_service() {
    let registry = labby::registry::build_default_registry();
    let service = registry.service("logs").expect("logs service registered");
    assert_eq!(service.status, "available");
}

#[tokio::test]
async fn logs_dispatch_help_and_schema_exist() {
    let help = labby::dispatch::logs::dispatch("help", serde_json::json!({}))
        .await
        .unwrap();
    let schema =
        labby::dispatch::logs::dispatch("schema", serde_json::json!({"action":"logs.search"}))
            .await
            .unwrap();

    assert!(help.is_object());
    assert!(schema.is_object());
}

#[test]
fn log_system_bootstrap_installs_runtime() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let runtime = labby::dispatch::logs::client::bootstrap_log_system_for_test();
    assert!(runtime.is_ok());
}

#[tokio::test]
async fn local_live_commands_fail_cleanly_without_long_lived_runtime() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let error = labby::dispatch::logs::dispatch("logs.tail", serde_json::json!({"limit": 10}))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), "internal_error");
}

#[test]
fn log_event_serialization_preserves_future_ingest_fields() {
    let event = labby::dispatch::logs::types::LogEvent::fixture();
    let json = serde_json::to_value(&event).unwrap();
    assert!(json.get("source_kind").is_some());
    assert!(json.get("source_device_id").is_some());
    assert!(json.get("ingest_path").is_some());
}

#[test]
fn subsystem_enum_includes_local_master_taxonomy() {
    assert_eq!(
        labby::dispatch::logs::types::Subsystem::Gateway.as_str(),
        "gateway"
    );
    assert_eq!(
        labby::dispatch::logs::types::Subsystem::OauthRelay.as_str(),
        "oauth_relay"
    );
    assert_eq!(
        labby::dispatch::logs::types::Subsystem::AuthUpstream.as_str(),
        "auth_upstream"
    );
}

#[test]
fn local_logs_resolvers_honor_config_and_documented_defaults() {
    let mut config = labby::config::LabConfig::default();
    config.local_logs = Some(labby::config::LocalLogsPreferences {
        store_path: Some(std::path::PathBuf::from("/tmp/lab-local-logs-config.db")),
        retention_days: Some(11),
        max_bytes: Some(12_345),
        queue_capacity: Some(77),
        subscriber_capacity: Some(55),
    });

    assert_eq!(
        labby::dispatch::logs::client::resolve_store_path(Some(&config)),
        std::path::PathBuf::from("/tmp/lab-local-logs-config.db")
    );

    let retention = labby::dispatch::logs::client::resolve_retention(Some(&config));
    assert_eq!(retention.max_age_days, 11);
    assert_eq!(retention.max_bytes, 12_345);
    assert_eq!(
        labby::dispatch::logs::client::resolve_queue_capacity(Some(&config)),
        77
    );
    assert_eq!(
        labby::dispatch::logs::client::resolve_subscriber_capacity(Some(&config)),
        55
    );

    let empty = labby::config::LabConfig::default();
    assert_eq!(
        labby::dispatch::logs::client::resolve_queue_capacity(Some(&empty)),
        1024
    );
    assert_eq!(
        labby::dispatch::logs::client::resolve_subscriber_capacity(Some(&empty)),
        256
    );
}

#[tokio::test]
async fn store_search_filters_by_subsystem_and_level() {
    let store = labby::dispatch::logs::store::open_store_for_test(
        labby::dispatch::logs::types::LogRetention {
            max_age_days: 30,
            max_bytes: 1024 * 1024,
        },
    )
    .await
    .unwrap();
    store
        .insert(&event_with(
            "evt-gateway-warn",
            1_713_225_600_000,
            labby::dispatch::logs::types::Subsystem::Gateway,
            labby::dispatch::logs::types::LogLevel::Warn,
            "gateway warning",
        ))
        .await
        .unwrap();
    store
        .insert(&event_with(
            "evt-api-info",
            1_713_225_601_000,
            labby::dispatch::logs::types::Subsystem::Api,
            labby::dispatch::logs::types::LogLevel::Info,
            "api info",
        ))
        .await
        .unwrap();

    let result = store
        .search(labby::dispatch::logs::types::LogQuery {
            subsystems: vec![labby::dispatch::logs::types::Subsystem::Gateway],
            levels: vec![labby::dispatch::logs::types::LogLevel::Warn],
            ..labby::dispatch::logs::types::LogQuery::default()
        })
        .await
        .unwrap();

    assert_eq!(result.events.len(), 1);
}

#[tokio::test]
async fn retention_enforces_age_and_size_limits() {
    let store = labby::dispatch::logs::store::open_store_for_test(
        labby::dispatch::logs::types::LogRetention {
            max_age_days: 7,
            max_bytes: 1024,
        },
    )
    .await
    .unwrap();
    store
        .insert(&event_with(
            "evt-old",
            1,
            labby::dispatch::logs::types::Subsystem::Gateway,
            labby::dispatch::logs::types::LogLevel::Warn,
            &"x".repeat(2048),
        ))
        .await
        .unwrap();
    store
        .insert(&event_with(
            "evt-new",
            4_102_444_800_000,
            labby::dispatch::logs::types::Subsystem::Gateway,
            labby::dispatch::logs::types::LogLevel::Info,
            "recent event",
        ))
        .await
        .unwrap();

    store.run_maintenance().await.unwrap();
    let stats = store.stats().await.unwrap();

    assert!(stats.on_disk_bytes <= 1024);
    assert!(stats.oldest_retained_ts.unwrap_or_default() >= 4_102_444_800_000);
}

#[tokio::test]
async fn ingest_redacts_sensitive_fields_before_store_and_stream() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let system = labby::dispatch::logs::client::bootstrap_running_log_system_for_test(16)
        .await
        .unwrap();
    system.ingest(raw_event_with_bearer_token()).await.unwrap();

    let stored = system
        .search(labby::dispatch::logs::types::LogQuery::default())
        .await
        .unwrap();
    assert!(!stored.events[0].message.contains("Bearer "));
    assert!(
        !stored.events[0]
            .fields_json
            .to_string()
            .contains("secret-value")
    );
}

#[tokio::test]
async fn stream_subscribers_receive_new_events_without_querying_store() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let system = labby::dispatch::logs::client::bootstrap_running_log_system_for_test(16)
        .await
        .unwrap();
    let mut sub = system
        .subscribe(labby::dispatch::logs::types::StreamSubscription::default())
        .await
        .unwrap();
    system.ingest(raw_gateway_event("stream me")).await.unwrap();

    let next = sub.recv().await.unwrap();
    assert_eq!(
        next.subsystem,
        labby::dispatch::logs::types::Subsystem::Gateway
    );
}

#[tokio::test]
async fn full_ingest_queue_records_overflow_without_blocking_caller() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let system = labby::dispatch::logs::client::bootstrap_running_log_system_for_test(1)
        .await
        .unwrap();
    for _ in 0..100 {
        drop(system.try_ingest(raw_gateway_event("queue pressure")));
    }

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let stats = loop {
        let stats = system.stats().await.unwrap();
        if stats.dropped_event_count > 0 {
            break stats;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never observed dropped events: {stats:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    assert!(stats.dropped_event_count > 0);
}

#[tokio::test]
async fn logs_search_returns_filtered_results() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let system = labby::dispatch::logs::client::bootstrap_running_log_system_for_test(16)
        .await
        .unwrap();
    system.ingest(raw_gateway_event("search me")).await.unwrap();

    let value = labby::dispatch::logs::dispatch(
        "logs.search",
        serde_json::json!({
            "query": { "subsystems": ["gateway"], "levels": ["warn"] }
        }),
    )
    .await
    .unwrap();

    let events = value["events"].as_array().expect("events array");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["message"], "search me");
}

#[tokio::test]
async fn logs_stats_returns_retention_metadata() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let _system = labby::dispatch::logs::client::bootstrap_running_log_system_for_test(16)
        .await
        .unwrap();

    let value = labby::dispatch::logs::dispatch("logs.stats", serde_json::json!({}))
        .await
        .unwrap();

    assert!(value.get("on_disk_bytes").is_some());
}

#[tokio::test]
async fn logs_tail_returns_bounded_follow_up_window() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let system = labby::dispatch::logs::client::bootstrap_running_log_system_for_test(16)
        .await
        .unwrap();
    system.ingest(raw_gateway_event("tail me")).await.unwrap();

    let value = labby::dispatch::logs::dispatch(
        "logs.tail",
        serde_json::json!({
            "after_ts": 0,
            "limit": 50
        }),
    )
    .await
    .unwrap();

    assert!(value.get("events").is_some());
    assert!(value.get("next_cursor").is_some());
}

#[tokio::test]
async fn logs_tail_honors_since_event_id_cursor() {
    let store = labby::dispatch::logs::store::open_store_for_test(
        labby::dispatch::logs::types::LogRetention {
            max_age_days: 30,
            max_bytes: 1024 * 1024,
        },
    )
    .await
    .unwrap();
    store
        .insert(&event_with(
            "evt-1",
            10,
            labby::dispatch::logs::types::Subsystem::Gateway,
            labby::dispatch::logs::types::LogLevel::Info,
            "before cursor",
        ))
        .await
        .unwrap();
    store
        .insert(&event_with(
            "evt-2",
            20,
            labby::dispatch::logs::types::Subsystem::Gateway,
            labby::dispatch::logs::types::LogLevel::Info,
            "cursor",
        ))
        .await
        .unwrap();
    store
        .insert(&event_with(
            "evt-3",
            30,
            labby::dispatch::logs::types::Subsystem::Gateway,
            labby::dispatch::logs::types::LogLevel::Info,
            "after cursor",
        ))
        .await
        .unwrap();

    let result = store
        .tail(labby::dispatch::logs::types::LogTailRequest {
            since_event_id: Some("evt-2".to_string()),
            limit: Some(10),
            ..labby::dispatch::logs::types::LogTailRequest::default()
        })
        .await
        .unwrap();

    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].event_id, "evt-3");
}

#[tokio::test]
async fn store_search_text_matches_request_identifiers() {
    let store = labby::dispatch::logs::store::open_store_for_test(
        labby::dispatch::logs::types::LogRetention {
            max_age_days: 30,
            max_bytes: 1024 * 1024,
        },
    )
    .await
    .unwrap();
    store
        .insert(&event_with(
            "evt-request-id",
            1_713_225_600_000,
            labby::dispatch::logs::types::Subsystem::Gateway,
            labby::dispatch::logs::types::LogLevel::Warn,
            "gateway warning",
        ))
        .await
        .unwrap();

    let result = store
        .search(labby::dispatch::logs::types::LogQuery {
            text: Some("req-fixture".to_string()),
            ..labby::dispatch::logs::types::LogQuery::default()
        })
        .await
        .unwrap();

    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].event_id, "evt-request-id");
}

#[tokio::test]
async fn local_logs_persist_across_restart() {
    let mut lock = test_lock();
    let _lock = lock.write().expect("log system test lock");
    let _installed = InstalledLogSystemGuard::new();
    let path = unique_store_path("lab-local-logs-persist");
    let _cleanup = SqlitePathCleanup::new(path.clone());
    let writer = labby::dispatch::logs::client::bootstrap_running_log_system(
        path.clone(),
        labby::dispatch::logs::types::LogRetention::default(),
        16,
        16,
    )
    .await
    .expect("writer runtime");
    writer
        .ingest(raw_gateway_event("persisted across restart"))
        .await
        .expect("seed event");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let reader = labby::dispatch::logs::client::bootstrap_store_backed_log_system(
        path,
        labby::dispatch::logs::types::LogRetention::default(),
    )
    .await
    .expect("reader runtime");

    let result = reader
        .search(labby::dispatch::logs::types::LogQuery {
            text: Some("persisted".to_string()),
            ..labby::dispatch::logs::types::LogQuery::default()
        })
        .await
        .expect("search after restart");

    assert_eq!(result.events.len(), 1);
    assert!(
        result.events[0]
            .message
            .contains("persisted across restart")
    );
}
