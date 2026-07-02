#![cfg(feature = "nodes")]
#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
use labby::node::checkin::{NodeHello, NodeStatus};
use labby::node::log_event::NodeLogEvent;
use labby::node::queue::NodeOutboundQueue;
use labby::node::runtime::NodeRuntime;
use labby::node::store::NodeStore;

fn test_node_status(node_id: &str) -> NodeStatus {
    NodeStatus {
        node_id: node_id.into(),
        connected: true,
        cpu_percent: Some(3.5),
        memory_used_bytes: Some(1024),
        storage_used_bytes: Some(2048),
        os: Some("linux".into()),
        ips: vec!["100.64.0.1".into()],
        health: None,
        version: None,
        uptime_seconds: None,
        cores: None,
        cpu_clock_mhz: None,
        cpu_temp_c: None,
        total_memory_bytes: None,
        total_storage_bytes: None,
        doctor_issues: vec![],
        active_claude_sessions: None,
        active_codex_sessions: None,
    }
}

#[tokio::test]
async fn device_store_marks_hello_devices_connected_and_tracks_status() {
    let store = NodeStore::default();
    store
        .record_hello(NodeHello {
            node_id: "controller".into(),
            role: "master".into(),
            version: "1.0.0".into(),
        })
        .await;

    let snapshot = store.node("controller").await.unwrap();
    assert!(snapshot.connected);

    store.record_status(test_node_status("controller")).await;

    let snapshot = store.node("controller").await.unwrap();
    assert!(snapshot.connected);
    assert_eq!(snapshot.node_id, "controller");
}

#[tokio::test]
async fn non_master_runtime_uploads_discovered_ai_cli_inventory() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join(".claude.json"),
        r#"{"mcpServers":{"labby":{"command": "labby","args":["serve"]}}}"#,
    )
    .unwrap();

    let runtime = NodeRuntime::non_master_for_test_with_home(
        "node-a",
        "http://master:8765".to_string(),
        temp.path(),
    )
    .unwrap();
    runtime.upload_initial_metadata().await.unwrap();

    let queue = NodeOutboundQueue::open(temp.path().join(".labby/node-runtime-queue.jsonl"))
        .await
        .unwrap();
    let drained = queue.drain_batch(10).await.unwrap();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].kind, "metadata");
    assert_eq!(drained[0].payload["node_id"], "node-a");
    assert_eq!(
        drained[0].payload["discovered_configs"][0]["path"],
        ".claude.json"
    );
    assert!(
        !drained[0].payload["discovered_configs"][0]["content_hash"]
            .as_str()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn master_store_keeps_uploaded_logs_by_device() {
    let store = NodeStore::default();
    store
        .record_logs(
            "node-a",
            vec![NodeLogEvent {
                node_id: "node-a".into(),
                source: "journald".into(),
                timestamp_unix_ms: 1,
                level: Some("info".into()),
                message: "hello".into(),
                fields: Default::default(),
            }],
        )
        .await;

    let snapshot = store.node("node-a").await.unwrap();
    assert_eq!(snapshot.logs.len(), 1);
}

#[tokio::test]
async fn queue_syslog_batch_appends_entries_for_websocket_delivery() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = NodeRuntime::non_master_for_test_with_home(
        "node-a",
        "http://master:8765".to_string(),
        temp.path(),
    )
    .unwrap();
    runtime
        .queue_syslog_batch(vec![NodeLogEvent {
            node_id: "node-a".into(),
            source: "journald".into(),
            timestamp_unix_ms: 1,
            level: Some("info".into()),
            message: "first".into(),
            fields: Default::default(),
        }])
        .await
        .unwrap();
    runtime
        .queue_syslog_batch(vec![NodeLogEvent {
            node_id: "node-a".into(),
            source: "journald".into(),
            timestamp_unix_ms: 2,
            level: Some("warn".into()),
            message: "second".into(),
            fields: Default::default(),
        }])
        .await
        .unwrap();

    let queue = NodeOutboundQueue::open(temp.path().join(".labby/node-runtime-queue.jsonl"))
        .await
        .unwrap();
    let drained = queue.drain_batch(10).await.unwrap();
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].payload["events"][0]["message"], "first");
    assert_eq!(drained[1].payload["events"][0]["message"], "second");
}

#[tokio::test]
async fn device_store_search_logs_applies_offset_limit_and_retention() {
    let store = NodeStore::default();
    for index in 0..10_100 {
        store
            .record_logs(
                "node-a",
                vec![NodeLogEvent {
                    node_id: "node-a".into(),
                    source: "journald".into(),
                    timestamp_unix_ms: index,
                    level: Some("info".into()),
                    message: format!("hello-{index}"),
                    fields: Default::default(),
                }],
            )
            .await;
    }

    let retained = store.node("node-a").await.unwrap().logs;
    assert_eq!(retained.len(), 10_000);
    assert_eq!(retained.front().unwrap().message, "hello-100");

    let searched = store.search_logs_for_node("node-a", "hello", 5, 3).await;
    assert_eq!(searched.len(), 3);
    assert_eq!(searched.first().unwrap().message, "hello-105");
}
