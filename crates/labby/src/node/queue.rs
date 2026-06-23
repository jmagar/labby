use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::Mutex;

const DEFAULT_SEGMENT_BYTES: u64 = 1024 * 1024;
const STATE_FILE_NAME: &str = "state.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedEnvelope {
    pub kind: String,
    pub payload: serde_json::Value,
}

impl QueuedEnvelope {
    #[must_use]
    #[allow(dead_code)]
    pub fn status(payload: serde_json::Value) -> Self {
        Self {
            kind: "status".to_string(),
            payload,
        }
    }

    #[must_use]
    pub fn metadata(payload: serde_json::Value) -> Self {
        Self {
            kind: "metadata".to_string(),
            payload,
        }
    }

    #[must_use]
    pub fn syslog_batch(payload: serde_json::Value) -> Self {
        Self {
            kind: "syslog_batch".to_string(),
            payload,
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn application_log_batch(payload: serde_json::Value) -> Self {
        Self {
            kind: "application_log_batch".to_string(),
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueState {
    head_segment: u64,
    head_offset: usize,
    active_segment: u64,
}

impl Default for QueueState {
    fn default() -> Self {
        Self {
            head_segment: 1,
            head_offset: 0,
            active_segment: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeOutboundQueue {
    path: PathBuf,
    storage_dir: PathBuf,
    io_lock: Arc<Mutex<()>>,
    segment_bytes: u64,
}

impl NodeOutboundQueue {
    pub async fn open(path: PathBuf) -> Result<Self> {
        Self::open_with_segment_bytes(path, DEFAULT_SEGMENT_BYTES).await
    }

    async fn open_with_segment_bytes(path: PathBuf, segment_bytes: u64) -> Result<Self> {
        let storage_dir = queue_storage_dir(&path);
        let io_lock = shared_queue_lock(&path);
        let queue = Self {
            path,
            storage_dir,
            io_lock,
            segment_bytes,
        };
        {
            let _guard = queue.io_lock.lock().await;
            queue.initialize_storage().await?;
        }
        Ok(queue)
    }

    pub async fn push(&self, envelope: QueuedEnvelope) -> Result<()> {
        let _guard = self.io_lock.lock().await;
        self.ensure_storage_exists().await?;
        let mut state = self.read_state().await?;
        let mut line =
            serde_json::to_string(&envelope).context("serialize queue entry for append")?;
        line.push('\n');
        let line_bytes =
            u64::try_from(line.len()).context("queue entry length does not fit in u64")?;

        let active_path = self.segment_path(state.active_segment);
        let current_len = file_len(&active_path).await?;
        if current_len > 0 && current_len.saturating_add(line_bytes) > self.segment_bytes {
            state.active_segment = state
                .active_segment
                .max(state.head_segment)
                .saturating_add(1);
        }

        let active_path = self.segment_path(state.active_segment);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_path)
            .await
            .with_context(|| format!("open {}", active_path.display()))?;
        file.write_all(line.as_bytes())
            .await
            .with_context(|| format!("append {}", active_path.display()))?;
        self.write_state(&state).await
    }

    pub async fn drain_batch(&self, limit: usize) -> Result<Vec<QueuedEnvelope>> {
        let _guard = self.io_lock.lock().await;
        self.ensure_storage_exists().await?;
        let state = self.read_state().await?;
        let segments = self.list_segments().await?;
        let mut drained = Vec::new();

        for segment in segments
            .into_iter()
            .filter(|segment| *segment >= state.head_segment)
        {
            let skip = if segment == state.head_segment {
                state.head_offset
            } else {
                0
            };
            let remaining = limit.saturating_sub(drained.len());
            if remaining == 0 {
                break;
            }
            drained.extend(
                read_segment_entries(&self.segment_path(segment), skip, Some(remaining)).await?,
            );
        }

        Ok(drained)
    }

    pub async fn ack_drained(&self, count: usize) -> Result<()> {
        if count == 0 {
            return Ok(());
        }

        let _guard = self.io_lock.lock().await;
        self.ensure_storage_exists().await?;
        let mut state = self.read_state().await?;
        let segments = self.list_segments().await?;
        let mut remaining = count;
        let starting_head_segment = state.head_segment;

        for segment in segments
            .into_iter()
            .filter(|segment| *segment >= starting_head_segment)
        {
            if remaining == 0 {
                break;
            }

            let path = self.segment_path(segment);
            let line_count = count_segment_entries(&path).await?;
            let consumed = if segment == state.head_segment {
                state.head_offset
            } else {
                0
            };
            let available = line_count.saturating_sub(consumed);

            if remaining < available {
                state.head_segment = segment;
                state.head_offset = consumed + remaining;
                break;
            }

            remaining = remaining.saturating_sub(available);
            state.head_segment = segment.saturating_add(1);
            state.head_offset = 0;
            if fs::try_exists(&path).await? {
                fs::remove_file(&path)
                    .await
                    .with_context(|| format!("remove drained segment {}", path.display()))?;
            }
        }

        if state.active_segment < state.head_segment {
            state.active_segment = state.head_segment;
        }

        self.write_state(&state).await
    }

    async fn initialize_storage(&self) -> Result<()> {
        self.ensure_storage_exists().await?;
        if fs::try_exists(&self.path).await? && !fs::metadata(&self.path).await?.is_dir() {
            self.migrate_legacy_queue().await?;
        }

        if !fs::try_exists(&self.state_path()).await? {
            self.write_state(&QueueState::default()).await?;
        }

        Ok(())
    }

    async fn ensure_storage_exists(&self) -> Result<()> {
        fs::create_dir_all(&self.storage_dir)
            .await
            .with_context(|| format!("create {}", self.storage_dir.display()))
    }

    async fn migrate_legacy_queue(&self) -> Result<()> {
        let entries = read_legacy_entries(&self.path).await?;
        if entries.is_empty() {
            fs::remove_file(&self.path)
                .await
                .with_context(|| format!("remove legacy {}", self.path.display()))?;
            return Ok(());
        }

        let mut state = QueueState::default();
        for entry in entries {
            let mut line =
                serde_json::to_string(&entry).context("serialize queue entry during migration")?;
            line.push('\n');
            let line_bytes =
                u64::try_from(line.len()).context("queue entry length does not fit in u64")?;
            let segment_path = self.segment_path(state.active_segment);
            let current_len = file_len(&segment_path).await?;
            if current_len > 0 && current_len.saturating_add(line_bytes) > self.segment_bytes {
                state.active_segment = state.active_segment.saturating_add(1);
            }
            let segment_path = self.segment_path(state.active_segment);
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&segment_path)
                .await
                .with_context(|| format!("open {}", segment_path.display()))?;
            file.write_all(line.as_bytes())
                .await
                .with_context(|| format!("append {}", segment_path.display()))?;
        }

        self.write_state(&state).await?;
        fs::remove_file(&self.path)
            .await
            .with_context(|| format!("remove legacy {}", self.path.display()))
    }

    async fn list_segments(&self) -> Result<Vec<u64>> {
        let mut reader = fs::read_dir(&self.storage_dir)
            .await
            .with_context(|| format!("read {}", self.storage_dir.display()))?;
        let mut segments = Vec::new();

        while let Some(entry) = reader.next_entry().await? {
            let file_type = entry.file_type().await?;
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if name == STATE_FILE_NAME {
                continue;
            }
            if let Some(segment) = parse_segment_name(&name) {
                segments.push(segment);
            }
        }

        segments.sort_unstable();
        Ok(segments)
    }

    async fn read_state(&self) -> Result<QueueState> {
        let path = self.state_path();
        if !fs::try_exists(&path).await? {
            return Ok(QueueState::default());
        }
        let raw = fs::read_to_string(&path)
            .await
            .with_context(|| format!("read {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
    }

    async fn write_state(&self, state: &QueueState) -> Result<()> {
        write_json_atomically(&self.state_path(), state).await
    }

    fn state_path(&self) -> PathBuf {
        self.storage_dir.join(STATE_FILE_NAME)
    }

    fn segment_path(&self, segment: u64) -> PathBuf {
        self.storage_dir.join(segment_file_name(segment))
    }
}

fn shared_queue_lock(path: &Path) -> Arc<Mutex<()>> {
    static QUEUE_LOCKS: OnceLock<StdMutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

    let locks = QUEUE_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

fn queue_storage_dir(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("node-runtime-queue");
    path.with_file_name(format!("{file_name}.segments"))
}

fn segment_file_name(segment: u64) -> String {
    format!("{segment:020}.jsonl")
}

fn parse_segment_name(name: &str) -> Option<u64> {
    name.strip_suffix(".jsonl")?.parse().ok()
}

async fn file_len(path: &Path) -> Result<u64> {
    match fs::metadata(path).await {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error).with_context(|| format!("metadata {}", path.display())),
    }
}

async fn count_segment_entries(path: &Path) -> Result<usize> {
    Ok(read_segment_entries(path, 0, None).await?.len())
}

async fn read_segment_entries(
    path: &Path,
    skip: usize,
    limit: Option<usize>,
) -> Result<Vec<QueuedEnvelope>> {
    if !fs::try_exists(path).await? {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;

    let iter = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .skip(skip)
        .map(|line| serde_json::from_str(line).context("parse queue entry"));

    match limit {
        Some(limit) => iter.take(limit).collect(),
        None => iter.collect(),
    }
}

async fn read_legacy_entries(path: &Path) -> Result<Vec<QueuedEnvelope>> {
    if !fs::try_exists(path).await? {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).context("parse queue entry"))
        .collect()
}

async fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }

    let mut raw = serde_json::to_string(value).context("serialize queue state")?;
    raw.push('\n');
    let tmp_path = atomic_temp_path(path);
    let write_result = async {
        let mut file = fs::File::create(&tmp_path)
            .await
            .with_context(|| format!("write temp {}", tmp_path.display()))?;
        file.write_all(raw.as_bytes())
            .await
            .with_context(|| format!("write temp {}", tmp_path.display()))?;
        file.sync_all()
            .await
            .with_context(|| format!("sync temp {}", tmp_path.display()))?;
        drop(file);
        fs::rename(&tmp_path, path)
            .await
            .with_context(|| format!("rewrite {}", path.display()))
    }
    .await;

    if write_result.is_err() {
        drop(fs::remove_file(&tmp_path).await);
    }

    write_result
}

fn atomic_temp_path(path: &Path) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("queue");
    path.with_file_name(format!("{file_name}.{suffix}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(index: usize) -> QueuedEnvelope {
        QueuedEnvelope::syslog_batch(serde_json::json!({
            "node_id": "test-device",
            "events": [{"message": format!("event-{index}")}]
        }))
    }

    #[tokio::test]
    async fn queue_preserves_fifo_across_segments() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let queue = NodeOutboundQueue::open_with_segment_bytes(
            tempdir.path().join("node-runtime-queue.jsonl"),
            120,
        )
        .await
        .expect("open queue");

        for index in 0..6 {
            queue.push(sample(index)).await.expect("push");
        }

        let drained = queue.drain_batch(10).await.expect("drain");
        assert_eq!(drained.len(), 6);
        for (index, entry) in drained.iter().enumerate() {
            assert_eq!(
                entry.payload["events"][0]["message"].as_str(),
                Some(format!("event-{index}").as_str())
            );
        }

        let segments = queue.list_segments().await.expect("segments");
        assert!(
            segments.len() > 1,
            "expected multiple segments, got {segments:?}"
        );
    }

    #[tokio::test]
    async fn ack_drained_advances_cursor_without_rewriting_head_segment() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let queue = NodeOutboundQueue::open_with_segment_bytes(
            tempdir.path().join("node-runtime-queue.jsonl"),
            1024,
        )
        .await
        .expect("open queue");

        for index in 0..3 {
            queue.push(sample(index)).await.expect("push");
        }

        let segment_path = queue.segment_path(1);
        let before = fs::read_to_string(&segment_path)
            .await
            .expect("read segment");
        queue.ack_drained(1).await.expect("ack");
        let after = fs::read_to_string(&segment_path)
            .await
            .expect("read segment");
        assert_eq!(before, after, "segment file should remain immutable");

        let drained = queue.drain_batch(10).await.expect("drain");
        assert_eq!(drained.len(), 2);
        assert_eq!(
            drained[0].payload["events"][0]["message"].as_str(),
            Some("event-1")
        );
    }

    #[tokio::test]
    async fn ack_drained_removes_fully_consumed_segments() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let queue = NodeOutboundQueue::open_with_segment_bytes(
            tempdir.path().join("node-runtime-queue.jsonl"),
            120,
        )
        .await
        .expect("open queue");

        for index in 0..5 {
            queue.push(sample(index)).await.expect("push");
        }

        let first_segment = queue.segment_path(1);
        assert!(fs::try_exists(&first_segment).await.expect("exists"));
        let first_count = count_segment_entries(&first_segment).await.expect("count");
        queue
            .ack_drained(first_count)
            .await
            .expect("ack first segment");
        assert!(
            !fs::try_exists(&first_segment)
                .await
                .expect("exists after ack")
        );
    }

    #[tokio::test]
    async fn open_migrates_legacy_jsonl_queue() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("node-runtime-queue.jsonl");
        let legacy = [sample(0), sample(1)]
            .into_iter()
            .map(|entry| serde_json::to_string(&entry).expect("serialize"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, format!("{legacy}\n"))
            .await
            .expect("write legacy");

        let queue = NodeOutboundQueue::open(path.clone())
            .await
            .expect("open queue");
        assert!(!fs::try_exists(&path).await.expect("legacy removed"));
        let drained = queue.drain_batch(10).await.expect("drain");
        assert_eq!(drained.len(), 2);
        assert_eq!(
            drained[1].payload["events"][0]["message"].as_str(),
            Some("event-1")
        );
    }
}
