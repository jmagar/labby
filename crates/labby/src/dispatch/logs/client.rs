//! LogSystem bootstrap + env/config resolution.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use super::ingest::{self, IngestCounters};
use super::store::LogStore;
use super::stream::StreamHub;
use super::types::{LogRetention, LogSystem};
use crate::config::LabConfig;
use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::env_non_empty;

// ── Process-global installed LogSystem ────────────────────────────────────────

static INSTALLED: OnceLock<RwLock<Option<Arc<LogSystem>>>> = OnceLock::new();

fn installed_slot() -> &'static RwLock<Option<Arc<LogSystem>>> {
    INSTALLED.get_or_init(|| RwLock::new(None))
}

fn install(system: Arc<LogSystem>) {
    let slot = installed_slot();
    let mut w = slot.write().expect("installed log system lock poisoned");
    *w = Some(system);
}

pub fn require_system() -> Result<Arc<LogSystem>, ToolError> {
    let slot = installed_slot();
    let r = slot.read().expect("installed log system lock poisoned");
    r.as_ref().cloned().ok_or_else(|| {
        ToolError::internal_message("local log system is not installed in this process")
    })
}

#[doc(hidden)]
#[allow(dead_code)]
pub fn clear_installed_log_system_for_test() {
    let slot = installed_slot();
    let mut w = slot.write().expect("installed log system lock poisoned");
    *w = None;
}

// ── Feature flags ─────────────────────────────────────────────────────────────

static INGEST_ENABLED: OnceLock<bool> = OnceLock::new();

pub fn is_ingest_enabled() -> bool {
    *INGEST_ENABLED
        .get_or_init(|| env_non_empty("LAB_LOGS_INGEST_ENABLED").as_deref() == Some("true"))
}

// ── Bootstraps ────────────────────────────────────────────────────────────────

pub async fn bootstrap_running_log_system(
    store_path: PathBuf,
    retention: LogRetention,
    queue_capacity: usize,
    subscriber_capacity: usize,
) -> anyhow::Result<Arc<LogSystem>> {
    let store = Arc::new(LogStore::open(store_path, retention).await?);
    let hub = Arc::new(StreamHub::new(subscriber_capacity));
    let (handle, counters) =
        ingest::spawn_writer(Arc::clone(&store), Arc::clone(&hub), queue_capacity);

    // Run maintenance once at startup to apply retention limits from previous runs.
    if let Err(err) = store.run_maintenance().await {
        tracing::warn!(
            target: "labby::dispatch::logs",
            ?err,
            "startup log maintenance failed"
        );
    }

    // Spawn a periodic maintenance task that runs every hour. The JoinHandle
    // is stored on LogSystem and aborted on drop so the task doesn't outlive
    // the store it holds a reference to.
    let store_for_maintenance = Arc::clone(&store);
    let maintenance_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        interval.tick().await; // skip the first immediate tick (startup already ran)
        loop {
            interval.tick().await;
            if let Err(err) = store_for_maintenance.run_maintenance().await {
                tracing::warn!(
                    target: "labby::dispatch::logs",
                    ?err,
                    "periodic log maintenance failed"
                );
            }
        }
    });

    let system = Arc::new(LogSystem {
        store,
        hub,
        ingest: handle,
        counters,
        maintenance_task,
    });
    install(Arc::clone(&system));
    Ok(system)
}

pub async fn bootstrap_store_backed_log_system(
    store_path: PathBuf,
    retention: LogRetention,
) -> anyhow::Result<Arc<LogSystem>> {
    let store = Arc::new(LogStore::open(store_path, retention).await?);
    let hub = Arc::new(StreamHub::new(1));
    let counters = Arc::new(IngestCounters::new());
    let handle = ingest::readonly_handle(Arc::clone(&counters));

    Ok(Arc::new(LogSystem {
        store,
        hub,
        ingest: handle,
        counters,
        maintenance_task: tokio::spawn(async {}), // no periodic maintenance for read-only bootstrap
    }))
}

// ── Test helpers ──────────────────────────────────────────────────────────────

#[doc(hidden)]
#[allow(dead_code)]
pub async fn bootstrap_running_log_system_for_test(
    queue_capacity: usize,
) -> anyhow::Result<Arc<LogSystem>> {
    let path = unique_test_store_path();
    bootstrap_running_log_system(path, LogRetention::default(), queue_capacity, 32).await
}

#[doc(hidden)]
#[allow(dead_code)]
pub fn bootstrap_log_system_for_test() -> anyhow::Result<Arc<LogSystem>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(bootstrap_running_log_system_for_test(16))
}

#[allow(dead_code)]
fn unique_test_store_path() -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "lab-logs-test-bootstrap-{}-{unique}.db",
        std::process::id()
    ))
}

// ── Config resolvers ──────────────────────────────────────────────────────────

const DEFAULT_DB_PATH_REL: &str = ".lab/logs.db";

pub fn resolve_store_path(config: Option<&LabConfig>) -> PathBuf {
    if let Some(env) = env_non_empty("LAB_LOCAL_LOGS_STORE_PATH") {
        return PathBuf::from(env);
    }
    if let Some(cfg) = config.and_then(|c| c.local_logs.as_ref()) {
        if let Some(p) = &cfg.store_path {
            return p.clone();
        }
    }
    default_store_path()
}

fn default_store_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(DEFAULT_DB_PATH_REL)
}

pub fn resolve_retention(config: Option<&LabConfig>) -> LogRetention {
    let base = config
        .and_then(|c| c.local_logs.as_ref())
        .map(|c| LogRetention {
            max_age_days: c
                .retention_days
                .unwrap_or(LogRetention::default().max_age_days),
            max_bytes: c.max_bytes.unwrap_or(LogRetention::default().max_bytes),
        })
        .unwrap_or_default();

    LogRetention {
        max_age_days: env_non_empty("LAB_LOCAL_LOGS_RETENTION_DAYS")
            .and_then(|s| s.parse().ok())
            .unwrap_or(base.max_age_days),
        max_bytes: env_non_empty("LAB_LOCAL_LOGS_MAX_BYTES")
            .and_then(|s| s.parse().ok())
            .unwrap_or(base.max_bytes),
    }
}

pub fn resolve_queue_capacity(config: Option<&LabConfig>) -> usize {
    env_non_empty("LAB_LOCAL_LOGS_QUEUE_CAPACITY")
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            config
                .and_then(|c| c.local_logs.as_ref())
                .and_then(|c| c.queue_capacity)
        })
        .unwrap_or(1024)
}

pub fn resolve_subscriber_capacity(config: Option<&LabConfig>) -> usize {
    env_non_empty("LAB_LOCAL_LOGS_SUBSCRIBER_CAPACITY")
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            config
                .and_then(|c| c.local_logs.as_ref())
                .and_then(|c| c.subscriber_capacity)
        })
        .unwrap_or(256)
}
