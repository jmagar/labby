use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TailnetIdentity {
    pub node_key: String,
    pub login_name: String,
    pub hostname: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingEnrollment {
    pub node_id: String,
    pub token: String,
    pub token_fingerprint: String,
    pub tailnet_identity: TailnetIdentity,
    pub client_version: String,
    pub first_seen_unix_ms: i64,
    pub last_seen_unix_ms: i64,
    pub metadata: Option<serde_json::Value>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovedEnrollment {
    pub node_id: String,
    pub token: String,
    pub token_fingerprint: String,
    pub approved_at_unix_ms: i64,
    pub approval_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeniedEnrollment {
    pub node_id: String,
    pub token: String,
    pub token_fingerprint: String,
    pub denied_at_unix_ms: i64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct EnrollmentSnapshot {
    pub pending: BTreeMap<String, PendingEnrollment>,
    pub approved: BTreeMap<String, ApprovedEnrollment>,
    pub denied: BTreeMap<String, DeniedEnrollment>,
}

#[derive(Debug, Clone)]
pub struct EnrollmentAttempt {
    pub node_id: String,
    pub token: String,
    pub tailnet_identity: TailnetIdentity,
    pub client_version: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollmentDecision {
    Approved(ApprovedEnrollment),
    PendingRequired,
    Denied(DeniedEnrollment),
    TokenMismatch(ApprovedEnrollment),
}

#[derive(Debug)]
pub struct EnrollmentStore {
    path: PathBuf,
    io_lock: Mutex<()>,
}

impl EnrollmentStore {
    pub async fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create {}", parent.display()))?;
        }
        if !path_exists(&path).await? {
            write_snapshot_atomically(&path, &EnrollmentSnapshot::default()).await?;
        }
        Ok(Self {
            path,
            io_lock: Mutex::new(()),
        })
    }

    pub async fn list(&self) -> Result<EnrollmentSnapshot> {
        let _guard = self.io_lock.lock().await;
        self.read_snapshot().await
    }

    pub async fn record_pending(&self, attempt: EnrollmentAttempt) -> Result<PendingEnrollment> {
        let _guard = self.io_lock.lock().await;
        let mut snapshot = self.read_snapshot().await?;
        let now = now_unix_ms()?;
        let fingerprint = token_fingerprint(&attempt.token);
        let pending = snapshot
            .pending
            .entry(attempt.node_id.clone())
            .and_modify(|record| {
                record.token = attempt.token.clone();
                record.token_fingerprint = fingerprint.clone();
                record.tailnet_identity = attempt.tailnet_identity.clone();
                record.client_version = attempt.client_version.clone();
                record.last_seen_unix_ms = now;
                record.metadata = attempt.metadata.clone();
            })
            .or_insert_with(|| PendingEnrollment {
                node_id: attempt.node_id.clone(),
                token: attempt.token.clone(),
                token_fingerprint: fingerprint.clone(),
                tailnet_identity: attempt.tailnet_identity.clone(),
                client_version: attempt.client_version.clone(),
                first_seen_unix_ms: now,
                last_seen_unix_ms: now,
                metadata: attempt.metadata.clone(),
                note: None,
            })
            .clone();
        snapshot.denied.remove(&attempt.node_id);
        write_snapshot_atomically(&self.path, &snapshot).await?;
        tracing::info!(
            surface = "node",
            service = "node.enrollment",
            action = "enrollment.record_pending",
            actor = "node",
            outcome = "success",
            entity_kind = "node",
            entity_id = %pending.node_id,
            token_fingerprint = %pending.token_fingerprint,
            "node enrollment recorded as pending",
        );
        Ok(pending)
    }

    pub async fn approve(&self, node_id: &str, note: Option<String>) -> Result<ApprovedEnrollment> {
        let _guard = self.io_lock.lock().await;
        let mut snapshot = self.read_snapshot().await?;
        if let Some(approved) = snapshot.approved.get(node_id) {
            tracing::info!(
                surface = "node",
                service = "node.enrollment",
                action = "enrollment.approve",
                actor = "operator",
                outcome = "already_approved",
                entity_kind = "node",
                entity_id = %approved.node_id,
                token_fingerprint = %approved.token_fingerprint,
                "node enrollment already approved",
            );
            return Ok(approved.clone());
        }
        let pending = match snapshot.pending.remove(node_id) {
            Some(pending) => pending,
            None => {
                tracing::warn!(
                    surface = "node",
                    service = "node.enrollment",
                    action = "enrollment.approve",
                    actor = "operator",
                    outcome = "failure",
                    kind = "not_found",
                    entity_kind = "node",
                    entity_id = %node_id,
                    "pending node enrollment approval failed",
                );
                return Err(anyhow!("pending enrollment not found for `{node_id}`"));
            }
        };
        let approved = ApprovedEnrollment {
            node_id: pending.node_id.clone(),
            token: pending.token.clone(),
            token_fingerprint: pending.token_fingerprint.clone(),
            approved_at_unix_ms: now_unix_ms()?,
            approval_note: note,
        };
        snapshot
            .approved
            .insert(node_id.to_string(), approved.clone());
        snapshot.denied.remove(node_id);
        write_snapshot_atomically(&self.path, &snapshot).await?;
        tracing::info!(
            surface = "node",
            service = "node.enrollment",
            action = "enrollment.approve",
            actor = "operator",
            outcome = "success",
            entity_kind = "node",
            entity_id = %approved.node_id,
            token_fingerprint = %approved.token_fingerprint,
            "node enrollment approved",
        );
        Ok(approved)
    }

    pub async fn deny(&self, node_id: &str, reason: Option<String>) -> Result<DeniedEnrollment> {
        let _guard = self.io_lock.lock().await;
        let mut snapshot = self.read_snapshot().await?;
        if let Some(denied) = snapshot.denied.get(node_id) {
            tracing::info!(
                surface = "node",
                service = "node.enrollment",
                action = "enrollment.deny",
                actor = "operator",
                outcome = "already_denied",
                entity_kind = "node",
                entity_id = %denied.node_id,
                token_fingerprint = %denied.token_fingerprint,
                "node enrollment already denied",
            );
            return Ok(denied.clone());
        }

        let (token, token_fingerprint) = if let Some(pending) = snapshot.pending.remove(node_id) {
            (pending.token, pending.token_fingerprint)
        } else if let Some(approved) = snapshot.approved.remove(node_id) {
            (approved.token, approved.token_fingerprint)
        } else {
            tracing::warn!(
                surface = "node",
                service = "node.enrollment",
                action = "enrollment.deny",
                actor = "operator",
                outcome = "failure",
                kind = "not_found",
                entity_kind = "node",
                entity_id = %node_id,
                "node enrollment denial failed",
            );
            return Err(anyhow!("enrollment not found for `{node_id}`"));
        };

        let denied = DeniedEnrollment {
            node_id: node_id.to_string(),
            token,
            token_fingerprint,
            denied_at_unix_ms: now_unix_ms()?,
            reason,
        };
        snapshot.denied.insert(node_id.to_string(), denied.clone());
        write_snapshot_atomically(&self.path, &snapshot).await?;
        tracing::info!(
            surface = "node",
            service = "node.enrollment",
            action = "enrollment.deny",
            actor = "operator",
            outcome = "success",
            entity_kind = "node",
            entity_id = %denied.node_id,
            token_fingerprint = %denied.token_fingerprint,
            "node enrollment denied",
        );
        Ok(denied)
    }

    pub async fn validate(&self, node_id: &str, token: &str) -> Result<EnrollmentDecision> {
        let _guard = self.io_lock.lock().await;
        let snapshot = self.read_snapshot().await?;
        let token_fingerprint = token_fingerprint(token);
        if let Some(denied) = snapshot.denied.get(node_id) {
            tracing::warn!(
                surface = "node",
                service = "node.enrollment",
                action = "enrollment.validate",
                actor = "node",
                outcome = "denied",
                kind = "auth_failed",
                entity_kind = "node",
                entity_id = %node_id,
                token_fingerprint = %token_fingerprint,
                "node enrollment validation denied",
            );
            return Ok(EnrollmentDecision::Denied(denied.clone()));
        }
        if let Some(approved) = snapshot.approved.get(node_id) {
            if approved.token == token {
                tracing::info!(
                    surface = "node",
                    service = "node.enrollment",
                    action = "enrollment.validate",
                    actor = "node",
                    outcome = "approved",
                    entity_kind = "node",
                    entity_id = %node_id,
                    token_fingerprint = %approved.token_fingerprint,
                    "node enrollment validation approved",
                );
                return Ok(EnrollmentDecision::Approved(approved.clone()));
            }
            tracing::warn!(
                surface = "node",
                service = "node.enrollment",
                action = "enrollment.validate",
                actor = "node",
                outcome = "token_mismatch",
                kind = "auth_failed",
                entity_kind = "node",
                entity_id = %node_id,
                token_fingerprint = %token_fingerprint,
                expected_token_fingerprint = %approved.token_fingerprint,
                "node enrollment validation token mismatch",
            );
            return Ok(EnrollmentDecision::TokenMismatch(approved.clone()));
        }
        tracing::warn!(
            surface = "node",
            service = "node.enrollment",
            action = "enrollment.validate",
            actor = "node",
            outcome = "pending_required",
            kind = "auth_failed",
            entity_kind = "node",
            entity_id = %node_id,
            token_fingerprint = %token_fingerprint,
            "node enrollment validation requires pending enrollment",
        );
        Ok(EnrollmentDecision::PendingRequired)
    }

    async fn read_snapshot(&self) -> Result<EnrollmentSnapshot> {
        read_snapshot(&self.path).await
    }
}

fn now_unix_ms() -> Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("time went backwards")?;
    i64::try_from(duration.as_millis()).context("timestamp overflow")
}

async fn path_exists(path: &Path) -> Result<bool> {
    match fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("metadata {}", path.display())),
    }
}

async fn read_snapshot(path: &Path) -> Result<EnrollmentSnapshot> {
    let bytes = fs::read(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    if bytes.is_empty() {
        return Ok(EnrollmentSnapshot::default());
    }
    serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))
}

async fn write_snapshot_atomically(path: &Path, snapshot: &EnrollmentSnapshot) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(snapshot).context("serialize enrollment snapshot")?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)
        .await
        .with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .await
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

pub fn token_fingerprint(token: &str) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[must_use]
pub fn default_store_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".lab/node-enrollments.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(node_id: &str, token: &str) -> EnrollmentAttempt {
        EnrollmentAttempt {
            node_id: node_id.to_string(),
            token: token.to_string(),
            tailnet_identity: TailnetIdentity {
                node_key: format!("node-{node_id}"),
                login_name: "user@example.com".to_string(),
                hostname: format!("{node_id}.tail"),
            },
            client_version: "0.7.3".to_string(),
            metadata: Some(serde_json::json!({
                "discovered_configs": []
            })),
        }
    }

    #[tokio::test]
    async fn unknown_device_is_recorded_as_pending() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = EnrollmentStore::open(tempdir.path().join("enrollments.json"))
            .await
            .expect("open store");

        let pending = store
            .record_pending(attempt("device-1", "token-1"))
            .await
            .expect("record pending");

        assert_eq!(pending.node_id, "device-1");
        assert_eq!(pending.token, "token-1");
        assert_eq!(pending.client_version, "0.7.3");

        let snapshot = store.list().await.expect("list");
        assert_eq!(snapshot.pending.len(), 1);
        assert!(snapshot.approved.is_empty());
        assert!(snapshot.denied.is_empty());
    }

    #[tokio::test]
    async fn approve_moves_pending_record_to_approved() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = EnrollmentStore::open(tempdir.path().join("enrollments.json"))
            .await
            .expect("open store");
        store
            .record_pending(attempt("device-1", "token-1"))
            .await
            .expect("record pending");

        let approved = store
            .approve("device-1", Some("operator approved".to_string()))
            .await
            .expect("approve");

        assert_eq!(approved.node_id, "device-1");
        assert_eq!(approved.token, "token-1");
        assert_eq!(approved.approval_note.as_deref(), Some("operator approved"));

        let snapshot = store.list().await.expect("list");
        assert!(snapshot.pending.is_empty());
        assert_eq!(snapshot.approved.len(), 1);
        assert!(snapshot.denied.is_empty());
    }

    #[tokio::test]
    async fn deny_moves_pending_record_to_denied() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = EnrollmentStore::open(tempdir.path().join("enrollments.json"))
            .await
            .expect("open store");
        store
            .record_pending(attempt("device-1", "token-1"))
            .await
            .expect("record pending");

        let denied = store
            .deny("device-1", Some("not allowed".to_string()))
            .await
            .expect("deny");

        assert_eq!(denied.node_id, "device-1");
        assert_eq!(denied.reason.as_deref(), Some("not allowed"));

        let snapshot = store.list().await.expect("list");
        assert!(snapshot.pending.is_empty());
        assert!(snapshot.approved.is_empty());
        assert_eq!(snapshot.denied.len(), 1);
    }

    #[tokio::test]
    async fn approved_device_requires_exact_token_match() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = EnrollmentStore::open(tempdir.path().join("enrollments.json"))
            .await
            .expect("open store");
        store
            .record_pending(attempt("device-1", "token-1"))
            .await
            .expect("record pending");
        let approved = store.approve("device-1", None).await.expect("approve");

        let allowed = store
            .validate("device-1", "token-1")
            .await
            .expect("validate");
        assert_eq!(allowed, EnrollmentDecision::Approved(approved.clone()));

        let mismatch = store
            .validate("device-1", "wrong-token")
            .await
            .expect("validate mismatch");
        assert_eq!(mismatch, EnrollmentDecision::TokenMismatch(approved));
    }
}
