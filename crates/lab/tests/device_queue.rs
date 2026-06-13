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
use labby::node::queue::{NodeOutboundQueue, QueuedEnvelope};

#[tokio::test]
async fn queue_persists_and_reloads_entries() {
    let temp = tempfile::tempdir().unwrap();
    let queue = NodeOutboundQueue::open(temp.path().join("queue.jsonl"))
        .await
        .unwrap();

    queue
        .push(QueuedEnvelope::status(
            serde_json::json!({"device_id":"tootie"}),
        ))
        .await
        .unwrap();
    drop(queue);

    let reopened = NodeOutboundQueue::open(temp.path().join("queue.jsonl"))
        .await
        .unwrap();
    let drained = reopened.drain_batch(10).await.unwrap();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].payload["device_id"], "tootie");
}

#[tokio::test]
async fn queue_ack_uses_latest_on_disk_entries_when_multiple_handles_share_a_path() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("queue.jsonl");

    let first = NodeOutboundQueue::open(path.clone()).await.unwrap();
    first
        .push(QueuedEnvelope::status(
            serde_json::json!({"device_id":"first"}),
        ))
        .await
        .unwrap();

    let second = NodeOutboundQueue::open(path.clone()).await.unwrap();
    second
        .push(QueuedEnvelope::status(
            serde_json::json!({"device_id":"second"}),
        ))
        .await
        .unwrap();

    first.ack_drained(1).await.unwrap();

    let reopened = NodeOutboundQueue::open(path).await.unwrap();
    let drained = reopened.drain_batch(10).await.unwrap();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].payload["device_id"], "second");
}
