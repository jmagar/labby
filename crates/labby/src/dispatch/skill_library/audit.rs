//! Bounded, deduplicating Skill Library authorization audit sink.

#![allow(
    dead_code,
    reason = "shared audit sink is consumed by the Wave 2 Skill Library dispatcher"
)]

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use labby_runtime::artifacts::canonical_json;
use labby_runtime::artifacts::validation::validate_id;
use labby_runtime::artifacts::{ArtifactError, LibraryActorId, LibraryTenantId};
use serde::{Deserialize, Serialize};

use super::auth::{SkillLibraryAction, SkillLibrarySurface};

const MAX_AUDIT_EVENTS: usize = 1024;
const MAX_CORRELATION_BYTES: usize = 128;

/// Validated canonical Artifact identity. Audit retains only its bounded digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CanonicalArtifactId(String);

impl CanonicalArtifactId {
    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        validate_id(&value, "artifact_id")?;
        Ok(Self(value))
    }

    fn audit_digest(&self) -> String {
        canonical_json::sha256_bytes(self.0.as_bytes())
    }
}

/// Bounded request correlation identifier established by the transport adapter.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SkillLibraryCorrelationId(String);

impl SkillLibraryCorrelationId {
    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, ()> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_CORRELATION_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
        {
            return Err(());
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SkillLibraryAuditOutcome {
    Allow,
    Deny,
    Unavailable,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SkillLibraryAuditStage {
    Transport,
    AccessSnapshot,
    Ownership,
    Commit,
    Publication,
    Response,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SkillLibraryTerminalOutcome {
    Committed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SkillLibraryTerminalStage {
    Commit,
    Publication,
    Response,
}

impl SkillLibraryTerminalStage {
    const fn audit_stage(self) -> SkillLibraryAuditStage {
        match self {
            Self::Commit => SkillLibraryAuditStage::Commit,
            Self::Publication => SkillLibraryAuditStage::Publication,
            Self::Response => SkillLibraryAuditStage::Response,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SkillLibraryAuditKey {
    correlation_id: SkillLibraryCorrelationId,
    action: SkillLibraryAction,
    target_digest: String,
    outcome: SkillLibraryAuditOutcome,
    stage: SkillLibraryAuditStage,
    tenant_id: Option<LibraryTenantId>,
    actor_id: Option<LibraryActorId>,
    surface: SkillLibrarySurface,
    policy_revision: Option<u64>,
    revision_digest: Option<String>,
    committed_version: Option<u64>,
    published_version: Option<u64>,
    terminal_outcome: Option<SkillLibraryTerminalOutcome>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillLibraryAuditEvent {
    key: SkillLibraryAuditKey,
    pub(crate) tenant_id: Option<LibraryTenantId>,
    pub(crate) actor_id: Option<LibraryActorId>,
    pub(crate) action: SkillLibraryAction,
    target_digest: String,
    pub(crate) surface: SkillLibrarySurface,
    pub(crate) outcome: SkillLibraryAuditOutcome,
    pub(crate) stage: SkillLibraryAuditStage,
    pub(crate) policy_revision: Option<u64>,
    revision_digest: Option<String>,
    pub(crate) committed_version: Option<u64>,
    pub(crate) published_version: Option<u64>,
    pub(crate) terminal_outcome: Option<SkillLibraryTerminalOutcome>,
    pub(crate) replayed: bool,
}

impl SkillLibraryAuditEvent {
    pub(crate) fn new(
        correlation_id: SkillLibraryCorrelationId,
        target: &CanonicalArtifactId,
        action: SkillLibraryAction,
        surface: SkillLibrarySurface,
        outcome: SkillLibraryAuditOutcome,
        stage: SkillLibraryAuditStage,
    ) -> Self {
        let target_digest = target.audit_digest();
        Self {
            key: SkillLibraryAuditKey {
                correlation_id,
                action,
                target_digest: target_digest.clone(),
                outcome,
                stage,
                tenant_id: None,
                actor_id: None,
                surface,
                policy_revision: None,
                revision_digest: None,
                committed_version: None,
                published_version: None,
                terminal_outcome: None,
            },
            tenant_id: None,
            actor_id: None,
            action,
            target_digest,
            surface,
            outcome,
            stage,
            policy_revision: None,
            revision_digest: None,
            committed_version: None,
            published_version: None,
            terminal_outcome: None,
            replayed: false,
        }
    }

    pub(crate) fn with_canonical_actor(
        mut self,
        tenant_id: LibraryTenantId,
        actor_id: LibraryActorId,
        policy_revision: u64,
    ) -> Self {
        self.tenant_id = Some(tenant_id);
        self.actor_id = Some(actor_id);
        self.policy_revision = Some(policy_revision);
        self.refresh_key();
        self
    }

    /// Rebind the terminal/audit target after a create receives its owner-qualified physical id.
    pub(crate) fn with_target(mut self, target: &CanonicalArtifactId) -> Self {
        let digest = target.audit_digest();
        self.target_digest = digest.clone();
        self.key.target_digest = digest;
        self
    }

    fn refresh_key(&mut self) {
        self.key.tenant_id.clone_from(&self.tenant_id);
        self.key.actor_id.clone_from(&self.actor_id);
        self.key.surface = self.surface;
        self.key.policy_revision = self.policy_revision;
        self.key.revision_digest.clone_from(&self.revision_digest);
        self.key.committed_version = self.committed_version;
        self.key.published_version = self.published_version;
        self.key.terminal_outcome = self.terminal_outcome;
    }
}

/// Redacted terminal mutation facts attached to an authorization decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SkillLibraryTerminalAudit<'a> {
    outcome: SkillLibraryTerminalOutcome,
    stage: SkillLibraryTerminalStage,
    revision_id: Option<&'a str>,
    committed_version: Option<u64>,
    published_version: Option<u64>,
    replayed: bool,
}

/// Canonical redacted terminal event suitable for durable transaction journals.
///
/// Target and revision identities are retained only as SHA-256 digests. Paths, file content,
/// credentials, bearer material, and caller-supplied metadata are not representable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SkillLibraryDurableAudit {
    pub(crate) schema_version: u32,
    pub(crate) correlation_id: String,
    pub(crate) action: String,
    pub(crate) target_digest: String,
    pub(crate) revision_digest: Option<String>,
    pub(crate) tenant_id: LibraryTenantId,
    pub(crate) actor_id: LibraryActorId,
    pub(crate) surface: String,
    pub(crate) policy_revision: u64,
    pub(crate) committed_version: Option<u64>,
    pub(crate) published_version: Option<u64>,
    pub(crate) outcome: String,
    pub(crate) stage: String,
    pub(crate) replayed: bool,
}

impl SkillLibraryDurableAudit {
    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>, ArtifactError> {
        canonical_json::to_canonical_vec(self)
    }

    pub(crate) fn digest(&self) -> Result<String, ArtifactError> {
        canonical_json::digest(self)
    }
}

impl<'a> SkillLibraryTerminalAudit<'a> {
    pub(crate) const fn new(
        outcome: SkillLibraryTerminalOutcome,
        stage: SkillLibraryTerminalStage,
    ) -> Self {
        Self {
            outcome,
            stage,
            revision_id: None,
            committed_version: None,
            published_version: None,
            replayed: false,
        }
    }

    pub(crate) const fn with_revision_id(mut self, revision_id: &'a str) -> Self {
        self.revision_id = Some(revision_id);
        self
    }

    pub(crate) const fn with_versions(
        mut self,
        committed_version: Option<u64>,
        published_version: Option<u64>,
    ) -> Self {
        self.committed_version = committed_version;
        self.published_version = published_version;
        self
    }

    pub(crate) const fn replayed(mut self, replayed: bool) -> Self {
        self.replayed = replayed;
        self
    }
}

/// Emit one terminal mutation outcome without retaining raw targets, revisions, paths, or content.
pub(crate) fn record_terminal_mutation(
    base: &SkillLibraryAuditEvent,
    terminal: SkillLibraryTerminalAudit<'_>,
) -> bool {
    skill_library_audit_sink().record(terminal_event(base, terminal))
}

/// Project a terminal result into the durable, redacted journal vocabulary.
pub(crate) fn durable_terminal_audit(
    base: &SkillLibraryAuditEvent,
    terminal: SkillLibraryTerminalAudit<'_>,
) -> Result<SkillLibraryDurableAudit, ArtifactError> {
    let event = terminal_event(base, terminal);
    let tenant_id = event.tenant_id.clone().ok_or(ArtifactError::InvalidField {
        field: "audit",
        reason: "missing_tenant",
    })?;
    let actor_id = event.actor_id.clone().ok_or(ArtifactError::InvalidField {
        field: "audit",
        reason: "missing_actor",
    })?;
    let policy_revision = event.policy_revision.ok_or(ArtifactError::InvalidField {
        field: "audit",
        reason: "missing_policy_revision",
    })?;
    let terminal_outcome = event.terminal_outcome.ok_or(ArtifactError::InvalidField {
        field: "audit",
        reason: "missing_terminal_outcome",
    })?;
    Ok(SkillLibraryDurableAudit {
        schema_version: 1,
        correlation_id: event.key.correlation_id.as_str().to_owned(),
        action: event.action.as_str().to_owned(),
        target_digest: event.target_digest,
        revision_digest: event.revision_digest,
        tenant_id,
        actor_id,
        surface: event.surface.as_str().to_owned(),
        policy_revision,
        committed_version: event.committed_version,
        published_version: event.published_version,
        outcome: match terminal_outcome {
            SkillLibraryTerminalOutcome::Committed => "committed",
            SkillLibraryTerminalOutcome::Failed => "failed",
        }
        .to_owned(),
        stage: match event.stage {
            SkillLibraryAuditStage::Commit => "commit",
            SkillLibraryAuditStage::Publication => "publication",
            SkillLibraryAuditStage::Response => "response",
            SkillLibraryAuditStage::Transport
            | SkillLibraryAuditStage::AccessSnapshot
            | SkillLibraryAuditStage::Ownership => {
                return Err(ArtifactError::InvalidField {
                    field: "audit",
                    reason: "non_terminal_stage",
                });
            }
        }
        .to_owned(),
        replayed: event.replayed,
    })
}

fn terminal_event(
    base: &SkillLibraryAuditEvent,
    terminal: SkillLibraryTerminalAudit<'_>,
) -> SkillLibraryAuditEvent {
    let mut event = base.clone();
    event.outcome = match terminal.outcome {
        SkillLibraryTerminalOutcome::Committed => SkillLibraryAuditOutcome::Allow,
        SkillLibraryTerminalOutcome::Failed => SkillLibraryAuditOutcome::Failed,
    };
    event.stage = terminal.stage.audit_stage();
    event.revision_digest = terminal
        .revision_id
        .map(|revision| canonical_json::sha256_bytes(revision.as_bytes()));
    event.committed_version = terminal.committed_version;
    event.published_version = terminal.published_version;
    event.terminal_outcome = Some(terminal.outcome);
    event.replayed = terminal.replayed;
    event.refresh_key();
    event
}

#[derive(Default)]
struct AuditState {
    order: VecDeque<SkillLibraryAuditKey>,
    keys: HashSet<SkillLibraryAuditKey>,
    events: VecDeque<SkillLibraryAuditEvent>,
}

/// Process-shared bounded audit sink. Recording the same terminal decision is idempotent.
#[derive(Clone, Default)]
pub(crate) struct SkillLibraryAuditSink {
    state: Arc<Mutex<AuditState>>,
}

impl SkillLibraryAuditSink {
    /// Returns true only when this decision was newly retained and emitted.
    pub(crate) fn record(&self, event: SkillLibraryAuditEvent) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.keys.contains(&event.key) {
            return false;
        }
        while state.order.len() >= MAX_AUDIT_EVENTS {
            if let Some(expired) = state.order.pop_front() {
                state.keys.remove(&expired);
                state.events.pop_front();
            }
        }
        state.keys.insert(event.key.clone());
        state.order.push_back(event.key.clone());
        state.events.push_back(event.clone());
        drop(state);
        tracing::info!(
            correlation_id = event.key.correlation_id.as_str(),
            action = event.action.as_str(),
            target_digest = event.target_digest,
            surface = event.surface.as_str(),
            outcome = ?event.outcome,
            stage = ?event.stage,
            policy_revision = event.policy_revision,
            revision_digest = event.revision_digest,
            committed_version = event.committed_version,
            published_version = event.published_version,
            terminal_outcome = ?event.terminal_outcome,
            replayed = event.replayed,
            "skill library authorization decision"
        );
        true
    }

    #[cfg(test)]
    fn events(&self) -> Vec<SkillLibraryAuditEvent> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .events
            .iter()
            .cloned()
            .collect()
    }
}

pub(super) fn skill_library_audit_sink() -> &'static SkillLibraryAuditSink {
    static SINK: OnceLock<SkillLibraryAuditSink> = OnceLock::new();
    SINK.get_or_init(SkillLibraryAuditSink::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_actor(tenant: &str, actor: &str) -> (LibraryTenantId, LibraryActorId) {
        (
            LibraryTenantId::from_canonical_projection(tenant).unwrap(),
            LibraryActorId::from_canonical_projection(actor).unwrap(),
        )
    }

    #[test]
    fn real_sink_deduplicates_and_retains_correlation_without_raw_target() {
        let sink = SkillLibraryAuditSink::default();
        let target = CanonicalArtifactId::parse("secret-canary").unwrap();
        let correlation = SkillLibraryCorrelationId::parse("request-1").unwrap();
        let event = SkillLibraryAuditEvent::new(
            correlation,
            &target,
            SkillLibraryAction::Activate,
            SkillLibrarySurface::Mcp,
            SkillLibraryAuditOutcome::Allow,
            SkillLibraryAuditStage::Ownership,
        );
        assert!(sink.record(event.clone()));
        assert!(!sink.record(event));
        assert_eq!(sink.events().len(), 1);
        let debug = format!("{:?}", sink.events());
        assert!(debug.contains("request-1"));
        assert!(!debug.contains("secret-canary"));
    }

    #[test]
    fn invalid_newline_and_oversized_identifiers_never_reach_the_sink() {
        assert!(CanonicalArtifactId::parse("secret\ncanary").is_err());
        assert!(CanonicalArtifactId::parse("x".repeat(1024)).is_err());
        assert!(SkillLibraryCorrelationId::parse("request\nforged").is_err());
        assert!(SkillLibraryCorrelationId::parse("x".repeat(129)).is_err());
    }

    #[test]
    fn same_client_correlation_keeps_distinct_canonical_actors() {
        let sink = SkillLibraryAuditSink::default();
        let target = CanonicalArtifactId::parse("private-skill").unwrap();
        let correlation = SkillLibraryCorrelationId::parse("client-request").unwrap();
        let base = SkillLibraryAuditEvent::new(
            correlation,
            &target,
            SkillLibraryAction::Read,
            SkillLibrarySurface::ApiBearer,
            SkillLibraryAuditOutcome::Allow,
            SkillLibraryAuditStage::Ownership,
        );
        let (tenant, actor_a) = canonical_actor("tenant-a", "actor-a");
        let (_, actor_b) = canonical_actor("tenant-a", "actor-b");
        assert!(
            sink.record(
                base.clone()
                    .with_canonical_actor(tenant.clone(), actor_a, 7,)
            )
        );
        assert!(sink.record(base.with_canonical_actor(tenant, actor_b, 7)));
        assert_eq!(sink.events().len(), 2);
    }

    #[test]
    fn terminal_exact_retry_dedups_but_failed_and_committed_are_distinct() {
        let target = CanonicalArtifactId::parse("private-skill").unwrap();
        let (tenant, actor) = canonical_actor("tenant-a", "actor-a");
        let base = SkillLibraryAuditEvent::new(
            SkillLibraryCorrelationId::parse("request-terminal").unwrap(),
            &target,
            SkillLibraryAction::Save,
            SkillLibrarySurface::Mcp,
            SkillLibraryAuditOutcome::Allow,
            SkillLibraryAuditStage::Ownership,
        )
        .with_canonical_actor(tenant, actor, 9);
        let sink = SkillLibraryAuditSink::default();
        let failed = terminal_event(
            &base,
            SkillLibraryTerminalAudit::new(
                SkillLibraryTerminalOutcome::Failed,
                SkillLibraryTerminalStage::Commit,
            )
            .with_revision_id("revision-secret"),
        );
        assert!(sink.record(failed.clone()));
        assert!(!sink.record(failed));

        let committed = terminal_event(
            &base,
            SkillLibraryTerminalAudit::new(
                SkillLibraryTerminalOutcome::Committed,
                SkillLibraryTerminalStage::Response,
            )
            .with_revision_id("revision-secret")
            .with_versions(Some(4), Some(4))
            .replayed(true),
        );
        assert!(sink.record(committed));
        let replay = terminal_event(
            &base,
            SkillLibraryTerminalAudit::new(
                SkillLibraryTerminalOutcome::Committed,
                SkillLibraryTerminalStage::Response,
            )
            .with_revision_id("revision-secret")
            .with_versions(Some(4), Some(4))
            .replayed(false),
        );
        assert!(!sink.record(replay));
        assert_eq!(sink.events().len(), 2);
        let retained = format!("{:?}", sink.events());
        assert!(!retained.contains("private-skill"));
        assert!(!retained.contains("revision-secret"));
    }

    #[test]
    fn durable_terminal_round_trip_is_redacted_and_actor_bound() {
        let target = CanonicalArtifactId::parse("secret-private-skill").unwrap();
        let (tenant, actor_a) = canonical_actor("tenant-a", "actor-a");
        let (_, actor_b) = canonical_actor("tenant-a", "actor-b");
        let base = SkillLibraryAuditEvent::new(
            SkillLibraryCorrelationId::parse("request-durable").unwrap(),
            &target,
            SkillLibraryAction::Save,
            SkillLibrarySurface::Mcp,
            SkillLibraryAuditOutcome::Allow,
            SkillLibraryAuditStage::Ownership,
        );
        let terminal = SkillLibraryTerminalAudit::new(
            SkillLibraryTerminalOutcome::Failed,
            SkillLibraryTerminalStage::Response,
        )
        .with_revision_id("secret-revision")
        .with_versions(Some(11), Some(11));
        let durable_a = durable_terminal_audit(
            &base
                .clone()
                .with_canonical_actor(tenant.clone(), actor_a, 23),
            terminal,
        )
        .unwrap();
        let bytes = durable_a.canonical_bytes().unwrap();
        let wire = String::from_utf8(bytes.clone()).unwrap();
        assert!(!wire.contains("secret-private-skill"));
        assert!(!wire.contains("secret-revision"));
        let reopened: SkillLibraryDurableAudit = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reopened, durable_a);
        assert_eq!(reopened.stage, "response");
        assert_eq!(reopened.outcome, "failed");
        assert_eq!(reopened.committed_version, Some(11));
        assert_eq!(reopened.published_version, Some(11));

        let durable_b =
            durable_terminal_audit(&base.with_canonical_actor(tenant, actor_b, 23), terminal)
                .unwrap();
        assert_ne!(durable_a.digest().unwrap(), durable_b.digest().unwrap());
    }
}
