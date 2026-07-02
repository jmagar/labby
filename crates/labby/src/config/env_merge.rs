//! Shared `.env` merge primitive.
//!
//! `setup.draft.commit` and gateway config mutations write to `~/.labby/.env`
//! through this module. This is the only sanctioned way to mutate the file:
//!
//! 1. Backup is part of merge: `.env` → `.env.bak.<unix-seconds>` before any
//!    write. Backup retention pruned to the last 10 entries.
//! 2. Atomic write via [`tempfile::NamedTempFile::new_in`] (same directory) +
//!    [`File::sync_all`] + [`tempfile::NamedTempFile::persist`].
//! 3. Existing key order is preserved.
//! 4. Comments (`#`) and blank lines pass through unchanged.
//! 5. Dedupe by key — one entry per key in the output.
//! 6. Conflicts (key exists with different value): skip-and-warn unless
//!    `force = true`.
//! 7. Values containing whitespace, `#`, or shell metacharacters are
//!    double-quoted with `\"` / `\\` escaping.
//! 8. Idempotence: running merge with the same inputs is a no-op (no backup,
//!    no rewrite).
//!
//! On Unix the resulting file is chmod 0600.

use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use tempfile::NamedTempFile;

/// Maximum number of `.env.bak.*` files retained after a successful merge.
pub const BACKUP_RETENTION: usize = 10;

/// Process-wide counter used to disambiguate same-millisecond backup names
/// without spinning on filesystem existence checks.
static BACKUP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Single key-value entry to merge into the target file.
#[derive(Debug, Clone)]
pub struct EnvEntry {
    pub key: String,
    pub value: String,
}

impl EnvEntry {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// Caller-supplied request for [`merge`].
#[derive(Debug, Clone, Default)]
pub struct MergeRequest {
    /// Entries to merge in the order they should be considered.
    pub entries: Vec<EnvEntry>,
    /// When `true`, conflicting keys are overwritten instead of skipped.
    pub force: bool,
    /// When `Some(mtime)`, abort with [`MergeError::WriteConflict`] if the
    /// target's current mtime differs (mtime-skew). Pass [`snapshot_mtime`]
    /// taken at read time to detect interleaved writers.
    pub expected_mtime: Option<SystemTime>,
}

/// Outcome of a successful [`merge`].
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct MergeOutcome {
    /// Number of entries that resulted in a key change (new key or override).
    pub written: usize,
    /// Human-readable warnings for skipped conflicts (`force=false`).
    pub skipped: Vec<String>,
    /// Path of the `.env.bak.<ts>` backup, if a backup was created. None
    /// indicates idempotent no-op (file unchanged).
    pub backup_path: Option<PathBuf>,
    /// Backup retention stats after this merge.
    pub pruned: PruneStats,
}

/// Dry-run classification for a merge request.
#[cfg(test)]
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MergePreview {
    pub changes: Vec<PreviewChange>,
    pub skipped: Vec<String>,
    pub written: usize,
}

#[cfg(test)]
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PreviewChange {
    pub key: String,
    pub status: PreviewStatus,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewStatus {
    Add,
    Update,
    Unchanged,
    Conflict,
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct PruneStats {
    pub kept: usize,
    pub removed: usize,
}

/// Stable error kinds for merge failures. Surface this through `kind()` when
/// building [`crate::dispatch::error::ToolError`] envelopes.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum MergeError {
    #[error("create temp file in {parent}: {source}")]
    TempCreate {
        parent: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("sync temp file: {source}")]
    SyncFailed {
        #[source]
        source: std::io::Error,
    },
    #[error("persist temp file across filesystems: {source}")]
    PersistCrossFs {
        #[source]
        source: std::io::Error,
    },
    #[error("write conflict ({reason}) on {path}")]
    WriteConflict {
        path: PathBuf,
        reason: WriteConflictReason,
    },
    #[error("write {path}: {reason:?}")]
    WriteFailed {
        path: PathBuf,
        reason: WriteFailReason,
    },
    #[error("rollback failed; backup retained at {backup_path}: {source}")]
    CommitRollbackFailed {
        backup_path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum WriteConflictReason {
    MtimeSkew,
}

impl std::fmt::Display for WriteConflictReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MtimeSkew => write!(f, "mtime_skew"),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum WriteFailReason {
    StorageFull,
    PermissionDenied,
    Other(String),
}

impl WriteFailReason {
    fn from_io(err: &std::io::Error) -> Self {
        match err.kind() {
            ErrorKind::StorageFull => Self::StorageFull,
            ErrorKind::PermissionDenied => Self::PermissionDenied,
            _ => Self::Other(err.to_string()),
        }
    }
}

impl MergeError {
    /// Stable error kind string for dispatch envelopes (see `docs/dev/ERRORS.md`).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::TempCreate { .. } => "merge_temp_create",
            Self::SyncFailed { .. } => "merge_sync_failed",
            Self::PersistCrossFs { .. } => "merge_persist_cross_fs",
            Self::WriteConflict { .. } => "merge_write_conflict",
            Self::WriteFailed { .. } => "write_failed",
            Self::CommitRollbackFailed { .. } => "commit_rollback_failed",
        }
    }
}

/// Read the current mtime of `path`, returning `None` if absent or unreadable.
pub fn snapshot_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Merge `req.entries` into `path`, writing atomically with backup + prune.
pub fn merge(path: &Path, req: MergeRequest) -> Result<MergeOutcome, MergeError> {
    let parent = path.parent().unwrap_or(Path::new(".")).to_path_buf();

    if !parent.exists() {
        fs::create_dir_all(&parent).map_err(|e| MergeError::WriteFailed {
            path: parent.clone(),
            reason: WriteFailReason::from_io(&e),
        })?;
    }

    // Read existing file (empty if absent).
    let existing_raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(MergeError::WriteFailed {
                path: path.to_path_buf(),
                reason: WriteFailReason::from_io(&e),
            });
        }
    };

    if let Some(expected) = req.expected_mtime
        && let Some(current) = snapshot_mtime(path)
        && current != expected
    {
        return Err(MergeError::WriteConflict {
            path: path.to_path_buf(),
            reason: WriteConflictReason::MtimeSkew,
        });
    }

    let existing_lines: Vec<&str> = existing_raw.lines().collect();
    let mut existing_map: HashMap<String, String> = HashMap::new();
    for line in &existing_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            existing_map.insert(k.trim().to_owned(), strip_quotes(v.trim()).to_owned());
        }
    }

    // Collapse duplicate request keys before comparing against existing state.
    // Last value wins, while the output order follows the first occurrence.
    let mut request_entries: Vec<EnvEntry> = Vec::new();
    for entry in &req.entries {
        if let Some(slot) = request_entries
            .iter_mut()
            .find(|existing| existing.key == entry.key)
        {
            slot.value = entry.value.clone();
        } else {
            request_entries.push(entry.clone());
        }
    }

    // Classify each final entry.
    let mut skipped: Vec<String> = Vec::new();
    let mut overrides: HashMap<String, String> = HashMap::new();
    let mut new_keys: Vec<(String, String)> = Vec::new();
    let mut written_count: usize = 0;

    for entry in &request_entries {
        match existing_map.get(&entry.key) {
            None => {
                new_keys.push((entry.key.clone(), entry.value.clone()));
                written_count += 1;
            }
            Some(existing_val) if existing_val == &entry.value => {
                // Idempotent — no change.
            }
            Some(_) => {
                if req.force {
                    overrides.insert(entry.key.clone(), entry.value.clone());
                    written_count += 1;
                } else {
                    skipped.push(conflict_warning(&entry.key));
                }
            }
        }
    }

    // Idempotent fast path: nothing to write, nothing to back up.
    if overrides.is_empty() && new_keys.is_empty() {
        return Ok(MergeOutcome {
            written: 0,
            skipped,
            backup_path: None,
            pruned: PruneStats::default(),
        });
    }

    // Backup BEFORE any write.
    let backup_path = if path.exists() {
        Some(create_backup(path)?)
    } else {
        None
    };

    // Build output line-by-line.
    let mut out_lines: Vec<String> = Vec::with_capacity(existing_lines.len() + new_keys.len() + 1);
    for line in &existing_lines {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && let Some((k, _)) = trimmed.split_once('=')
        {
            let key = k.trim();
            if let Some(new_val) = overrides.get(key) {
                out_lines.push(format!("{key}={}", quote_value(new_val)));
                continue;
            }
        }
        out_lines.push((*line).to_owned());
    }

    if !new_keys.is_empty() {
        if !out_lines.last().is_none_or(|l| l.trim().is_empty()) {
            out_lines.push(String::new());
        }
        for (key, value) in &new_keys {
            out_lines.push(format!("{key}={}", quote_value(value)));
        }
    }

    // Atomic write.
    write_atomically(path, &out_lines, &parent)?;

    // Prune backups (post-write).
    let pruned = prune_backups(&parent, path).unwrap_or_default();

    Ok(MergeOutcome {
        written: written_count,
        skipped,
        backup_path,
        pruned,
    })
}

/// Classify a merge without writing, backing up, or pruning files.
#[cfg(test)]
pub fn preview(path: &Path, req: &MergeRequest) -> Result<MergePreview, MergeError> {
    let existing_raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(MergeError::WriteFailed {
                path: path.to_path_buf(),
                reason: WriteFailReason::from_io(&e),
            });
        }
    };

    let mut existing_map: HashMap<String, String> = HashMap::new();
    for line in existing_raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            existing_map.insert(k.trim().to_owned(), strip_quotes(v.trim()).to_owned());
        }
    }

    let mut request_entries: Vec<EnvEntry> = Vec::new();
    for entry in &req.entries {
        if let Some(slot) = request_entries
            .iter_mut()
            .find(|existing| existing.key == entry.key)
        {
            slot.value = entry.value.clone();
        } else {
            request_entries.push(entry.clone());
        }
    }

    let mut preview = MergePreview::default();
    for entry in request_entries {
        match existing_map.get(&entry.key) {
            None => {
                preview.written += 1;
                preview.changes.push(PreviewChange {
                    key: entry.key,
                    status: PreviewStatus::Add,
                });
            }
            Some(existing_val) if existing_val == &entry.value => {
                preview.changes.push(PreviewChange {
                    key: entry.key,
                    status: PreviewStatus::Unchanged,
                });
            }
            Some(_) if req.force => {
                preview.written += 1;
                preview.changes.push(PreviewChange {
                    key: entry.key,
                    status: PreviewStatus::Update,
                });
            }
            Some(_) => {
                preview.skipped.push(conflict_warning(&entry.key));
                preview.changes.push(PreviewChange {
                    key: entry.key,
                    status: PreviewStatus::Conflict,
                });
            }
        }
    }
    Ok(preview)
}

fn conflict_warning(key: &str) -> String {
    format!("CONFLICT: {key} already set; skipping (set force=true to overwrite)")
}

fn write_atomically(path: &Path, lines: &[String], parent: &Path) -> Result<(), MergeError> {
    let mut tmp = NamedTempFile::new_in(parent).map_err(|e| MergeError::TempCreate {
        parent: parent.to_path_buf(),
        source: e,
    })?;
    {
        let file = tmp.as_file_mut();
        for line in lines {
            writeln!(file, "{line}").map_err(|e| MergeError::WriteFailed {
                path: path.to_path_buf(),
                reason: WriteFailReason::from_io(&e),
            })?;
        }
        file.sync_all()
            .map_err(|e| MergeError::SyncFailed { source: e })?;
    }
    tmp.persist(path).map_err(|persist_err| {
        let io_err = persist_err.error;
        // EXDEV (18 on Linux) signals a cross-filesystem rename; the temp must
        // be in the same directory as the target to avoid this.
        if io_err.raw_os_error() == Some(18) {
            MergeError::PersistCrossFs { source: io_err }
        } else {
            MergeError::WriteFailed {
                path: path.to_path_buf(),
                reason: WriteFailReason::from_io(&io_err),
            }
        }
    })?;
    // fsync the parent directory so the rename is durable across power loss.
    // tempfile::persist guarantees atomicity on the rename, but Linux durability
    // requires an additional fsync on the parent to flush the directory entry.
    if let Ok(dir) = fs::File::open(parent) {
        dir.sync_all().ok();
    }
    set_secure_perms(path);
    Ok(())
}

#[cfg(unix)]
fn set_secure_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = fs::metadata(path) {
        let mut perms = metadata.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms).ok();
    }
}

#[cfg(not(unix))]
fn set_secure_perms(_path: &Path) {
    // Windows ACL deferred (v2). See bg3e.3 known-limitation note.
}

fn create_backup(path: &Path) -> Result<PathBuf, MergeError> {
    // Millisecond timestamp + monotonic process counter + pid disambiguates
    // same-millisecond commits without filesystem existence checks.
    let ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let counter = BACKUP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let backup = PathBuf::from(format!("{}.bak.{ms}.{pid}.{counter}", path.display()));
    fs::copy(path, &backup).map_err(|e| MergeError::WriteFailed {
        path: backup.clone(),
        reason: WriteFailReason::from_io(&e),
    })?;
    Ok(backup)
}

fn prune_backups(parent: &Path, target: &Path) -> std::io::Result<PruneStats> {
    let target_name = target.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if target_name.is_empty() {
        return Ok(PruneStats::default());
    }
    let prefix = format!("{target_name}.bak.");
    let mut backups: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.starts_with(&prefix) {
            continue;
        }
        let mtime = entry.metadata()?.modified()?;
        backups.push((entry.path(), mtime));
    }
    if backups.len() <= BACKUP_RETENTION {
        return Ok(PruneStats {
            kept: backups.len(),
            removed: 0,
        });
    }
    backups.sort_by_key(|(_, mtime)| *mtime);
    let to_remove = backups.len() - BACKUP_RETENTION;
    let mut removed = 0;
    for (path, _) in backups.iter().take(to_remove) {
        match fs::remove_file(path) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(
                subsystem = "env_merge",
                phase = "backup.prune",
                path = %path.display(),
                error = %e,
                "could not remove old backup; continuing"
            ),
        }
    }
    Ok(PruneStats {
        kept: BACKUP_RETENTION,
        removed,
    })
}

/// Restore a backup over `target`. Used by setup.draft.commit on rollback.
#[allow(dead_code)]
pub fn restore_backup(target: &Path, backup: &Path) -> Result<(), MergeError> {
    fs::copy(backup, target).map_err(|e| MergeError::CommitRollbackFailed {
        backup_path: backup.to_path_buf(),
        source: e,
    })?;
    set_secure_perms(target);
    Ok(())
}

fn quote_value(value: &str) -> String {
    let needs_quote = value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '#' | '"' | '\\' | '$' | '`' | '\''));
    if !needs_quote {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str(r#"\""#),
            '\\' => out.push_str(r"\\"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Strip enclosing double quotes from a serialized `.env` value and undo
/// the `\"` / `\\` escapes applied by [`quote_value`]. Pub so the dispatch
/// layer can use the same parser when reading `.env.draft` directly
/// (no second copy of the same logic in `dispatch/setup/draft.rs`).
#[must_use]
pub fn strip_quotes(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1]
            .replace(r#"\""#, "\"")
            .replace(r"\\", r"\")
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    fn write_initial(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn idempotent_no_change_skips_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_initial(dir.path(), ".env", "FOO=bar\n");
        let outcome = merge(
            &path,
            MergeRequest {
                entries: vec![EnvEntry::new("FOO", "bar")],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.written, 0);
        assert!(outcome.backup_path.is_none());
    }

    #[test]
    fn appends_new_key_with_blank_separator() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_initial(dir.path(), ".env", "FOO=bar\n");
        merge(
            &path,
            MergeRequest {
                entries: vec![EnvEntry::new("BAR", "baz")],
                ..Default::default()
            },
        )
        .unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("FOO=bar"));
        assert!(after.contains("BAR=baz"));
        // FOO must precede BAR (order preservation + append semantics).
        assert!(after.find("FOO=").unwrap() < after.find("BAR=").unwrap());
    }

    #[test]
    fn preserves_comments_and_blank_lines() {
        let dir = tempfile::tempdir().unwrap();
        let initial = "# top comment\n\nFOO=1\n# inline\nBAR=2\n";
        let path = write_initial(dir.path(), ".env", initial);
        merge(
            &path,
            MergeRequest {
                entries: vec![EnvEntry::new("FOO", "1.5")],
                force: true,
                ..Default::default()
            },
        )
        .unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("# top comment"));
        assert!(after.contains("# inline"));
        assert!(after.contains("FOO=1.5"));
        assert!(after.contains("BAR=2"));
        // Blank line preserved between comment and FOO.
        assert!(after.contains("# top comment\n\nFOO="));
    }

    #[test]
    fn skip_and_warn_on_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_initial(dir.path(), ".env", "FOO=bar\n");
        let outcome = merge(
            &path,
            MergeRequest {
                entries: vec![EnvEntry::new("FOO", "baz")],
                force: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.written, 0);
        assert_eq!(outcome.skipped.len(), 1);
        assert!(!outcome.skipped[0].contains("bar"));
        assert!(fs::read_to_string(&path).unwrap().contains("FOO=bar"));
    }

    #[test]
    fn preview_classifies_without_writing_or_leaking_existing_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_initial(dir.path(), ".env", "FOO=secret\nBAR=same\n");
        let outcome = preview(
            &path,
            &MergeRequest {
                entries: vec![
                    EnvEntry::new("FOO", "new-secret"),
                    EnvEntry::new("BAR", "same"),
                    EnvEntry::new("BAZ", "new"),
                ],
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(outcome.written, 1);
        assert_eq!(outcome.skipped.len(), 1);
        assert!(!outcome.skipped[0].contains("secret"));
        assert!(fs::read_to_string(&path).unwrap().contains("FOO=secret"));
    }

    #[test]
    fn force_overwrite_replaces_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_initial(dir.path(), ".env", "FOO=bar\n");
        let outcome = merge(
            &path,
            MergeRequest {
                entries: vec![EnvEntry::new("FOO", "baz")],
                force: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.written, 1);
        assert!(outcome.backup_path.is_some());
        assert!(fs::read_to_string(&path).unwrap().contains("FOO=baz"));
    }

    #[test]
    fn quotes_values_with_whitespace_and_metachars() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        merge(
            &path,
            MergeRequest {
                entries: vec![
                    EnvEntry::new("WITH_SPACES", "hello world"),
                    EnvEntry::new("WITH_HASH", "abc#def"),
                    EnvEntry::new("PLAIN", "alphanum"),
                ],
                ..Default::default()
            },
        )
        .unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains(r#"WITH_SPACES="hello world""#));
        assert!(after.contains(r#"WITH_HASH="abc#def""#));
        assert!(after.contains("PLAIN=alphanum"));
    }

    #[test]
    fn mtime_skew_returns_write_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_initial(dir.path(), ".env", "FOO=bar\n");
        let stale = SystemTime::UNIX_EPOCH;
        let err = merge(
            &path,
            MergeRequest {
                entries: vec![EnvEntry::new("FOO", "baz")],
                force: true,
                expected_mtime: Some(stale),
            },
        )
        .unwrap_err();
        assert_eq!(err.kind(), "merge_write_conflict");
        match err {
            MergeError::WriteConflict { reason, .. } => {
                assert!(matches!(reason, WriteConflictReason::MtimeSkew));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn backup_pruning_keeps_last_ten() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_initial(dir.path(), ".env", "FOO=0\n");
        for i in 1..=15 {
            // Force a backup each iteration by changing the value.
            merge(
                &path,
                MergeRequest {
                    entries: vec![EnvEntry::new("FOO", i.to_string())],
                    force: true,
                    ..Default::default()
                },
            )
            .unwrap();
            // Tweak mtime so each backup has a unique timestamp.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let backups: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".env.bak."))
            .collect();
        assert_eq!(
            backups.len(),
            BACKUP_RETENTION,
            "expected {BACKUP_RETENTION} backups, got {}",
            backups.len()
        );
    }

    #[test]
    fn dedupes_within_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        merge(
            &path,
            MergeRequest {
                entries: vec![
                    EnvEntry::new("FOO", "first"),
                    EnvEntry::new("FOO", "second"),
                ],
                ..Default::default()
            },
        )
        .unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("FOO=second"));
        assert_eq!(after.matches("FOO=").count(), 1);
    }

    #[test]
    fn restore_backup_recovers_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_initial(dir.path(), ".env", "FOO=original\n");
        let outcome = merge(
            &path,
            MergeRequest {
                entries: vec![EnvEntry::new("FOO", "modified")],
                force: true,
                ..Default::default()
            },
        )
        .unwrap();
        let backup = outcome.backup_path.expect("backup created");
        restore_backup(&path, &backup).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("FOO=original"));
    }

    #[test]
    fn error_kind_strings_are_stable() {
        // Pin the public stable kind strings used in docs/dev/ERRORS.md.
        let cases: &[(&str, MergeError)] = &[
            (
                "merge_temp_create",
                MergeError::TempCreate {
                    parent: PathBuf::from("/x"),
                    source: std::io::Error::other("e"),
                },
            ),
            (
                "merge_sync_failed",
                MergeError::SyncFailed {
                    source: std::io::Error::other("e"),
                },
            ),
            (
                "merge_persist_cross_fs",
                MergeError::PersistCrossFs {
                    source: std::io::Error::other("e"),
                },
            ),
            (
                "merge_write_conflict",
                MergeError::WriteConflict {
                    path: PathBuf::from("/x"),
                    reason: WriteConflictReason::MtimeSkew,
                },
            ),
            (
                "write_failed",
                MergeError::WriteFailed {
                    path: PathBuf::from("/x"),
                    reason: WriteFailReason::Other("e".into()),
                },
            ),
            (
                "commit_rollback_failed",
                MergeError::CommitRollbackFailed {
                    backup_path: PathBuf::from("/x"),
                    source: std::io::Error::other("e"),
                },
            ),
        ];
        for (expected, err) in cases {
            assert_eq!(err.kind(), *expected);
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 64,
            ..proptest::test_runner::Config::default()
        })]

        /// merge(merge(empty, X), X) is identical to merge(empty, X) — no churn.
        #[test]
        fn idempotent_under_repeat_application(
            entries in proptest::collection::vec(
                (
                    "[A-Z][A-Z0-9_]{0,15}",
                    "[a-zA-Z0-9 _.-]{0,32}",
                ),
                0..8,
            )
        ) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(".env");

            let to_entries = |v: &Vec<(String, String)>| -> Vec<EnvEntry> {
                v.iter().map(|(k, val)| EnvEntry::new(k.clone(), val.clone())).collect()
            };

            merge(&path, MergeRequest {
                entries: to_entries(&entries),
                force: true,
                ..Default::default()
            }).unwrap();

            let snapshot = fs::read_to_string(&path).unwrap_or_default();

            let outcome = merge(&path, MergeRequest {
                entries: to_entries(&entries),
                force: true,
                ..Default::default()
            }).unwrap();

            // Second application must be a no-op.
            assert_eq!(outcome.written, 0);
            assert!(outcome.backup_path.is_none());
            let after = fs::read_to_string(&path).unwrap_or_default();
            assert_eq!(after, snapshot, "file mutated on idempotent reapply");
        }

        /// Comments and blank lines from the original file survive merge.
        #[test]
        fn preserves_comments_property(
            tail in "[a-zA-Z0-9_=.\\- ]{0,40}",
        ) {
            let dir = tempfile::tempdir().unwrap();
            let initial = format!(
                "# top comment\n\nFOO=existing\n# inline {tail}\nBAR=baz\n"
            );
            let path = dir.path().join(".env");
            fs::write(&path, &initial).unwrap();

            merge(&path, MergeRequest {
                entries: vec![EnvEntry::new("NEW_KEY", "value")],
                ..Default::default()
            }).unwrap();

            let after = fs::read_to_string(&path).unwrap();
            assert!(after.contains("# top comment"));
            assert!(after.contains(&format!("# inline {tail}")));
        }
    }

    // Inherently unix-only: asserts 0o600 mode bits. Whole-fn gated rather than
    // wrapping the body in `#[cfg(unix)] { ... }` (which would compile to an
    // empty test that still counts on Windows).
    #[cfg(unix)]
    #[test]
    fn unix_perms_set_to_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        merge(
            &path,
            MergeRequest {
                entries: vec![EnvEntry::new("FOO", "bar")],
                ..Default::default()
            },
        )
        .unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }
}
