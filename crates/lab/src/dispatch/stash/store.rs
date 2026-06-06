//! Canonical local stash store — path layout and JSON record I/O.
//!
//! All file operations are **synchronous** by design. Async callers must wrap
//! calls in `tokio::task::spawn_blocking` (or equivalent).
//!
//! # Layout under `stash_root/`
//!
//! ```text
//! components/   — JSON records + advisory locks per component
//!                 <id>.json, <id>.lock, <id>.deploy.lock
//! revisions/    — immutable revision snapshots
//!                 <rev_id>/meta.json, <rev_id>/files/
//!                 by-component/<component_id>.json  (revision ID index)
//! workspaces/   — live working copies per component
//!                 <id>/              (directory-shaped)
//!                 <id>/<filename>    (file-shaped)
//! providers/    — provider link records  (<id>.json)
//!                 by-component/<component_id>.json  (provider ID index)
//! targets/      — deploy target records  (<id>.json)
//! ```
//!
//! There is **no** `objects/` directory.

#![allow(dead_code)]

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use tempfile::NamedTempFile;

use lab_apis::stash::types::{
    StashComponent, StashDeployTarget, StashProviderRecord, StashRevision, StashWorkspaceShape,
};

use crate::dispatch::error::ToolError;

// ── Constants ────────────────────────────────────────────────────────────────

const DIR_COMPONENTS: &str = "components";
const DIR_REVISIONS: &str = "revisions";
const DIR_WORKSPACES: &str = "workspaces";
const DIR_PROVIDERS: &str = "providers";
const DIR_TARGETS: &str = "targets";

/// Secondary index sub-directory under `revisions/` for per-component revision lists.
/// lab-qz6a.24: enables O(1) `list_revisions_for` instead of O(R) full scan.
const DIR_REVISIONS_BY_COMPONENT: &str = "revisions/by-component";

/// Secondary index sub-directory under `providers/` for per-component provider lists.
/// lab-qz6a.25: enables O(1) `list_providers_for` instead of O(P) full scan.
const DIR_PROVIDERS_BY_COMPONENT: &str = "providers/by-component";

const EXT_RECORD: &str = ".json";
const EXT_LOCK: &str = ".lock";
const EXT_DEPLOY_LOCK: &str = ".deploy.lock";
const FILE_META: &str = "meta.json";
const DIR_FILES: &str = "files";

/// Maximum `id` length in bytes.
const MAX_ID_LEN: usize = 64;

/// Poll interval for the deploy-lock timeout loop.
const DEPLOY_LOCK_POLL_MS: u64 = 50;

// ── StashStore ───────────────────────────────────────────────────────────────

/// Encapsulates all I/O against a local stash root directory.
///
/// The struct itself is cheap to clone (a single `PathBuf`). All methods are
/// **synchronous** and safe to call from `spawn_blocking`.
#[derive(Debug, Clone)]
pub struct StashStore {
    root: PathBuf,
}

impl StashStore {
    /// Create a `StashStore` backed by `root`.
    ///
    /// The directory does not need to exist yet — call [`Self::ensure_dirs`]
    /// before performing any I/O.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Return the root path of this store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    // ── Path helpers: components ─────────────────────────────────────────────

    /// Path to `components/<id>.json`.
    pub fn component_record_path(&self, id: &str) -> PathBuf {
        self.root
            .join(DIR_COMPONENTS)
            .join(format!("{id}{EXT_RECORD}"))
    }

    /// Path to `components/<id>.lock`.
    pub fn component_lock_path(&self, id: &str) -> PathBuf {
        self.root
            .join(DIR_COMPONENTS)
            .join(format!("{id}{EXT_LOCK}"))
    }

    /// Path to `components/<id>.deploy.lock`.
    pub fn component_deploy_lock_path(&self, id: &str) -> PathBuf {
        self.root
            .join(DIR_COMPONENTS)
            .join(format!("{id}{EXT_DEPLOY_LOCK}"))
    }

    // ── Path helpers: workspaces ─────────────────────────────────────────────

    /// Path to `workspaces/<id>/`.
    pub fn workspace_dir(&self, id: &str) -> PathBuf {
        self.root.join(DIR_WORKSPACES).join(id)
    }

    /// Resolve the workspace path for a component.
    ///
    /// - **File-shaped**: `workspaces/<id>/<filename>` (requires `filename`).
    /// - **Directory-shaped**: `workspaces/<id>/`.
    pub fn workspace_path(
        &self,
        id: &str,
        shape: StashWorkspaceShape,
        filename: Option<&str>,
    ) -> PathBuf {
        let base = self.workspace_dir(id);
        match shape {
            StashWorkspaceShape::File => {
                let name = filename.unwrap_or("file");
                base.join(name)
            }
            StashWorkspaceShape::Directory => base,
        }
    }

    // ── Path helpers: revisions ──────────────────────────────────────────────

    /// Path to `revisions/<rev_id>/`.
    pub fn revision_dir(&self, rev_id: &str) -> PathBuf {
        self.root.join(DIR_REVISIONS).join(rev_id)
    }

    /// Path to `revisions/<rev_id>/files/`.
    pub fn revision_files_path(&self, rev_id: &str) -> PathBuf {
        self.revision_dir(rev_id).join(DIR_FILES)
    }

    /// Path to `revisions/<rev_id>/meta.json`.
    pub fn revision_meta_path(&self, rev_id: &str) -> PathBuf {
        self.revision_dir(rev_id).join(FILE_META)
    }

    // ── Path helpers: secondary indexes ─────────────────────────────────────

    /// Path to `revisions/by-component/<component_id>.json`.
    ///
    /// This is a JSON array of revision IDs belonging to the component.
    /// lab-qz6a.24: provides O(1) lookup instead of O(R) full scan.
    pub fn component_revision_index_path(&self, component_id: &str) -> PathBuf {
        self.root
            .join(DIR_REVISIONS_BY_COMPONENT)
            .join(format!("{component_id}{EXT_RECORD}"))
    }

    /// Path to `providers/by-component/<component_id>.json`.
    ///
    /// This is a JSON array of provider IDs belonging to the component.
    /// lab-qz6a.25: provides O(1) lookup instead of O(P) full scan.
    pub fn component_provider_index_path(&self, component_id: &str) -> PathBuf {
        self.root
            .join(DIR_PROVIDERS_BY_COMPONENT)
            .join(format!("{component_id}{EXT_RECORD}"))
    }

    // ── Path helpers: providers ──────────────────────────────────────────────

    /// Path to `providers/<id>.json`.
    pub fn provider_record_path(&self, id: &str) -> PathBuf {
        self.root
            .join(DIR_PROVIDERS)
            .join(format!("{id}{EXT_RECORD}"))
    }

    // ── Path helpers: targets ────────────────────────────────────────────────

    /// Path to `targets/<id>.json`.
    pub fn target_record_path(&self, id: &str) -> PathBuf {
        self.root
            .join(DIR_TARGETS)
            .join(format!("{id}{EXT_RECORD}"))
    }

    // ── Initialization ───────────────────────────────────────────────────────

    /// Create the top-level sub-directories and secondary index directories
    /// if they do not yet exist.
    ///
    /// This is idempotent and should be called once at startup.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for sub in [
            DIR_COMPONENTS,
            DIR_REVISIONS,
            DIR_WORKSPACES,
            DIR_PROVIDERS,
            DIR_TARGETS,
            // Secondary indexes (lab-qz6a.24, lab-qz6a.25)
            DIR_REVISIONS_BY_COMPONENT,
            DIR_PROVIDERS_BY_COMPONENT,
        ] {
            std::fs::create_dir_all(self.root.join(sub))?;
        }
        Ok(())
    }

    // ── Validation ───────────────────────────────────────────────────────────

    /// Validate that `id` is a safe, filesystem-legal identifier.
    ///
    /// Rules: non-empty, `<= 64` bytes, alphanumeric or hyphens only.
    pub fn validate_id(id: &str) -> Result<(), ToolError> {
        if id.is_empty() {
            return Err(invalid_param("id", "must not be empty"));
        }
        if id.len() > MAX_ID_LEN {
            return Err(invalid_param(
                "id",
                &format!("must not exceed {MAX_ID_LEN} characters"),
            ));
        }
        if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(invalid_param(
                "id",
                "must contain only alphanumeric characters and hyphens",
            ));
        }
        Ok(())
    }

    // ── Component record I/O ─────────────────────────────────────────────────

    /// Read and deserialize the component record for `id`, or `None` if absent.
    pub fn read_component(&self, id: &str) -> Result<Option<StashComponent>, ToolError> {
        Self::validate_id(id)?;
        let path = self.component_record_path(id);
        read_json_optional(&path)
    }

    /// Atomically write a component record to disk.
    ///
    /// The write is not lock-protected on its own; callers that require
    /// exclusive access should wrap the call in [`Self::with_component_lock`].
    pub fn write_component(&self, component: &StashComponent) -> Result<(), ToolError> {
        Self::validate_id(&component.id)?;
        let path = self.component_record_path(&component.id);
        write_json_atomic(&path, component)
    }

    /// List all component records in the store.
    ///
    /// Performs a full scan of `components/` and filters to `.json` files only.
    /// Malformed records are skipped with an internal-error result propagated to
    /// the caller on the first decode failure.
    pub fn list_components(&self) -> Result<Vec<StashComponent>, ToolError> {
        let dir = self.root.join(DIR_COMPONENTS);
        list_json_records(&dir)
    }

    /// Remove the component JSON record for `id`.
    ///
    /// Returns `Ok(())` if the file does not exist.
    pub fn delete_component_record(&self, id: &str) -> Result<(), ToolError> {
        Self::validate_id(id)?;
        let path = self.component_record_path(id);
        remove_if_exists(&path)
    }

    /// Fully delete a component and all its associated data from the store.
    ///
    /// Removes, in order:
    /// 1. All revision directories and files for the component.
    /// 2. The per-component revision index (`revisions/by-component/<id>.json`).
    /// 3. All provider records belonging to the component.
    /// 4. The per-component provider index (`providers/by-component/<id>.json`).
    /// 5. The workspace directory (`workspaces/<id>/`).
    /// 6. The component JSON record (`components/<id>.json`).
    /// 7. The advisory lock file (`components/<id>.lock`).
    /// 8. The deploy lock file (`components/<id>.deploy.lock`).
    ///
    /// All steps are best-effort after validation: a missing file or directory at
    /// any step is treated as already gone, not an error. The operation runs inside
    /// `with_component_lock` to serialise concurrent callers.
    ///
    /// **Callers must NOT hold the component lock before calling this method.**
    pub fn delete_component(&self, id: &str) -> Result<(), ToolError> {
        Self::validate_id(id)?;

        self.with_component_lock(id, || {
            // 1. Remove all revision dirs belonging to this component.
            let rev_ids = self.revision_ids_for_component(id)?;
            for rev_id in &rev_ids {
                let rev_dir = self.revision_dir(rev_id);
                remove_dir_all_if_exists(&rev_dir)?;
            }

            // 2. Remove the revision index for this component.
            let rev_index = self.component_revision_index_path(id);
            remove_if_exists(&rev_index)?;

            // 3. Remove all provider records belonging to this component.
            let providers = self.list_providers_for(id)?;
            for prov in &providers {
                let prov_path = self.provider_record_path(&prov.id);
                remove_if_exists(&prov_path)?;
            }

            // 4. Remove the provider index for this component.
            let prov_index = self.component_provider_index_path(id);
            remove_if_exists(&prov_index)?;

            // 5. Remove the workspace directory.
            let workspace = self.workspace_dir(id);
            remove_dir_all_if_exists(&workspace)?;

            // 6. Remove the component JSON record.
            let comp_path = self.component_record_path(id);
            remove_if_exists(&comp_path)?;

            Ok(())
        })?;

        // 7 & 8. Remove lock files AFTER releasing the lock (guard dropped above).
        //        These are outside `with_component_lock` so we are not holding an
        //        fd_lock guard on the file we are about to delete.
        let lock_path = self.component_lock_path(id);
        remove_if_exists(&lock_path)?;
        let deploy_lock_path = self.component_deploy_lock_path(id);
        remove_if_exists(&deploy_lock_path)?;

        Ok(())
    }

    /// Return the list of revision IDs that belong to `component_id`.
    ///
    /// Uses the per-component index when available; falls back to a full
    /// O(R) scan over `revisions/` when the index is absent or corrupt.
    fn revision_ids_for_component(&self, component_id: &str) -> Result<Vec<String>, ToolError> {
        let index_path = self.component_revision_index_path(component_id);
        if index_path.exists() {
            let bytes = std::fs::read(&index_path).map_err(io_internal)?;
            if let Ok(ids) = serde_json::from_slice::<Vec<String>>(&bytes) {
                return Ok(ids);
            }
            // Corrupt index — fall through to full scan.
        }

        // Full scan fallback.
        let revisions_dir = self.root.join(DIR_REVISIONS);
        if !revisions_dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&revisions_dir).map_err(io_internal)? {
            let entry = entry.map_err(io_internal)?;
            if entry.file_name() == "by-component" {
                continue;
            }
            let meta_path = entry.path().join(FILE_META);
            let bytes = match std::fs::read(&meta_path) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_internal(e)),
            };
            let rev: StashRevision = serde_json::from_slice(&bytes).map_err(decode_error)?;
            if rev.component_id == component_id {
                ids.push(rev.id);
            }
        }
        Ok(ids)
    }

    // ── Revision meta I/O ────────────────────────────────────────────────────

    /// Read and deserialize a revision's `meta.json`, or `None` if absent.
    pub fn read_revision_meta(&self, rev_id: &str) -> Result<Option<StashRevision>, ToolError> {
        Self::validate_id(rev_id)?;
        let path = self.revision_meta_path(rev_id);
        read_json_optional(&path)
    }

    /// Atomically write a revision's `meta.json` and append the revision ID to
    /// the per-component index at `revisions/by-component/<component_id>.json`.
    ///
    /// Write order: meta first, then index.  A crash between the two leaves
    /// meta-without-index. Recovery depends on the component's prior state:
    /// - If no index yet exists, `list_revisions_for` falls back to the O(R) scan
    ///   and finds the orphaned meta. Full recovery.
    /// - If an index already exists, the new revision's ID is simply absent from
    ///   the index. The revision is invisible until the index is manually repaired
    ///   or the index is removed to trigger a full scan.
    /// Reverse order (index before meta) would leave a dangling index entry
    /// that points to non-existent meta.
    ///
    /// Callers are expected to hold the component advisory lock before calling
    /// this method; do NOT take the lock here (would deadlock via fd_lock re-entrancy).
    ///
    /// lab-qz6a.24: index append makes `list_revisions_for` O(1) instead of O(R).
    pub fn write_revision_meta(&self, rev: &StashRevision) -> Result<(), ToolError> {
        Self::validate_id(&rev.id)?;
        let path = self.revision_meta_path(&rev.id);
        write_json_atomic(&path, rev)?;
        self.append_revision_to_index(&rev.component_id, &rev.id)
    }

    /// Append `rev_id` to the per-component revision index.
    ///
    /// Reads the existing index (or starts with an empty vec), appends `rev_id`,
    /// and writes atomically.  Duplicate IDs are not checked — the caller
    /// (`write_revision_meta`) guarantees uniqueness.
    ///
    /// Returns `decode_error` when the index exists but is corrupt — never
    /// silently overwrites a non-empty index with a single-entry vec.
    pub fn append_revision_to_index(
        &self,
        component_id: &str,
        rev_id: &str,
    ) -> Result<(), ToolError> {
        Self::validate_id(component_id)?;
        Self::validate_id(rev_id)?;
        let index_path = self.component_revision_index_path(component_id);
        let mut ids: Vec<String> = match std::fs::read(&index_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                ToolError::Sdk {
                    sdk_kind: "decode_error".into(),
                    message: format!(
                        "revision index for `{component_id}` is corrupt and cannot be appended to: {e}; \
                         inspect `{}` and repair or remove it",
                        index_path.display()
                    ),
                }
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io_internal(e)),
        };
        ids.push(rev_id.to_string());
        write_json_atomic(&index_path, &ids)
    }

    /// List all revisions that belong to a given component.
    ///
    /// lab-qz6a.24: uses the per-component index at
    /// `revisions/by-component/<component_id>.json` when it exists, falling
    /// back to a full O(R) scan of `revisions/*/meta.json` for backwards
    /// compatibility with stores written before the index was introduced.
    pub fn list_revisions_for(&self, component_id: &str) -> Result<Vec<StashRevision>, ToolError> {
        Self::validate_id(component_id)?;

        let index_path = self.component_revision_index_path(component_id);
        if index_path.exists() {
            // Fast path: index present — load only the revisions listed in it.
            // If the index is corrupt, fall through to the O(R) scan rather than
            // returning empty results; a corrupt-but-present index must not hide
            // revisions whose meta.json files are intact on disk.
            let bytes = std::fs::read(&index_path).map_err(io_internal)?;
            match serde_json::from_slice::<Vec<String>>(&bytes) {
                Ok(ids) => {
                    let mut out = Vec::with_capacity(ids.len());
                    for rev_id in &ids {
                        let meta_path = self.revision_meta_path(rev_id);
                        let rev_bytes = match std::fs::read(&meta_path) {
                            Ok(b) => b,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(e) => return Err(io_internal(e)),
                        };
                        let rev: StashRevision =
                            serde_json::from_slice(&rev_bytes).map_err(decode_error)?;
                        out.push(rev);
                    }
                    return Ok(out);
                }
                Err(_) => {
                    // Index is corrupt — fall through to the full scan below.
                    tracing::warn!(
                        component_id,
                        index = %index_path.display(),
                        "revision index is corrupt; falling back to full scan"
                    );
                }
            }
        }

        // Fallback: O(R) full scan — used for stores pre-dating the index or
        // when the index is corrupt (see above).
        let revisions_dir = self.root.join(DIR_REVISIONS);
        if !revisions_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&revisions_dir).map_err(io_internal)? {
            let entry = entry.map_err(io_internal)?;
            // Skip the by-component sub-directory itself.
            if entry.file_name() == "by-component" {
                continue;
            }
            let meta_path = entry.path().join(FILE_META);
            if !meta_path.is_file() {
                continue;
            }
            let bytes = match std::fs::read(&meta_path) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_internal(e)),
            };
            let rev: StashRevision = serde_json::from_slice(&bytes).map_err(decode_error)?;
            if rev.component_id == component_id {
                out.push(rev);
            }
        }
        Ok(out)
    }

    // ── Target record I/O ────────────────────────────────────────────────────

    /// Read and deserialize a deploy target record, or `None` if absent.
    pub fn read_target(&self, id: &str) -> Result<Option<StashDeployTarget>, ToolError> {
        Self::validate_id(id)?;
        let path = self.target_record_path(id);
        read_json_optional(&path)
    }

    /// Atomically write a deploy target record.
    ///
    /// The `id` parameter controls the filename (`targets/<id>.json`);
    /// the inner `id` field of `target` is NOT assumed to match.
    pub fn write_target(&self, id: &str, target: &StashDeployTarget) -> Result<(), ToolError> {
        Self::validate_id(id)?;
        let path = self.target_record_path(id);
        write_json_atomic(&path, target)
    }

    /// List all deploy targets in the store.
    ///
    /// Returns `(filename_id, target)` pairs. The `filename_id` is derived from
    /// the `.json` filename and may differ from the inner `id` field of the
    /// target enum variant.
    pub fn list_targets(&self) -> Result<Vec<(String, StashDeployTarget)>, ToolError> {
        let dir = self.root.join(DIR_TARGETS);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(io_internal)? {
            let entry = entry.map_err(io_internal)?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(id) = name.strip_suffix(EXT_RECORD) else {
                continue;
            };
            // Skip lock files that sneak in.
            if id.ends_with(".lock") || id.ends_with(".deploy") {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_internal(e)),
            };
            let target: StashDeployTarget = serde_json::from_slice(&bytes).map_err(decode_error)?;
            out.push((id.to_string(), target));
        }
        Ok(out)
    }

    /// Remove the deploy target record for `id`.
    ///
    /// Returns `Ok(())` if absent.
    pub fn delete_target(&self, id: &str) -> Result<(), ToolError> {
        Self::validate_id(id)?;
        let path = self.target_record_path(id);
        remove_if_exists(&path)
    }

    // ── Provider record I/O ──────────────────────────────────────────────────

    /// Read and deserialize a provider record, or `None` if absent.
    pub fn read_provider(&self, id: &str) -> Result<Option<StashProviderRecord>, ToolError> {
        Self::validate_id(id)?;
        let path = self.provider_record_path(id);
        read_json_optional(&path)
    }

    /// Atomically write a provider record and append the provider ID to the
    /// per-component index at `providers/by-component/<component_id>.json`.
    ///
    /// Write order: provider record first, then index (recoverable on crash).
    /// Callers hold the component advisory lock.
    ///
    /// lab-qz6a.25: index append makes `list_providers_for` O(1) instead of O(P).
    pub fn write_provider(&self, provider: &StashProviderRecord) -> Result<(), ToolError> {
        Self::validate_id(&provider.id)?;
        let path = self.provider_record_path(&provider.id);
        write_json_atomic(&path, provider)?;
        self.append_provider_to_index(&provider.component_id, &provider.id)
    }

    /// Append `provider_id` to the per-component provider index.
    ///
    /// Reads the existing index (or starts with an empty vec), appends
    /// `provider_id`, and writes atomically.
    ///
    /// Returns `decode_error` when the index exists but is corrupt — never
    /// silently overwrites a non-empty index with a single-entry vec.
    pub fn append_provider_to_index(
        &self,
        component_id: &str,
        provider_id: &str,
    ) -> Result<(), ToolError> {
        Self::validate_id(component_id)?;
        Self::validate_id(provider_id)?;
        let index_path = self.component_provider_index_path(component_id);
        let mut ids: Vec<String> = match std::fs::read(&index_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                ToolError::Sdk {
                    sdk_kind: "decode_error".into(),
                    message: format!(
                        "provider index for `{component_id}` is corrupt and cannot be appended to: {e}; \
                         inspect `{}` and repair or remove it",
                        index_path.display()
                    ),
                }
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io_internal(e)),
        };
        ids.push(provider_id.to_string());
        write_json_atomic(&index_path, &ids)
    }

    /// List all provider records that belong to a given component.
    ///
    /// lab-qz6a.25: uses the per-component index at
    /// `providers/by-component/<component_id>.json` when it exists, falling
    /// back to a full O(P) scan of `providers/` for backwards compatibility.
    pub fn list_providers_for(
        &self,
        component_id: &str,
    ) -> Result<Vec<StashProviderRecord>, ToolError> {
        Self::validate_id(component_id)?;

        let index_path = self.component_provider_index_path(component_id);
        if index_path.exists() {
            // Fast path: index present — load only the providers listed in it.
            // If the index is corrupt, fall through to the O(P) scan rather than
            // returning empty results.
            let bytes = std::fs::read(&index_path).map_err(io_internal)?;
            match serde_json::from_slice::<Vec<String>>(&bytes) {
                Ok(ids) => {
                    let mut out = Vec::with_capacity(ids.len());
                    for prov_id in &ids {
                        let prov_path = self.provider_record_path(prov_id);
                        let prov_bytes = match std::fs::read(&prov_path) {
                            Ok(b) => b,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(e) => return Err(io_internal(e)),
                        };
                        let record: StashProviderRecord =
                            serde_json::from_slice(&prov_bytes).map_err(decode_error)?;
                        out.push(record);
                    }
                    return Ok(out);
                }
                Err(_) => {
                    tracing::warn!(
                        component_id,
                        index = %index_path.display(),
                        "provider index is corrupt; falling back to full scan"
                    );
                }
            }
        }

        // Fallback: O(P) full scan for stores pre-dating the index or when it is corrupt.
        let dir = self.root.join(DIR_PROVIDERS);
        let all: Vec<StashProviderRecord> = list_json_records(&dir)?;
        Ok(all
            .into_iter()
            .filter(|p| p.component_id == component_id)
            .collect())
    }

    /// List all provider records in the store (no component filter).
    ///
    /// Used by `service.rs::providers_list` when no `component_id` filter is given.
    /// lab-qz6a.25: replaces the duplicate `list_json_records_from_dir` helper in service.rs.
    pub fn list_all_providers(&self) -> Result<Vec<StashProviderRecord>, ToolError> {
        let dir = self.root.join(DIR_PROVIDERS);
        list_json_records(&dir)
    }

    /// Remove the provider record for `id`.
    ///
    /// Returns `Ok(())` if absent.
    pub fn delete_provider(&self, id: &str) -> Result<(), ToolError> {
        Self::validate_id(id)?;
        let path = self.provider_record_path(id);
        remove_if_exists(&path)
    }

    // ── Advisory locks ───────────────────────────────────────────────────────

    /// Acquire an exclusive advisory lock on `components/<id>.lock` and execute
    /// `f` while the lock is held.
    ///
    /// The lock file is created if it does not yet exist. The lock is released
    /// when the returned guard is dropped (at end of scope).
    ///
    /// This is a **blocking** call — it will wait indefinitely for the lock.
    pub fn with_component_lock<F, T>(&self, id: &str, f: F) -> Result<T, ToolError>
    where
        F: FnOnce() -> Result<T, ToolError>,
    {
        Self::validate_id(id)?;
        let lock_path = self.component_lock_path(id);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(io_internal)?;
        }
        let file = open_lock_file(&lock_path)?;
        let mut rw_lock = fd_lock::RwLock::new(file);
        // Blocking exclusive write — waits until the lock is available.
        let _guard = rw_lock.write().map_err(io_internal)?;
        f()
    }

    /// Acquire an exclusive advisory deploy lock on `components/<id>.deploy.lock`
    /// and execute `f` while the lock is held.
    ///
    /// Polls `try_write()` in a loop with `DEPLOY_LOCK_POLL_MS` sleep intervals.
    /// Returns a `Sdk { sdk_kind: "conflict" }` error if `timeout_ms` elapses
    /// before the lock is acquired.
    pub fn with_deploy_lock<F, T>(&self, id: &str, timeout_ms: u64, f: F) -> Result<T, ToolError>
    where
        F: FnOnce() -> Result<T, ToolError>,
    {
        Self::validate_id(id)?;
        let lock_path = self.component_deploy_lock_path(id);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(io_internal)?;
        }
        let file = open_lock_file(&lock_path)?;
        let mut rw_lock = fd_lock::RwLock::new(file);
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            match rw_lock.try_write() {
                Ok(guard) => {
                    // Keep `guard` alive across the entire duration of `f()`.
                    // Binding to a named variable (not `_guard`) ensures the
                    // fd-lock write guard is not dropped before `f()` returns.
                    let result = f();
                    drop(guard);
                    return result;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(ToolError::Sdk {
                            sdk_kind: "conflict".into(),
                            message: format!(
                                "deploy lock timed out after {timeout_ms}ms for component {id}"
                            ),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(DEPLOY_LOCK_POLL_MS));
                }
                Err(e) => return Err(io_internal(e)),
            }
        }
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Open (or create) a lock file with read+write access, without truncating.
fn open_lock_file(path: &Path) -> Result<File, ToolError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(io_internal)
}

/// Atomically write `value` as pretty-printed JSON to `path`.
///
/// Uses a temp file in the same directory and `persist` (atomic rename) so
/// readers never see a partially-written file.
fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), ToolError> {
    let Some(parent) = path.parent() else {
        return Err(ToolError::internal_message("target path has no parent"));
    };
    std::fs::create_dir_all(parent).map_err(io_internal)?;
    let mut temp = NamedTempFile::new_in(parent).map_err(io_internal)?;
    let bytes = serde_json::to_vec_pretty(value).map_err(io_internal)?;
    temp.write_all(&bytes).map_err(io_internal)?;
    temp.as_file().sync_all().map_err(io_internal)?;
    temp.persist(path).map_err(|e| io_internal(e.error))?;
    Ok(())
}

/// Read and deserialize a JSON file, returning `None` if the file does not
/// exist.
fn read_json_optional<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, ToolError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_internal(e)),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(decode_error)
}

/// Scan a directory for `*.json` files and deserialize each one.
///
/// Files that are missing by the time we read them (TOCTOU) are skipped.
/// Lock files (`*.lock.json`) are excluded by the `.json`-only extension filter
/// combined with the suffix check — lock files use `.lock`, not `.json`.
fn list_json_records<T: serde::de::DeserializeOwned>(dir: &Path) -> Result<Vec<T>, ToolError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(io_internal)? {
        let entry = entry.map_err(io_internal)?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Only process plain `<id>.json` files; skip `.lock`, `.deploy.lock`.
        if !name.ends_with(EXT_RECORD) {
            continue;
        }
        // Exclude anything whose stem still ends with ".lock" or ".deploy".
        let stem = &name[..name.len() - EXT_RECORD.len()];
        if stem.ends_with(".lock") || stem.ends_with(".deploy") {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_internal(e)),
        };
        let record: T = serde_json::from_slice(&bytes).map_err(decode_error)?;
        out.push(record);
    }
    Ok(out)
}

/// Remove a file, treating `NotFound` as success.
fn remove_if_exists(path: &Path) -> Result<(), ToolError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_internal(e)),
    }
}

/// Recursively remove a directory tree, treating `NotFound` as success.
fn remove_dir_all_if_exists(path: &Path) -> Result<(), ToolError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_internal(e)),
    }
}

fn io_internal(error: impl std::fmt::Display) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: error.to_string(),
    }
}

fn decode_error(error: serde_json::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "decode_error".into(),
        message: format!("decode stash record JSON: {error}"),
    }
}

fn invalid_param(param: &str, message: &str) -> ToolError {
    ToolError::InvalidParam {
        param: param.into(),
        message: message.into(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use lab_apis::stash::types::{StashComponentKind, StashWorkspaceShape};
    use tempfile::tempdir;

    fn make_store() -> (StashStore, tempfile::TempDir) {
        let dir = tempdir().expect("tempdir");
        let store = StashStore::new(dir.path().to_path_buf());
        store.ensure_dirs().expect("ensure_dirs");
        (store, dir)
    }

    fn sample_component(id: &str) -> StashComponent {
        StashComponent {
            id: id.to_string(),
            kind: StashComponentKind::Skill,
            name: "my-skill".to_string(),
            label: Some("demo skill".to_string()),
            head_revision_id: None,
            origin: None,
            workspace_root: PathBuf::from("/tmp/skill"),
            workspace_shape: StashWorkspaceShape::Directory,
            unix_mode: None,
            created_at: "2026-04-26T12:00:00Z".to_string(),
            updated_at: "2026-04-26T12:00:00Z".to_string(),
        }
    }

    fn sample_revision(rev_id: &str, component_id: &str) -> StashRevision {
        StashRevision {
            id: rev_id.to_string(),
            component_id: component_id.to_string(),
            label: Some("v1".to_string()),
            content_digest: "abc123".to_string(),
            created_at: "2026-04-26T12:00:00Z".to_string(),
            file_count: 3,
            unix_mode: None,
        }
    }

    fn sample_provider(id: &str, component_id: &str) -> StashProviderRecord {
        StashProviderRecord {
            id: id.to_string(),
            component_id: component_id.to_string(),
            kind: "filesystem".to_string(),
            label: "local".to_string(),
            config: serde_json::json!({}),
        }
    }

    // ── validate_id ──────────────────────────────────────────────────────────

    #[test]
    fn validate_id_accepts_valid_ids() {
        assert!(StashStore::validate_id("abc123").is_ok());
        assert!(StashStore::validate_id("my-component").is_ok());
        assert!(StashStore::validate_id("a").is_ok());
        assert!(StashStore::validate_id(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn validate_id_rejects_empty() {
        let err = StashStore::validate_id("").expect_err("empty must fail");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn validate_id_rejects_too_long() {
        let err = StashStore::validate_id(&"a".repeat(65)).expect_err("too long must fail");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn validate_id_rejects_path_chars() {
        for bad in ["../etc/passwd", "foo/bar", "foo.bar", "foo bar"] {
            let err = StashStore::validate_id(bad).expect_err(&format!("{bad:?} must fail"));
            assert_eq!(err.kind(), "invalid_param");
        }
    }

    // ── ensure_dirs ──────────────────────────────────────────────────────────

    #[test]
    fn ensure_dirs_creates_expected_subdirectories() {
        let (store, _dir) = make_store();
        for sub in [
            DIR_COMPONENTS,
            DIR_REVISIONS,
            DIR_WORKSPACES,
            DIR_PROVIDERS,
            DIR_TARGETS,
        ] {
            assert!(store.root.join(sub).is_dir(), "missing {sub}");
        }
        // lab-qz6a.24/25: secondary index directories must also exist
        assert!(
            store.root.join(DIR_REVISIONS_BY_COMPONENT).is_dir(),
            "missing revisions/by-component"
        );
        assert!(
            store.root.join(DIR_PROVIDERS_BY_COMPONENT).is_dir(),
            "missing providers/by-component"
        );
        assert!(
            !store.root.join("objects").exists(),
            "objects/ must not exist"
        );
    }

    // ── component I/O ────────────────────────────────────────────────────────

    #[test]
    fn component_roundtrip() {
        let (store, _dir) = make_store();
        let comp = sample_component("comp-01");
        assert!(
            store
                .read_component("comp-01")
                .expect("read absent")
                .is_none()
        );
        store.write_component(&comp).expect("write");
        let back = store
            .read_component("comp-01")
            .expect("read")
            .expect("present");
        assert_eq!(back.id, comp.id);
        assert_eq!(back.name, comp.name);
    }

    #[test]
    fn list_components_excludes_lock_files() {
        let (store, _dir) = make_store();
        store
            .write_component(&sample_component("comp-01"))
            .expect("write");
        // Simulate a stale lock file in the same directory.
        std::fs::write(store.root.join(DIR_COMPONENTS).join("comp-01.lock"), b"")
            .expect("write lock file");
        let list = store.list_components().expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "comp-01");
    }

    #[test]
    fn delete_component_record_is_idempotent() {
        let (store, _dir) = make_store();
        store
            .delete_component_record("nonexistent-id")
            .expect("delete absent");
        store
            .write_component(&sample_component("comp-02"))
            .expect("write");
        store
            .delete_component_record("comp-02")
            .expect("delete present");
        assert!(
            store
                .read_component("comp-02")
                .expect("read after delete")
                .is_none()
        );
    }

    // ── revision I/O ─────────────────────────────────────────────────────────

    #[test]
    fn revision_roundtrip_and_filter_by_component() {
        let (store, _dir) = make_store();
        let rev1 = sample_revision("rev-01", "comp-01");
        let rev2 = sample_revision("rev-02", "comp-02");
        store.write_revision_meta(&rev1).expect("write rev1");
        store.write_revision_meta(&rev2).expect("write rev2");

        let comp1_revs = store
            .list_revisions_for("comp-01")
            .expect("list comp-01 revs");
        assert_eq!(comp1_revs.len(), 1);
        assert_eq!(comp1_revs[0].id, "rev-01");

        let comp2_revs = store
            .list_revisions_for("comp-02")
            .expect("list comp-02 revs");
        assert_eq!(comp2_revs.len(), 1);
        assert_eq!(comp2_revs[0].id, "rev-02");
    }

    /// lab-qz6a.24: fallback scan works for stores that have meta.json but no index.
    #[test]
    fn revision_fallback_scan_without_index() {
        let (store, _dir) = make_store();
        // Write meta.json directly, bypassing write_revision_meta (no index written).
        let rev = sample_revision("rev-99", "comp-99");
        let meta_path = store.revision_meta_path("rev-99");
        std::fs::create_dir_all(meta_path.parent().unwrap()).expect("create dir");
        std::fs::write(&meta_path, serde_json::to_vec_pretty(&rev).unwrap()).expect("write meta");

        // The index does not exist for comp-99; fallback scan must find it.
        let revs = store.list_revisions_for("comp-99").expect("list");
        assert_eq!(revs.len(), 1);
        assert_eq!(revs[0].id, "rev-99");
    }

    // ── target I/O ───────────────────────────────────────────────────────────

    #[test]
    fn target_roundtrip_and_delete() {
        let (store, _dir) = make_store();
        let target = StashDeployTarget::Local {
            id: "t-01".to_string(),
            name: "home".to_string(),
            path: PathBuf::from("/home/user/.claude"),
        };
        assert!(store.read_target("t-01").expect("read absent").is_none());
        store.write_target("t-01", &target).expect("write");
        let back = store.read_target("t-01").expect("read").expect("present");
        let StashDeployTarget::Local { id, .. } = back else {
            panic!("wrong variant");
        };
        assert_eq!(id, "t-01");
        store.delete_target("t-01").expect("delete");
        assert!(
            store
                .read_target("t-01")
                .expect("read after delete")
                .is_none()
        );
    }

    #[test]
    fn list_targets_returns_all() {
        let (store, _dir) = make_store();
        for i in 0..3_u8 {
            let t = StashDeployTarget::Local {
                id: format!("t-{i:02}"),
                name: format!("target-{i}"),
                path: PathBuf::from(format!("/tmp/t{i}")),
            };
            store.write_target(&format!("t-{i:02}"), &t).expect("write");
        }
        let list = store.list_targets().expect("list");
        assert_eq!(list.len(), 3);
    }

    // ── provider I/O ─────────────────────────────────────────────────────────

    #[test]
    fn provider_roundtrip_and_filter_by_component() {
        let (store, _dir) = make_store();
        let p1 = sample_provider("prov-01", "comp-01");
        let p2 = sample_provider("prov-02", "comp-02");
        store.write_provider(&p1).expect("write p1");
        store.write_provider(&p2).expect("write p2");

        let comp1_providers = store.list_providers_for("comp-01").expect("list");
        assert_eq!(comp1_providers.len(), 1);
        assert_eq!(comp1_providers[0].id, "prov-01");
    }

    /// lab-qz6a.25: fallback scan works for stores that have provider JSON but no index.
    #[test]
    fn provider_fallback_scan_without_index() {
        let (store, _dir) = make_store();
        // Write provider record directly, bypassing write_provider (no index written).
        let prov = sample_provider("prov-99", "comp-99");
        let prov_path = store.provider_record_path("prov-99");
        std::fs::create_dir_all(prov_path.parent().unwrap()).expect("create dir");
        std::fs::write(&prov_path, serde_json::to_vec_pretty(&prov).unwrap())
            .expect("write provider");

        // The index does not exist for comp-99; fallback scan must find it.
        let providers = store.list_providers_for("comp-99").expect("list");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "prov-99");
    }

    #[test]
    fn delete_provider_is_idempotent() {
        let (store, _dir) = make_store();
        store.delete_provider("nonexistent").expect("delete absent");
    }

    // ── advisory locks ───────────────────────────────────────────────────────

    #[test]
    fn with_component_lock_runs_closure() {
        let (store, _dir) = make_store();
        let result = store
            .with_component_lock("comp-01", || Ok(42_u32))
            .expect("lock and run");
        assert_eq!(result, 42);
    }

    #[test]
    fn with_deploy_lock_runs_closure_and_succeeds() {
        let (store, _dir) = make_store();
        let result = store
            .with_deploy_lock("comp-01", 500, || Ok("done"))
            .expect("deploy lock");
        assert_eq!(result, "done");
    }

    #[test]
    fn with_deploy_lock_timeout_returns_conflict() {
        let (store, _dir) = make_store();
        let store2 = store.clone();

        // Hold the deploy lock from a background thread.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();

        let handle = std::thread::spawn(move || {
            store2
                .with_deploy_lock("comp-lock-test", 5_000, || {
                    ready_tx.send(()).expect("send ready");
                    release_rx.recv().expect("wait for release");
                    Ok::<(), ToolError>(())
                })
                .expect("background lock");
        });

        ready_rx.recv().expect("wait for background lock held");

        // Try to acquire with a tiny timeout — should time out.
        let err = store
            .with_deploy_lock("comp-lock-test", 150, || Ok(()))
            .expect_err("should time out");
        assert_eq!(err.kind(), "conflict");

        // Release the background lock.
        release_tx.send(()).expect("send release");
        handle.join().expect("join thread");
    }

    /// Verifies that mutual exclusion is actually held for the **entire** duration
    /// of `f()`, not just until the guard binding goes out of scope.
    ///
    /// The test: a background thread acquires the lock and, from inside `f()`,
    /// signals readiness.  The main thread then verifies it cannot acquire the
    /// lock (conflict) while `f()` is still running.  After `f()` returns the
    /// main thread re-tries and must succeed — proving the guard was live across
    /// `f()` but released once `f()` returned.
    #[test]
    fn with_deploy_lock_holds_guard_across_f() {
        let (store, _dir) = make_store();
        let store2 = store.clone();
        let store3 = store.clone();

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();

        // Thread 1: hold the lock for the duration of f().
        let handle = std::thread::spawn(move || {
            store2
                .with_deploy_lock("lock-guard-test", 5_000, || {
                    ready_tx.send(()).expect("send ready");
                    // Stay inside f() until told to release.
                    release_rx.recv().expect("wait for release");
                    Ok::<(), ToolError>(())
                })
                .expect("background lock");
        });

        // Wait until f() is executing in thread 1 (guard must be live).
        ready_rx.recv().expect("wait for background lock held");

        // While f() is still running in thread 1, we must NOT be able to acquire.
        let conflict = store
            .with_deploy_lock("lock-guard-test", 100, || Ok(()))
            .expect_err("must conflict while f() is running");
        assert_eq!(conflict.kind(), "conflict", "guard must be held during f()");

        // Release the closure in thread 1 so the guard is dropped.
        release_tx.send(()).expect("send release");
        handle.join().expect("join background thread");

        // Now that f() has returned and the guard is dropped, we must succeed.
        store3
            .with_deploy_lock("lock-guard-test", 500, || Ok(()))
            .expect("must succeed once guard is released after f()");
    }

    // ── path helpers ─────────────────────────────────────────────────────────

    #[test]
    fn workspace_path_file_shaped_uses_filename() {
        let store = StashStore::new(PathBuf::from("/stash"));
        let p = store.workspace_path("comp-01", StashWorkspaceShape::File, Some("config.json"));
        assert_eq!(p, PathBuf::from("/stash/workspaces/comp-01/config.json"));
    }

    #[test]
    fn workspace_path_directory_shaped_returns_dir() {
        let store = StashStore::new(PathBuf::from("/stash"));
        let p = store.workspace_path("comp-01", StashWorkspaceShape::Directory, None);
        assert_eq!(p, PathBuf::from("/stash/workspaces/comp-01"));
    }

    // ── index corruption (lab-4sd2) ──────────────────────────────────────────

    /// A corrupt revision index must not destroy prior IDs on the next save.
    /// After corruption, append_revision_to_index must return decode_error.
    #[test]
    fn corrupt_revision_index_append_returns_decode_error_not_silent_reset() {
        let (store, _dir) = make_store();
        let rev1 = sample_revision("rev-01", "comp-01");
        store.write_revision_meta(&rev1).expect("write rev1");

        // Corrupt the index file.
        let index_path = store.component_revision_index_path("comp-01");
        std::fs::write(&index_path, b"not valid json").expect("corrupt index");

        // Attempting to append a new revision must fail with decode_error,
        // NOT silently overwrite the index with a single-entry vec.
        let err = store
            .append_revision_to_index("comp-01", "rev-02")
            .expect_err("corrupt index must produce error");
        assert_eq!(
            err.kind(),
            "decode_error",
            "expected decode_error, got: {err:?}"
        );

        // The corrupt file must still be intact (not overwritten).
        let contents = std::fs::read(&index_path).expect("read after failed append");
        assert_eq!(
            &contents, b"not valid json",
            "corrupt index must not be overwritten"
        );
    }

    /// A corrupt revision index on the read path must fall back to the full
    /// scan and return all revisions whose meta.json files are intact.
    #[test]
    fn corrupt_revision_index_read_falls_back_to_full_scan() {
        let (store, _dir) = make_store();
        // Write two revisions (this creates the index).
        let rev1 = sample_revision("rev-01", "comp-01");
        let rev2 = sample_revision("rev-02", "comp-01");
        store.write_revision_meta(&rev1).expect("write rev1");
        store.write_revision_meta(&rev2).expect("write rev2");

        // Corrupt the index.
        let index_path = store.component_revision_index_path("comp-01");
        std::fs::write(&index_path, b"{{corrupt}}").expect("corrupt index");

        // list_revisions_for must fall back and find both revisions via full scan.
        let revs = store
            .list_revisions_for("comp-01")
            .expect("list must succeed via fallback scan");
        assert_eq!(
            revs.len(),
            2,
            "both revisions must be found despite corrupt index"
        );
    }

    /// A corrupt provider index on the append path must return decode_error.
    #[test]
    fn corrupt_provider_index_append_returns_decode_error() {
        let (store, _dir) = make_store();
        let p1 = sample_provider("prov-01", "comp-01");
        store.write_provider(&p1).expect("write prov1");

        let index_path = store.component_provider_index_path("comp-01");
        std::fs::write(&index_path, b"not json").expect("corrupt index");

        let err = store
            .append_provider_to_index("comp-01", "prov-02")
            .expect_err("must fail");
        assert_eq!(err.kind(), "decode_error");
    }

    /// A corrupt provider index on the read path must fall back to the full scan.
    #[test]
    fn corrupt_provider_index_read_falls_back_to_full_scan() {
        let (store, _dir) = make_store();
        let p1 = sample_provider("prov-01", "comp-01");
        let p2 = sample_provider("prov-02", "comp-01");
        store.write_provider(&p1).expect("write p1");
        store.write_provider(&p2).expect("write p2");

        let index_path = store.component_provider_index_path("comp-01");
        std::fs::write(&index_path, b"{{corrupt}}").expect("corrupt");

        let providers = store
            .list_providers_for("comp-01")
            .expect("fallback scan must succeed");
        assert_eq!(providers.len(), 2, "both providers found via fallback scan");
    }

    // ── delete_component (lab-3mjv) ──────────────────────────────────────────

    /// delete_component removes the component record, all revisions, revision
    /// index, all provider records, provider index, and workspace directory.
    /// A second call on the now-absent component must be idempotent.
    #[test]
    fn delete_component_cleans_up_all_associated_data() {
        let (store, _dir) = make_store();

        // Set up: component, two revisions, two providers, workspace dir.
        let comp = sample_component("comp-del");
        store.write_component(&comp).expect("write comp");

        let rev1 = sample_revision("rev-d1", "comp-del");
        let rev2 = sample_revision("rev-d2", "comp-del");
        store.write_revision_meta(&rev1).expect("write rev1");
        store.write_revision_meta(&rev2).expect("write rev2");
        // Create actual revision dirs to prove they are removed.
        let rev1_dir = store.revision_dir("rev-d1");
        let rev2_dir = store.revision_dir("rev-d2");
        std::fs::create_dir_all(&rev1_dir).expect("create rev1 dir");
        std::fs::create_dir_all(&rev2_dir).expect("create rev2 dir");

        let prov1 = sample_provider("prov-d1", "comp-del");
        let prov2 = sample_provider("prov-d2", "comp-del");
        store.write_provider(&prov1).expect("write prov1");
        store.write_provider(&prov2).expect("write prov2");

        let workspace = store.workspace_dir("comp-del");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        // Verify everything exists before deletion.
        assert!(
            store.component_record_path("comp-del").exists(),
            "comp record"
        );
        assert!(rev1_dir.exists(), "rev1 dir");
        assert!(rev2_dir.exists(), "rev2 dir");
        assert!(
            store.component_revision_index_path("comp-del").exists(),
            "rev index"
        );
        assert!(
            store.provider_record_path("prov-d1").exists(),
            "prov1 record"
        );
        assert!(
            store.provider_record_path("prov-d2").exists(),
            "prov2 record"
        );
        assert!(
            store.component_provider_index_path("comp-del").exists(),
            "prov index"
        );
        assert!(workspace.exists(), "workspace");

        // Delete.
        store
            .delete_component("comp-del")
            .expect("delete_component");

        // Verify everything is gone.
        assert!(
            !store.component_record_path("comp-del").exists(),
            "comp record gone"
        );
        assert!(!rev1_dir.exists(), "rev1 dir gone");
        assert!(!rev2_dir.exists(), "rev2 dir gone");
        assert!(
            !store.component_revision_index_path("comp-del").exists(),
            "rev index gone"
        );
        assert!(
            !store.provider_record_path("prov-d1").exists(),
            "prov1 gone"
        );
        assert!(
            !store.provider_record_path("prov-d2").exists(),
            "prov2 gone"
        );
        assert!(
            !store.component_provider_index_path("comp-del").exists(),
            "prov index gone"
        );
        assert!(!workspace.exists(), "workspace gone");

        // A second delete must be idempotent.
        store
            .delete_component("comp-del")
            .expect("idempotent second delete");
    }

    /// delete_component on a component with no data must succeed silently.
    #[test]
    fn delete_component_is_idempotent_for_nonexistent_component() {
        let (store, _dir) = make_store();
        store
            .delete_component("ghost-comp")
            .expect("delete of non-existent component must be idempotent");
    }
}
