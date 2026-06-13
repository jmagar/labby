//! First-run detection + state-machine evaluator for `setup.state`.

use lab_apis::setup::{SetupSnapshot, SetupState};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::env_merge::snapshot_mtime;
use crate::registry::{ToolRegistry, service_meta};

use super::client::{draft_path, env_path};
use super::draft;

/// Read every required env var from the registry. A service contributes its
/// required vars unconditionally — wizards skip optional ones.
fn registry_required_keys(registry: &ToolRegistry) -> Vec<String> {
    let mut keys = Vec::new();
    for entry in registry.services() {
        if let Some(meta) = service_meta(entry.name) {
            for var in meta.required_env {
                keys.push(var.name.to_string());
            }
        }
    }
    keys
}

/// Build a `SetupSnapshot` describing the current state of `~/.lab/.env`.
#[must_use]
pub fn snapshot(registry: &ToolRegistry) -> SetupSnapshot {
    let env = env_path();
    let draft = draft_path();
    let env_exists = env.exists();
    let has_draft = draft.exists();
    let draft_stale = draft_is_stale(&env, &draft);
    let draft_metadata = draft_metadata(&env, &draft);

    let state = if !env_exists {
        SetupState::Uninitialized
    } else {
        let entries = draft::read_entries(&env);
        let registered: Vec<String> = registry_required_keys(registry);
        let missing: Vec<String> = registered
            .into_iter()
            .filter(|key| !entries.iter().any(|e| &e.key == key && !e.value.is_empty()))
            .collect();
        if missing.is_empty() {
            SetupState::Ready
        } else if entries.is_empty() {
            SetupState::ConfigMissing { envars: missing }
        } else {
            SetupState::PartiallyConfigured { missing }
        }
    };

    SetupSnapshot {
        first_run: matches!(
            state,
            SetupState::Uninitialized | SetupState::ConfigMissing { .. }
        ),
        env_path: env,
        draft_path: draft,
        last_completed_step: 0,
        draft_stale,
        has_draft,
        draft_entry_count: draft_metadata.draft_entry_count,
        env_mtime_unix_seconds: draft_metadata.env_mtime_unix_seconds,
        draft_mtime_unix_seconds: draft_metadata.draft_mtime_unix_seconds,
        state,
    }
}

struct DraftMetadata {
    draft_entry_count: usize,
    env_mtime_unix_seconds: Option<u64>,
    draft_mtime_unix_seconds: Option<u64>,
}

fn draft_metadata(env: &Path, draft: &Path) -> DraftMetadata {
    DraftMetadata {
        draft_entry_count: draft::read_entries(draft).len(),
        env_mtime_unix_seconds: unix_seconds(snapshot_mtime(env)),
        draft_mtime_unix_seconds: unix_seconds(snapshot_mtime(draft)),
    }
}

fn unix_seconds(mtime: Option<SystemTime>) -> Option<u64> {
    mtime
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

fn draft_is_stale(env: &Path, draft: &Path) -> bool {
    if !draft.exists() {
        return false;
    }
    let env_mtime = snapshot_mtime(env);
    let draft_mtime = snapshot_mtime(draft);
    match (env_mtime, draft_mtime) {
        (Some(e), Some(d)) => e > d,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{draft_metadata, unix_seconds};
    use std::time::{Duration, SystemTime};

    // Note: snapshot() reads LAB_HOME via env, which Rust 2024 marks unsafe.
    // The crate forbids unsafe, so we can't mutate the env var inside tests
    // here. End-to-end coverage of the state machine ships in the smoke test
    // recipe (`just smoke-setup`) added in Chunk F.

    #[test]
    fn draft_metadata_counts_entries_and_reports_unix_mtimes() {
        let temp = tempfile::tempdir().unwrap();
        let env = temp.path().join(".env");
        let draft = temp.path().join(".env.draft");
        std::fs::write(&env, "LAB_MCP_HTTP_TOKEN=abc\n").unwrap();
        std::fs::write(&draft, "LAB_TEST=1\n# comment\nOTHER=2\n").unwrap();

        let metadata = draft_metadata(&env, &draft);

        assert_eq!(metadata.draft_entry_count, 2);
        assert!(metadata.env_mtime_unix_seconds.is_some());
        assert!(metadata.draft_mtime_unix_seconds.is_some());
    }

    #[test]
    fn unix_seconds_returns_none_before_epoch() {
        let before_epoch = SystemTime::UNIX_EPOCH - Duration::from_secs(1);

        assert_eq!(unix_seconds(Some(before_epoch)), None);
        assert_eq!(unix_seconds(None), None);
    }
}
