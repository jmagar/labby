//! Shared marketplace stash metadata and snapshot helpers.
//!
//! This module owns the durable `.stash.json` schema plus the `.base/` and
//! `.drift-cache.json` helpers used by marketplace artifact fork/update flows.
//! It intentionally does not dispatch actions.
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempfile::NamedTempFile;
use xxhash_rust::xxh3::xxh3_64;

use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::client;
use crate::dispatch::path_safety;

const STASH_META_FILE: &str = ".stash.json";
const STASH_LOCK_FILE: &str = ".stash.lock";
const BASE_DIR: &str = ".base";
const DRIFT_CACHE_FILE: &str = ".drift-cache.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashMeta {
    pub schema_version: u8,
    pub fork_type: ForkType,
    pub upstream_id: String,
    pub upstream_version: String,
    pub upstream_commit: Option<String>,
    pub forked_at: String,
    pub forked_artifacts: Vec<String>,
    #[serde(default)]
    pub content_hashes: HashMap<String, String>,
    pub patch_records: Vec<PatchRecord>,
    pub update_config: UpdateConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForkType {
    Plugin,
    Artifact,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchRecord {
    pub path: String,
    pub patched_at: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateConfig {
    pub strategy: ConflictStrategy,
    pub notify: bool,
    pub last_check_at: Option<String>,
    pub last_check_result: Option<bool>,
    pub check_ttl_secs: Option<u64>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            strategy: ConflictStrategy::AlwaysAsk,
            notify: true,
            last_check_at: None,
            last_check_result: None,
            check_ttl_secs: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStrategy {
    KeepMine,
    TakeUpstream,
    #[default]
    AlwaysAsk,
    AiSuggest,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct DriftCache {
    pub entries: HashMap<String, DriftCacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DriftCacheEntry {
    pub last_verified_at: String,
    pub file_hash: String,
    pub file_mtime_secs: i64,
    pub file_size: u64,
    pub base_hash: String,
}

impl DriftCacheEntry {
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.file_hash != self.base_hash
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftStatus {
    Clean,
    Dirty,
    Deleted,
    BaseMissing,
}

pub fn acquire_stash_lock(stash_dir: &Path) -> Result<File, ToolError> {
    std::fs::create_dir_all(stash_dir).map_err(client::io_internal)?;
    let lock_path = stash_dir.join(STASH_LOCK_FILE);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(client::io_internal)?;
    fs4::FileExt::lock(&file).map_err(client::io_internal)?;
    Ok(file)
}

pub fn read_stash_meta(stash_dir: &Path) -> Result<Option<StashMeta>, ToolError> {
    let path = stash_dir.join(STASH_META_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(client::io_internal(error)),
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(decode_error)?;
    let Some(schema_version) = value.get("schema_version").and_then(Value::as_u64) else {
        return Ok(None);
    };
    if schema_version == 0 {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(decode_error)
}

pub fn write_stash_meta(stash_dir: &Path, meta: &StashMeta) -> Result<(), ToolError> {
    let _lock = acquire_stash_lock(stash_dir)?;
    write_json_atomic(&stash_dir.join(STASH_META_FILE), meta)
}

pub fn read_base_snapshot(stash_dir: &Path, rel_path: &str) -> Result<Option<String>, ToolError> {
    let path = base_snapshot_path(stash_dir, rel_path)?;
    match path_safety::reject_symlink(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == "not_found" => return Ok(None),
        Err(error) => return Err(error),
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(client::io_internal(error)),
    }
}

pub fn write_base_snapshot(
    stash_dir: &Path,
    rel_path: &str,
    content: &str,
) -> Result<(), ToolError> {
    let dest = base_snapshot_path(stash_dir, rel_path)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(client::io_internal)?;
    }
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&dest) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            path_safety::reject_symlink(&dest)?;
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&dest)
                .map_err(client::io_internal)?
        }
        Err(error) => return Err(client::io_internal(error)),
    };
    file.write_all(content.as_bytes())
        .map_err(client::io_internal)?;
    file.sync_all().map_err(client::io_internal)?;
    Ok(())
}

pub fn delete_base_snapshot(stash_dir: &Path, rel_path: &str) -> Result<(), ToolError> {
    let path = base_snapshot_path(stash_dir, rel_path)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(client::io_internal(error)),
    }
}

pub fn list_base_snapshots(stash_dir: &Path) -> Result<Vec<String>, ToolError> {
    let root = stash_dir.join(BASE_DIR);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut paths = BTreeSet::new();
    collect_base_snapshots(&root, &root, &mut paths)?;
    Ok(paths.into_iter().collect())
}

pub fn validate_rel_path(rel_path: &str) -> Result<(), ToolError> {
    if rel_path.is_empty() {
        return Err(invalid_param("rel_path", "must not be empty"));
    }
    if rel_path.as_bytes().contains(&0) {
        return Err(invalid_param("rel_path", "must not contain null bytes"));
    }
    if rel_path.contains('\\') {
        return Err(invalid_param("rel_path", "path traversal not allowed"));
    }
    for component in Path::new(rel_path).components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err(invalid_param("rel_path", "path traversal not allowed")),
        }
    }
    Ok(())
}

pub fn read_drift_cache(stash_dir: &Path) -> Result<DriftCache, ToolError> {
    let path = stash_dir.join(DRIFT_CACHE_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DriftCache::default());
        }
        Err(error) => return Err(client::io_internal(error)),
    };
    serde_json::from_slice(&bytes).map_err(decode_error)
}

pub fn write_drift_cache(stash_dir: &Path, cache: &DriftCache) -> Result<(), ToolError> {
    write_json_atomic(&stash_dir.join(DRIFT_CACHE_FILE), cache)
}

pub fn check_drift(
    stash_dir: &Path,
    rel_path: &str,
    cache: &mut DriftCache,
) -> Result<DriftStatus, ToolError> {
    validate_rel_path(rel_path)?;
    let base_path = stash_dir.join(BASE_DIR).join(rel_path);
    if !base_path.exists() {
        return Ok(DriftStatus::BaseMissing);
    }
    path_safety::reject_symlink(&base_path)?;

    let working_path = stash_dir.join(rel_path);
    let metadata = match std::fs::metadata(&working_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DriftStatus::Deleted);
        }
        Err(error) => return Err(client::io_internal(error)),
    };
    let mtime_secs = metadata_time_secs(metadata.modified().map_err(client::io_internal)?);
    let file_size = metadata.len();
    if let Some(entry) = cache.entries.get(rel_path) {
        if entry.file_mtime_secs == mtime_secs && entry.file_size == file_size {
            return Ok(status_from_dirty(entry.is_dirty()));
        }
    }

    let file_hash = hash_file(&working_path)?;
    let base_hash = compute_base_hash(stash_dir, rel_path)?;
    let entry = DriftCacheEntry {
        last_verified_at: jiff::Timestamp::now().to_string(),
        file_hash,
        file_mtime_secs: mtime_secs,
        file_size,
        base_hash,
    };
    let status = status_from_dirty(entry.is_dirty());
    cache.entries.insert(rel_path.to_string(), entry);
    Ok(status)
}

pub fn compute_base_hash(stash_dir: &Path, rel_path: &str) -> Result<String, ToolError> {
    let path = base_snapshot_path(stash_dir, rel_path)?;
    if !path.exists() {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: "base snapshot is missing".into(),
        });
    }
    path_safety::reject_symlink(&path)?;
    hash_file(&path)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), ToolError> {
    let Some(parent) = path.parent() else {
        return Err(ToolError::internal_message("target path has no parent"));
    };
    std::fs::create_dir_all(parent).map_err(client::io_internal)?;
    let mut temp = NamedTempFile::new_in(parent).map_err(client::io_internal)?;
    let bytes = serde_json::to_vec_pretty(value).map_err(client::io_internal)?;
    temp.write_all(&bytes).map_err(client::io_internal)?;
    temp.as_file().sync_all().map_err(client::io_internal)?;
    temp.persist(path)
        .map_err(|error| client::io_internal(error.error))?;
    Ok(())
}

fn base_snapshot_path(stash_dir: &Path, rel_path: &str) -> Result<PathBuf, ToolError> {
    validate_rel_path(rel_path)?;
    Ok(stash_dir.join(BASE_DIR).join(rel_path))
}

fn collect_base_snapshots(
    root: &Path,
    current: &Path,
    out: &mut BTreeSet<String>,
) -> Result<(), ToolError> {
    for entry in std::fs::read_dir(current).map_err(client::io_internal)? {
        let entry = entry.map_err(client::io_internal)?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(client::io_internal)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_base_snapshots(root, &path, out)?;
            continue;
        }
        let relative = path.strip_prefix(root).map_err(client::io_internal)?;
        let Some(relative) = relative.to_str() else {
            return Err(ToolError::internal_message(
                "base snapshot path is not valid UTF-8",
            ));
        };
        // Snapshot keys are canonical forward-slash relative paths (the same
        // form `write_base_snapshot` accepts); normalize the OS-native
        // separator so Windows backslashes are not rejected by
        // `validate_rel_path` (which treats `\` as traversal).
        let relative = relative.replace('\\', "/");
        validate_rel_path(&relative)?;
        out.insert(relative);
    }
    Ok(())
}

fn metadata_time_secs(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(error) => -i64::try_from(error.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

fn status_from_dirty(dirty: bool) -> DriftStatus {
    if dirty {
        DriftStatus::Dirty
    } else {
        DriftStatus::Clean
    }
}

fn hash_file(path: &Path) -> Result<String, ToolError> {
    let bytes = std::fs::read(path).map_err(client::io_internal)?;
    Ok(hash_bytes(&bytes))
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:016x}", xxh3_64(bytes))
}

fn decode_error(error: serde_json::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "decode_error".into(),
        message: format!("decode stash metadata JSON: {error}"),
    }
}

fn invalid_param(param: &str, message: &str) -> ToolError {
    ToolError::InvalidParam {
        param: param.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn sample_meta() -> StashMeta {
        StashMeta {
            schema_version: 1,
            fork_type: ForkType::Artifact,
            upstream_id: "demo-plugin@demo-market".into(),
            upstream_version: "1.2.3".into(),
            upstream_commit: Some("abc123".into()),
            forked_at: "2026-04-25T12:00:00Z".into(),
            forked_artifacts: vec!["agents/foo.md".into()],
            content_hashes: HashMap::from([("agents/foo.md".into(), "hash-a".into())]),
            patch_records: vec![PatchRecord {
                path: "agents/foo.md".into(),
                patched_at: "2026-04-25T12:01:00Z".into(),
                description: Some("demo patch".into()),
            }],
            update_config: UpdateConfig {
                strategy: ConflictStrategy::KeepMine,
                notify: false,
                last_check_at: Some("2026-04-25T12:02:00Z".into()),
                last_check_result: Some(true),
                check_ttl_secs: Some(60),
            },
        }
    }

    #[test]
    fn validate_rel_path_accepts_normal_relative_paths() {
        assert!(validate_rel_path("agents/foo.md").is_ok());
        assert!(validate_rel_path("skills/bar/baz.md").is_ok());
    }

    #[test]
    fn validate_rel_path_rejects_invalid_paths() {
        for path in [
            "../secrets",
            "/etc/passwd",
            "a/../b",
            "bad\0path",
            "",
            r"C:\windows",
        ] {
            let err = validate_rel_path(path).expect_err("path must reject");
            assert_eq!(err.kind(), "invalid_param");
        }
    }

    #[test]
    fn read_stash_meta_returns_none_when_absent() {
        let dir = tempdir().expect("tempdir");
        let meta = read_stash_meta(dir.path()).expect("read metadata");
        assert!(meta.is_none());
    }

    #[test]
    fn read_stash_meta_returns_none_for_missing_or_zero_schema() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(STASH_META_FILE),
            json!({ "upstream_id": "demo" }).to_string(),
        )
        .expect("write missing schema");
        assert!(
            read_stash_meta(dir.path())
                .expect("read missing schema")
                .is_none()
        );

        std::fs::write(
            dir.path().join(STASH_META_FILE),
            json!({ "schema_version": 0, "upstream_id": "demo" }).to_string(),
        )
        .expect("write zero schema");
        assert!(
            read_stash_meta(dir.path())
                .expect("read zero schema")
                .is_none()
        );
    }

    #[test]
    fn write_and_read_stash_meta_roundtrips_all_fields() {
        let dir = tempdir().expect("tempdir");
        let meta = sample_meta();
        write_stash_meta(dir.path(), &meta).expect("write metadata");
        let read = read_stash_meta(dir.path())
            .expect("read metadata")
            .expect("metadata exists");

        assert_eq!(read.schema_version, 1);
        assert_eq!(read.fork_type, ForkType::Artifact);
        assert_eq!(read.upstream_id, meta.upstream_id);
        assert_eq!(read.upstream_version, meta.upstream_version);
        assert_eq!(read.upstream_commit, meta.upstream_commit);
        assert_eq!(read.forked_at, meta.forked_at);
        assert_eq!(read.forked_artifacts, meta.forked_artifacts);
        assert_eq!(read.content_hashes, meta.content_hashes);
        assert_eq!(read.patch_records, meta.patch_records);
        assert_eq!(read.update_config, meta.update_config);
    }

    #[test]
    fn base_snapshot_read_write_delete_and_list_work() {
        let dir = tempdir().expect("tempdir");
        assert!(
            read_base_snapshot(dir.path(), "agents/foo.md")
                .expect("read absent snapshot")
                .is_none()
        );

        write_base_snapshot(dir.path(), "agents/foo.md", "hello").expect("write snapshot");
        write_base_snapshot(dir.path(), "skills/bar/baz.md", "world")
            .expect("write nested snapshot");

        assert_eq!(
            read_base_snapshot(dir.path(), "agents/foo.md").expect("read snapshot"),
            Some("hello".into())
        );
        assert_eq!(
            list_base_snapshots(dir.path()).expect("list snapshots"),
            vec!["agents/foo.md".to_string(), "skills/bar/baz.md".to_string()]
        );

        delete_base_snapshot(dir.path(), "agents/foo.md").expect("delete snapshot");
        assert!(
            read_base_snapshot(dir.path(), "agents/foo.md")
                .expect("read deleted snapshot")
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_base_snapshot_rejects_symlink_destination() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join(BASE_DIR);
        std::fs::create_dir_all(base.join("agents")).expect("create base dir");
        let target = dir.path().join("outside.txt");
        std::fs::write(&target, "outside").expect("write target");
        std::os::unix::fs::symlink(&target, base.join("agents/foo.md")).expect("symlink");

        let err = write_base_snapshot(dir.path(), "agents/foo.md", "replacement")
            .expect_err("symlink must reject");
        assert_eq!(err.kind(), "symlink_rejected");
        assert_eq!(
            std::fs::read_to_string(target).expect("read target"),
            "outside"
        );
    }

    #[test]
    fn stash_lock_blocks_second_writer_until_first_is_dropped() {
        let dir = tempdir().expect("tempdir");
        let first = acquire_stash_lock(dir.path()).expect("first lock");
        let path = dir.path().to_path_buf();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            started_tx.send(()).expect("send started");
            let second = acquire_stash_lock(&path).expect("second lock");
            done_tx.send(()).expect("send done");
            drop(second);
        });

        started_rx.recv().expect("receive started");
        assert!(done_rx.try_recv().is_err());
        drop(first);
        done_rx.recv().expect("receive done");
        handle.join().expect("join thread");
    }

    #[test]
    fn drift_cache_detects_clean_dirty_deleted_and_base_missing() {
        let dir = tempdir().expect("tempdir");
        write_base_snapshot(dir.path(), "agents/foo.md", "base").expect("write base");
        std::fs::create_dir_all(dir.path().join("agents")).expect("create working parent");
        std::fs::write(dir.path().join("agents/foo.md"), "base").expect("write working");

        let mut cache = DriftCache::default();
        assert_eq!(
            check_drift(dir.path(), "agents/foo.md", &mut cache).expect("check clean"),
            DriftStatus::Clean
        );
        assert_eq!(
            check_drift(dir.path(), "agents/foo.md", &mut cache).expect("cached clean"),
            DriftStatus::Clean
        );

        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(dir.path().join("agents/foo.md"), "changed").expect("change working");
        assert_eq!(
            check_drift(dir.path(), "agents/foo.md", &mut cache).expect("check dirty"),
            DriftStatus::Dirty
        );

        std::fs::remove_file(dir.path().join("agents/foo.md")).expect("delete working");
        assert_eq!(
            check_drift(dir.path(), "agents/foo.md", &mut cache).expect("check deleted"),
            DriftStatus::Deleted
        );

        std::fs::create_dir_all(dir.path().join("agents")).expect("recreate working parent");
        std::fs::write(dir.path().join("agents/missing.md"), "working")
            .expect("write missing-base working");
        assert_eq!(
            check_drift(dir.path(), "agents/missing.md", &mut cache).expect("check base missing"),
            DriftStatus::BaseMissing
        );
    }

    #[test]
    fn drift_cache_roundtrips_and_base_hash_is_stable() {
        let dir = tempdir().expect("tempdir");
        write_base_snapshot(dir.path(), "agents/foo.md", "base").expect("write base");
        let base_hash = compute_base_hash(dir.path(), "agents/foo.md").expect("base hash");
        let cache = DriftCache {
            entries: HashMap::from([(
                "agents/foo.md".into(),
                DriftCacheEntry {
                    last_verified_at: "2026-04-25T12:00:00Z".into(),
                    file_hash: base_hash.clone(),
                    file_mtime_secs: 1,
                    file_size: 4,
                    base_hash,
                },
            )]),
        };

        write_drift_cache(dir.path(), &cache).expect("write drift cache");
        let read = read_drift_cache(dir.path()).expect("read drift cache");
        assert_eq!(read, cache);
    }
}
