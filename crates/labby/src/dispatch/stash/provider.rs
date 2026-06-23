//! `StashProvider` trait — the interface all storage backends must implement.
//!
//! Each provider kind (filesystem, …) implements this trait. The trait is
//! synchronous by design — providers are called from synchronous service
//! functions that are themselves wrapped in `spawn_blocking` at the async
//! boundary.

use labby_apis::stash::types::{StashProviderRecord, StashRevision};

use crate::dispatch::error::ToolError;
use crate::dispatch::stash::store::StashStore;

/// Storage backend interface for stash sync operations.
///
/// Implementors are constructed from a [`StashProviderRecord`] via
/// [`crate::dispatch::stash::providers::provider_from_record`].
///
/// All methods are **synchronous** and safe to call from within a
/// `tokio::task::spawn_blocking` context.
#[allow(dead_code)]
pub trait StashProvider: Send + Sync {
    /// Return the driver kind string (e.g. `"filesystem"`).
    fn kind(&self) -> &'static str;

    /// Push a revision's files to the remote storage location.
    ///
    /// Copies files from `store.revision_files_path(rev.id)` to the
    /// provider's remote root for `component_id`.
    fn push_revision(
        &self,
        store: &StashStore,
        component_id: &str,
        rev: &StashRevision,
    ) -> Result<(), ToolError>;

    /// Pull the latest revision from the remote storage location.
    ///
    /// Returns `None` when no remote revisions exist for `component_id`.
    /// On success, returns a freshly created [`StashRevision`] record whose
    /// **files are already written** to `store.revision_files_path(new_rev_id)`,
    /// but whose **meta is NOT yet written** — the caller must call
    /// `store.write_revision_meta(&rev)` and update `head_revision_id` while
    /// holding the component advisory lock (lab-qytb).
    fn pull_latest(
        &self,
        store: &StashStore,
        component_id: &str,
    ) -> Result<Option<StashRevision>, ToolError>;

    /// List remote revision IDs available for `component_id`.
    ///
    /// Returns an ordered list of revision ID strings. The caller determines
    /// which is "latest" (typically the last-written directory).
    fn list_remote(&self, component_id: &str) -> Result<Vec<String>, ToolError>;
}

/// Helper: build a [`StashProviderRecord`] from link parameters.
///
/// Exposed so `service::provider_link` can construct the record without
/// duplicating the ULID-generation logic.
pub fn build_provider_record(
    component_id: &str,
    kind: &str,
    label: &str,
    config: serde_json::Value,
) -> StashProviderRecord {
    StashProviderRecord {
        id: ulid::Ulid::new().to_string().to_lowercase(),
        component_id: component_id.to_string(),
        kind: kind.to_string(),
        label: label.to_string(),
        config,
    }
}
