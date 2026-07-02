//! Pure data types for the `setup` Bootstrap service.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Sentinel returned by `setup.draft.get` in place of any value whose owning
/// `UiSchema.secret == true` flag is set. Never let a real secret leave the
/// dispatch layer.
pub const SECRET_SENTINEL: &str = "***";

/// First-run setup state machine. Modeled as states (not booleans) so the
/// wizard UI has a single source of truth for "what step are we on".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SetupState {
    /// `~/.labby/.env` does not exist at all.
    Uninitialized,
    /// `.env` exists but is missing one or more required core env vars.
    ConfigMissing { envars: Vec<String> },
    /// `.env` is partially populated; some service env keys are missing.
    PartiallyConfigured { missing: Vec<String> },
    /// All required keys present; running health probes.
    HealthChecking { services: Vec<String> },
    /// Probes complete; configuration is committed and healthy.
    Ready,
}

/// Snapshot returned by `setup.state` to the wizard / settings UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupSnapshot {
    /// True when no `~/.labby/.env` exists or it lacks required keys.
    pub first_run: bool,
    pub env_path: PathBuf,
    pub draft_path: PathBuf,
    /// Last completed wizard step (0-indexed). UI uses this to resume.
    pub last_completed_step: u8,
    /// True when `.env` was modified since the draft snapshot was taken.
    pub draft_stale: bool,
    /// True when a `.env.draft` is present.
    pub has_draft: bool,
    /// Number of key/value entries currently present in `.env.draft`.
    pub draft_entry_count: usize,
    /// Last modified time for `.env`, as Unix seconds.
    pub env_mtime_unix_seconds: Option<u64>,
    /// Last modified time for `.env.draft`, as Unix seconds.
    pub draft_mtime_unix_seconds: Option<u64>,
    pub state: SetupState,
}

/// Single key=value entry within a draft mutation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftEntry {
    pub key: String,
    pub value: String,
}

/// Optional grouping wrapper for callers that prefer `{ service, entries }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftSection {
    pub service: String,
    pub entries: Vec<DraftEntry>,
}

/// Outcome envelope for `setup.draft.commit` (and `setup.finalize`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitOutcome {
    /// Number of keys written to `.env`.
    pub written: usize,
    /// Skip warnings (when force=false and a key collided).
    pub skipped: Vec<String>,
    /// Backup path created at the start of the commit (None on idempotent no-op).
    pub backup_path: Option<PathBuf>,
    /// Number of services that passed `doctor.audit.full`.
    pub audit_pass_count: usize,
    /// Total services audited.
    pub audit_total_count: usize,
}
