//! Host-brokered artifact writes for Code Mode.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use ulid::Ulid;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{env_non_empty, lab_home, redact_home, reject_path_traversal};
use crate::dispatch::path_safety::reject_existing_symlink_ancestors;

const DEFAULT_CONTENT_TYPE: &str = "text/plain";
const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;

/// Default number of per-run artifact directories retained under
/// `$LAB_HOME/code-mode-artifacts/`. Old run directories are pruned on the first
/// artifact write of a run (never on search / no-write runs) so the on-disk
/// store stays bounded. Override with `LAB_CODE_MODE_ARTIFACT_RETENTION_RUNS`;
/// set it to `0` to disable pruning (unbounded growth).
const DEFAULT_ARTIFACT_RETENTION_RUNS: usize = 200;

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

/// Best-effort prune of old per-run artifact directories so the store stays
/// bounded. Keeps the newest `retain` run directories (ULID names sort
/// chronologically) and removes the rest.
pub(in crate::dispatch::gateway::code_mode) async fn prune_artifact_runs(retain: usize) {
    prune_artifact_runs_in(&artifact_store_root(), retain).await;
}

/// Core prune over an explicit store root (so tests need no `$LAB_HOME`).
///
/// Only directories whose names parse as ULIDs — i.e. run directories this
/// feature created — are ever considered for removal, so an operator's stray
/// file or directory under the store can never be collected. Errors are
/// swallowed (best-effort, debug-logged); pruning must never fail a run.
pub(in crate::dispatch::gateway::code_mode) async fn prune_artifact_runs_in(
    store_root: &Path,
    retain: usize,
) {
    if retain == 0 {
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
    if run_dirs.len() <= retain {
        return;
    }
    run_dirs.sort(); // ascending: oldest ULID first
    let remove_count = run_dirs.len() - retain;
    for name in run_dirs.into_iter().take(remove_count) {
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
) -> Result<CodeModeArtifactReceipt, ToolError> {
    let rel_path = normalize_artifact_path(&request.path)?;
    let bytes = request.content.as_bytes();
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(ToolError::InvalidParam {
            message: format!(
                "artifact content is {} bytes; maximum is {} bytes",
                bytes.len(),
                MAX_ARTIFACT_BYTES
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
