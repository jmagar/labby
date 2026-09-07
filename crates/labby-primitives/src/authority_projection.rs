//! Versioned, bounded wire contract for Labby authority projection into Depot.

use serde::{Deserialize, Serialize};

pub const AUTHORITY_PROJECTION_SCHEMA_VERSION: u16 = 1;
pub const MAX_AUTHORITY_RECORDS_PER_BATCH: usize = 256;
pub const MAX_AUTHORITY_ENVELOPE_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionKind {
    Delta,
    Snapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthorityProjectionRecord {
    pub sequence: u64,
    pub resource_type: String,
    pub resource_id: String,
    pub operation: String,
    pub value: Option<serde_json::Value>,
}

/// The signature is Ed25519 over deterministic canonical JSON encoding of
/// every field except `signature`. Digests are lowercase SHA-256 hex.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthorityProjectionEnvelope {
    pub schema_version: u16,
    pub installation_id: String,
    pub organization_id: String,
    pub sequence_start: u64,
    pub sequence_end: u64,
    pub kind: ProjectionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_base_sequence: Option<u64>,
    /// Stable identifier shared by every chunk of one staged snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    /// Activates the staged snapshot after this envelope is durably accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_complete: Option<bool>,
    pub generated_at: String,
    pub previous_digest: Option<String>,
    pub payload_digest: String,
    pub key_id: String,
    pub records: Vec<AuthorityProjectionRecord>,
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthorityProjectionAck {
    pub organization_id: String,
    pub highest_contiguous_sequence: u64,
    pub last_envelope_digest: String,
    pub snapshot_digest: Option<String>,
}

impl AuthorityProjectionEnvelope {
    #[must_use]
    pub fn within_bounds(&self) -> bool {
        self.schema_version == AUTHORITY_PROJECTION_SCHEMA_VERSION
            && !self.installation_id.trim().is_empty()
            && !self.organization_id.trim().is_empty()
            && !self.key_id.trim().is_empty()
            && self.records.len() <= MAX_AUTHORITY_RECORDS_PER_BATCH
            && self.sequence_start <= self.sequence_end
            && self.records.first().map(|record| record.sequence) == Some(self.sequence_start)
            && self.records.last().map(|record| record.sequence) == Some(self.sequence_end)
            && self
                .records
                .windows(2)
                .all(|pair| pair[1].sequence == pair[0].sequence + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_rejects_gaps_and_unknown_versions() {
        let record = |sequence| AuthorityProjectionRecord {
            sequence,
            resource_type: "team".into(),
            resource_id: format!("t{sequence}"),
            operation: "upsert".into(),
            value: Some(serde_json::json!({"policy_epoch":sequence})),
        };
        let mut envelope = AuthorityProjectionEnvelope {
            schema_version: 1,
            installation_id: "i".into(),
            organization_id: "o".into(),
            sequence_start: 1,
            sequence_end: 2,
            kind: ProjectionKind::Delta,
            snapshot_base_sequence: None,
            snapshot_id: None,
            snapshot_complete: None,
            generated_at: "2026-09-05T00:00:00Z".into(),
            previous_digest: None,
            payload_digest: format!("sha256:{}", "11".repeat(32)),
            key_id: "k".into(),
            records: vec![record(1), record(2)],
            signature: "sig".into(),
        };
        assert!(envelope.within_bounds());
        envelope.records[1].sequence = 3;
        assert!(!envelope.within_bounds());
        envelope.schema_version = 2;
        assert!(!envelope.within_bounds());
    }
}
