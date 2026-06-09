//! Host-brokered artifact writes for Code Mode.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use ulid::Ulid;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{env_non_empty, lab_home, redact_home, reject_path_traversal};
use crate::dispatch::path_safety::reject_existing_symlink_ancestors;

const DEFAULT_CONTENT_TYPE: &str = "text/plain";

/// Upper bound on the `content_type` metadata string.
///
/// This is the one artifact field that *does* reach the model: unlike `content`
/// (written to disk, never returned), `content_type` rides the receipt back into
/// the execution response and the truncation marker. So it gets a context-bound
/// cap; a snippet can't bloat the response with a megabyte `contentType`.
const MAX_CONTENT_TYPE_BYTES: usize = 256;

/// Default per-artifact content cap, in MiB.
///
/// This is NOT a context guard — artifact content is written to disk and only
/// the small receipt is returned to the model. It is a resource bound that keeps
/// a single write comfortably under the runner's 64 MiB JS heap (see
/// `runner.rs`), so an oversized artifact fails as a clean `invalid_param`
/// instead of an opaque QuickJS out-of-memory trap. Override with
/// `LAB_CODE_MODE_ARTIFACT_MAX_MIB` (keep it below ~64 to preserve the clean
/// error boundary).
const DEFAULT_ARTIFACT_MAX_MIB: usize = 8;

/// Default number of per-run artifact directories retained under
/// `$LAB_HOME/code-mode-artifacts/`. Old run directories are pruned on the first
/// artifact write of a run (never on search / no-write runs) so the on-disk
/// store stays bounded. Override with `LAB_CODE_MODE_ARTIFACT_RETENTION_RUNS`;
/// set it to `0` to disable *count* pruning.
const DEFAULT_ARTIFACT_RETENTION_RUNS: usize = 200;

/// Default total-store byte budget, in MiB. Now that a single artifact can be
/// several MiB, the run-count cap alone no longer bounds disk usage, so pruning
/// also drops the oldest inactive run directories until the whole store fits
/// this budget. Override with `LAB_CODE_MODE_ARTIFACT_MAX_STORE_MIB`; set it to
/// `0` to disable *byte* pruning.
const DEFAULT_ARTIFACT_MAX_STORE_MIB: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::dispatch::gateway::code_mode) struct CodeModeArtifactWrite {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub content_type: Option<String>,
}

/// Receipt for one successfully persisted artifact. `bytes`/`sha256`/
/// `content_type` are always derived together from the same content that was
/// written. Fields are module-visible (not `pub`), so no code outside the
/// `code_mode` module can mint a receipt; within the module,
/// [`write_code_mode_artifact`] is by convention the sole producer, which keeps
/// the digest and byte-count honest. serde serializes the fields into the
/// execution response regardless of their visibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeArtifactReceipt {
    pub(in crate::dispatch::gateway::code_mode) path: String,
    pub(in crate::dispatch::gateway::code_mode) absolute_path: String,
    pub(in crate::dispatch::gateway::code_mode) content_type: String,
    pub(in crate::dispatch::gateway::code_mode) bytes: usize,
    pub(in crate::dispatch::gateway::code_mode) sha256: String,
}

fn artifact_store_root() -> PathBuf {
    lab_home().join("code-mode-artifacts")
}

#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn code_mode_artifact_root(run_id: &str) -> PathBuf {
    artifact_store_root().join(run_id)
}

/// Resolve the per-run artifact retention cap from the environment, falling back
/// to [`DEFAULT_ARTIFACT_RETENTION_RUNS`]. `0` disables pruning.
#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn artifact_retention_runs() -> usize {
    // Absent/blank → default silently. Present-but-unparseable → warn and fall
    // back, so a fat-fingered value (e.g. `5O`) isn't silently ignored.
    let Some(raw) = env_non_empty("LAB_CODE_MODE_ARTIFACT_RETENTION_RUNS") else {
        return DEFAULT_ARTIFACT_RETENTION_RUNS;
    };
    match raw.trim().parse::<usize>() {
        Ok(value) => value,
        Err(_) => {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "code_execute",
                value = %raw,
                default = DEFAULT_ARTIFACT_RETENTION_RUNS,
                "ignoring unparseable LAB_CODE_MODE_ARTIFACT_RETENTION_RUNS; using default"
            );
            DEFAULT_ARTIFACT_RETENTION_RUNS
        }
    }
}

/// Resolve the per-artifact content cap (in bytes) from the environment,
/// falling back to [`DEFAULT_ARTIFACT_MAX_MIB`]. The env value is expressed in
/// MiB for ergonomics (`LAB_CODE_MODE_ARTIFACT_MAX_MIB=16`).
#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn artifact_max_bytes() -> usize {
    let default_bytes = DEFAULT_ARTIFACT_MAX_MIB * 1024 * 1024;
    // Absent/blank → default silently. Present-but-unparseable or `0` → warn and
    // fall back (a 0 MiB cap would reject every write).
    let Some(raw) = env_non_empty("LAB_CODE_MODE_ARTIFACT_MAX_MIB") else {
        return default_bytes;
    };
    match raw.trim().parse::<usize>() {
        Ok(mib) if mib > 0 => mib.saturating_mul(1024 * 1024),
        _ => {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "code_execute",
                value = %raw,
                default_mib = DEFAULT_ARTIFACT_MAX_MIB,
                "ignoring invalid LAB_CODE_MODE_ARTIFACT_MAX_MIB; using default"
            );
            default_bytes
        }
    }
}

/// Resolve the total-store byte budget from the environment, falling back to
/// [`DEFAULT_ARTIFACT_MAX_STORE_MIB`]. The env value is in MiB
/// (`LAB_CODE_MODE_ARTIFACT_MAX_STORE_MIB=8192`); `0` disables byte pruning.
#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn artifact_max_store_bytes() -> u64 {
    let default_bytes = DEFAULT_ARTIFACT_MAX_STORE_MIB * 1024 * 1024;
    let Some(raw) = env_non_empty("LAB_CODE_MODE_ARTIFACT_MAX_STORE_MIB") else {
        return default_bytes;
    };
    match raw.trim().parse::<u64>() {
        // `0` is meaningful here (disable byte pruning), unlike the per-artifact
        // cap where 0 is nonsense.
        Ok(mib) => mib.saturating_mul(1024 * 1024),
        Err(_) => {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "code_execute",
                value = %raw,
                default_mib = DEFAULT_ARTIFACT_MAX_STORE_MIB,
                "ignoring unparseable LAB_CODE_MODE_ARTIFACT_MAX_STORE_MIB; using default"
            );
            default_bytes
        }
    }
}

/// Best-effort recursive byte size of a directory. Symlinks are not followed
/// (`file_type()` does not traverse them), so the count can never wander outside
/// the store. Unreadable entries are skipped — this only feeds a retention
/// heuristic, never a correctness decision.
async fn dir_size_bytes(path: PathBuf) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![path];
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            match entry.file_type().await {
                Ok(ft) if ft.is_dir() => stack.push(entry.path()),
                Ok(ft) if ft.is_file() => {
                    if let Ok(meta) = entry.metadata().await {
                        total = total.saturating_add(meta.len());
                    }
                }
                _ => {}
            }
        }
    }
    total
}

/// Process-global set of run ids whose execution is still in flight.
///
/// The artifact store is shared across all concurrent Code Mode executions, and
/// pruning runs on the first write of *any* run. Without this set, a run with a
/// low `retain` could `remove_dir_all` a *different* concurrent run's directory
/// while that run is still writing into it. Membership here makes a run's
/// directory un-prunable for as long as it is executing — see
/// [`ActiveArtifactRun`].
fn active_runs() -> &'static Mutex<HashSet<String>> {
    static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Snapshot the currently-active run ids so a prune pass can exclude them.
pub(in crate::dispatch::gateway::code_mode) fn active_artifact_runs_snapshot() -> HashSet<String> {
    active_runs()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// RAII registration of an in-flight run id. Construct once per execution and
/// hold it for the whole run; `Drop` removes the id so the directory becomes
/// eligible for pruning only after the run has finished.
pub(in crate::dispatch::gateway::code_mode) struct ActiveArtifactRun {
    run_id: String,
}

impl ActiveArtifactRun {
    pub(in crate::dispatch::gateway::code_mode) fn register(run_id: &str) -> Self {
        active_runs()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(run_id.to_string());
        Self {
            run_id: run_id.to_string(),
        }
    }
}

impl Drop for ActiveArtifactRun {
    fn drop(&mut self) {
        active_runs()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.run_id);
    }
}

/// Best-effort prune of old per-run artifact directories so the store stays
/// bounded by both a run-count cap and a total-byte budget. Keeps the newest
/// runs (ULID names sort chronologically) and removes older ones, except any run
/// still executing.
pub(in crate::dispatch::gateway::code_mode) async fn prune_artifact_runs(retain: usize) {
    let active = active_artifact_runs_snapshot();
    prune_artifact_runs_in(
        &artifact_store_root(),
        retain,
        artifact_max_store_bytes(),
        &active,
    )
    .await;
}

/// Core prune over an explicit store root (so tests need no `$LAB_HOME`).
///
/// Removes the oldest run directories that fall outside *either* the run-count
/// cap (`retain`, newest-N) *or* the total-byte budget (`max_store_bytes`,
/// newest-fits-first). `retain == 0` disables the count rule and
/// `max_store_bytes == 0` disables the byte rule; with both off this is a no-op.
///
/// Only directories whose names parse as ULIDs — i.e. run directories this
/// feature created — are ever considered for removal, so an operator's stray
/// file or directory under the store can never be collected. Run ids in
/// `active` are skipped unconditionally (even past either limit) so a concurrent
/// run's directory is never deleted while it is still writing. Errors are
/// swallowed (best-effort, debug-logged); pruning must never fail a run.
pub(in crate::dispatch::gateway::code_mode) async fn prune_artifact_runs_in(
    store_root: &Path,
    retain: usize,
    max_store_bytes: u64,
    active: &HashSet<String>,
) {
    let count_pruning = retain > 0;
    let byte_pruning = max_store_bytes > 0;
    if !count_pruning && !byte_pruning {
        return;
    }
    let mut entries = match tokio::fs::read_dir(store_root).await {
        Ok(entries) => entries,
        // Store not created yet (no artifact has ever been written): nothing to prune.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        // Any other read failure (EACCES, EIO, store replaced by a file, …)
        // disables retention for this run; surface it so unbounded growth is
        // diagnosable rather than silent.
        Err(err) => {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "code_execute",
                error = %err,
                "code-mode artifact retention disabled: cannot read store directory"
            );
            return;
        }
    };
    let mut run_dirs: Vec<String> = Vec::new();
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            // A mid-enumeration failure can leave `run_dirs` short and skip
            // pruning entirely; log it so under-pruning isn't silent.
            Err(err) => {
                tracing::warn!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "code_execute",
                    error = %err,
                    "code-mode artifact retention: store enumeration interrupted; store may be under-pruned"
                );
                break;
            }
        };
        let is_dir = entry
            .file_type()
            .await
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if Ulid::from_string(&name).is_ok() {
            run_dirs.push(name);
        }
    }
    run_dirs.sort(); // ascending: oldest ULID first
    let newest_first: Vec<&String> = run_dirs.iter().rev().collect();

    // When byte-pruning is on, size every run directory concurrently up front —
    // the walks are independent — instead of serializing them inside the
    // decision loop below.
    let sizes: Vec<u64> = if byte_pruning {
        futures::future::join_all(
            newest_first
                .iter()
                .map(|name| dir_size_bytes(store_root.join(name))),
        )
        .await
    } else {
        Vec::new()
    };

    // Walk newest-first, keeping a run while it sits inside BOTH the count window
    // and the running byte budget; everything past either limit is a removal
    // candidate. Active runs still count toward the byte total (they're on disk)
    // but are never themselves removed.
    let mut cumulative: u64 = 0;
    let mut to_remove: Vec<String> = Vec::new();
    for (idx, name) in newest_first.iter().enumerate() {
        if byte_pruning {
            cumulative = cumulative.saturating_add(sizes[idx]);
        }
        let within_count = !count_pruning || idx < retain;
        let within_bytes = !byte_pruning || cumulative <= max_store_bytes;
        if within_count && within_bytes {
            continue;
        }
        // Never collect a run that is still executing — its directory may be
        // mid-write. It becomes eligible on a later prune once it finishes.
        if active.contains(*name) {
            continue;
        }
        to_remove.push((*name).clone());
    }

    for name in to_remove {
        let path = store_root.join(&name);
        if let Err(err) = tokio::fs::remove_dir_all(&path).await {
            tracing::debug!(
                surface = "dispatch",
                service = "code_mode",
                action = "code_execute",
                error = %err,
                "failed to prune old code-mode artifact directory"
            );
        }
    }
}

pub(in crate::dispatch::gateway::code_mode) async fn write_code_mode_artifact(
    root: &Path,
    request: &CodeModeArtifactWrite,
    max_bytes: usize,
) -> Result<CodeModeArtifactReceipt, ToolError> {
    let rel_path = normalize_artifact_path(&request.path)?;
    reject_oversized_content_type(request.content_type.as_deref())?;
    let bytes = request.content.as_bytes();
    if bytes.len() > max_bytes {
        return Err(ToolError::InvalidParam {
            message: format!(
                "artifact content is {} bytes; maximum is {max_bytes} bytes",
                bytes.len(),
            ),
            param: "content".to_string(),
        });
    }

    let destination = root.join(&rel_path);
    // Defense-in-depth per `reject_path_traversal`'s documented contract: the
    // lexical guard in `normalize_artifact_path` cannot see through symlinks, so
    // confirm the joined destination stays within `root` and that no existing
    // symlinked ancestor redirects the write outside the jail before any
    // directory or file is created.
    reject_existing_symlink_ancestors(root, &destination)?;

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to create artifact directory: {err}"),
            })?;
    }

    let mut file = tokio::fs::File::create(&destination)
        .await
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create artifact file: {err}"),
        })?;
    file.write_all(bytes).await.map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to write artifact file: {err}"),
    })?;
    file.flush().await.map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to flush artifact file: {err}"),
    })?;

    let sha256 = Sha256::digest(bytes);
    let content_type = request
        .content_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_CONTENT_TYPE)
        .to_string();

    Ok(CodeModeArtifactReceipt {
        path: rel_path,
        absolute_path: redact_home(&destination.display().to_string()),
        content_type,
        bytes: bytes.len(),
        sha256: hex::encode(sha256),
    })
}

/// Reject a `content_type` that would bloat the response. The receipt (and the
/// truncation marker) carry this string into the model's context, so unlike the
/// on-disk content it needs a small, fixed cap.
fn reject_oversized_content_type(content_type: Option<&str>) -> Result<(), ToolError> {
    if let Some(value) = content_type
        && value.len() > MAX_CONTENT_TYPE_BYTES
    {
        return Err(ToolError::InvalidParam {
            message: format!(
                "artifact content_type is {} bytes; maximum is {MAX_CONTENT_TYPE_BYTES} bytes",
                value.len(),
            ),
            param: "content_type".to_string(),
        });
    }
    Ok(())
}

fn normalize_artifact_path(raw: &str) -> Result<String, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "artifact path must be a non-empty relative path".to_string(),
            param: "path".to_string(),
        });
    }
    // Normalize Windows-style separators to `/` BEFORE the lexical guards below.
    // On Unix a backslash is an ordinary filename byte, so `a\..\..\etc\evil`
    // would pass `is_absolute`/`reject_path_traversal` as a single innocent
    // component and only afterwards (when the receipt path is built) turn into
    // real `../` separators that escape the jail. Converting first makes the
    // guards see exactly the separators the filesystem will.
    let normalized = trimmed.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(ToolError::InvalidParam {
            message: "artifact path must be a relative path".to_string(),
            param: "path".to_string(),
        });
    }
    reject_path_traversal(&normalized)?;
    Ok(normalized)
}
