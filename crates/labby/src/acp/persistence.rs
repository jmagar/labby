#![allow(dead_code)]

//! ACP persistence — JSON-file implementation (legacy) and re-export of the
//! new `AcpPersistence` trait.
//!
//! `JsonFileAcpPersistence` is retained here because `registry.rs` still uses
//! the legacy sync helpers. It will be removed once the registry migrates to
//! `SqliteAcpPersistence` (bead 6+).
//!
//! The new `AcpPersistence` trait lives in `labby_apis::acp::persistence`.
//! The SQLite implementation lives in
//! `crates/lab/src/dispatch/acp/persistence.rs`.

use std::path::{Path, PathBuf};

use tokio::fs;

#[allow(unused_imports)]
pub use labby_apis::acp::persistence::AcpPersistence;

use super::types::{BridgeEvent, BridgeSessionSummary};

// ── Legacy JSON file persistence ──────────────────────────────────────────────

/// Legacy JSON-file persistence used by `AcpSessionRegistry`.
///
/// Deprecated: use `SqliteAcpPersistence` from
/// `crate::dispatch::acp::persistence` for new code.
#[derive(Clone)]
pub struct JsonFileAcpPersistence {
    base_dir: PathBuf,
}

impl JsonFileAcpPersistence {
    #[must_use]
    pub fn new() -> Self {
        let base_dir = std::env::var("LAB_ACP_SESSION_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                Path::new(&home).join(".lab").join("acp-sessions")
            });
        Self { base_dir }
    }

    pub fn load_sessions_sync(&self) -> Vec<BridgeSessionSummary> {
        drop(std::fs::create_dir_all(&self.base_dir));
        let index = self.base_dir.join("sessions.json");
        let Ok(raw) = std::fs::read_to_string(index) else {
            return Vec::new();
        };
        serde_json::from_str::<Vec<BridgeSessionSummary>>(&raw).unwrap_or_default()
    }

    pub fn load_events_sync(&self, session_id: &str) -> Vec<BridgeEvent> {
        drop(std::fs::create_dir_all(&self.base_dir));
        let path = self.event_path(session_id);
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        raw.lines()
            .filter_map(|line| serde_json::from_str::<BridgeEvent>(line).ok())
            .collect()
    }

    pub async fn save_sessions(
        &self,
        sessions: &[BridgeSessionSummary],
    ) -> Result<(), std::io::Error> {
        fs::create_dir_all(&self.base_dir).await?;
        let body = serde_json::to_vec_pretty(sessions).map_err(std::io::Error::other)?;
        fs::write(self.base_dir.join("sessions.json"), body).await
    }

    pub async fn save_events(
        &self,
        session_id: &str,
        events: &[BridgeEvent],
    ) -> Result<(), std::io::Error> {
        fs::create_dir_all(&self.base_dir).await?;
        let mut out = String::new();
        for event in events {
            let line = serde_json::to_string(event).map_err(std::io::Error::other)?;
            out.push_str(&line);
            out.push('\n');
        }
        fs::write(self.event_path(session_id), out).await
    }

    fn event_path(&self, session_id: &str) -> PathBuf {
        self.base_dir.join(format!("{session_id}.jsonl"))
    }
}

impl Default for JsonFileAcpPersistence {
    fn default() -> Self {
        Self::new()
    }
}
