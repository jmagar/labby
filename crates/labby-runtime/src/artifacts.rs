//! Surface-neutral Artifact domain and local runtime support.
//!
//! This module is the open personal Artifact foundation for Labby. It owns the
//! portable ArtifactInterchange v1 contract, deterministic content addressing,
//! validation, Agent Skills projection, and the local immutable-revision store.
//! Product transports remain adapters over this layer.

pub mod agent;
pub mod canonical_json;
pub mod hook;
pub mod library;
pub mod lifecycle;
mod local_io;
pub mod materialize_skill;
pub mod model;
pub mod prompt;
pub mod provider;
pub mod skill;
pub mod store;
mod store_ops;
pub mod validation;

pub use agent::{LogicalAgentFile, MaterializedAgent, materialize_logical_agent};
pub use hook::{LogicalHookFile, MaterializedHook, materialize_logical_hook};
pub use materialize_skill::{
    LogicalSkillFile, MaterializedSkill, materialize_acquired_skill,
    materialize_acquired_skill_owned, materialize_logical_skill,
    materialize_skill_from_trusted_staging,
};

pub use library::{
    LibraryActorId, LibraryAuditIntent, LibraryAuthorization, LibraryDurableAudit, LibraryGrant,
    LibraryIdempotency, LibraryMutation, LibraryMutationOutcome, LibraryMutationReceiptFacts,
    LibraryOwnerKind, LibraryOwnership, LibraryReceipt, LibrarySnapshot, LibraryTenantId,
    LibraryTimestamp, SkillLibraryFile, SkillLibraryRecord, SkillTransactionBoundary,
    SkillVisibility, qualify_materialized_skill_owner,
};
pub use lifecycle::{
    ArtifactChangeKind, ArtifactComponentChange, ArtifactRevisionDiff, ArtifactUpdatePlan,
    ArtifactWorkspaceSnapshot, ArtifactWorkspaceSnapshotRequest,
};
pub use model::{
    ARTIFACT_INTERCHANGE_SCHEMA, ArtifactComponent, ArtifactDescriptor, ArtifactInterchange,
    ArtifactLicenseState, ArtifactLineage, ArtifactProvenance, ArtifactPublication, ArtifactRecord,
    ArtifactRevision, Distribution, ExecutionRisk, JsonMap, PublicationState, Redistribution,
    ReviewState, TakedownState, Visibility,
};
pub use prompt::{LogicalPromptFile, MaterializedPrompt, materialize_logical_prompt};
pub use provider::{
    ArtifactAcquisition, ArtifactPayloadFile, ArtifactProvider, ArtifactProviderFuture,
    ArtifactProviderRequest, LocalArtifactProvider,
};
pub use store::{ArtifactExportOptions, ArtifactForkRequest, ArtifactImportRequest, ArtifactStore};

use thiserror::Error;

/// Stable errors produced by the surface-neutral Artifact implementation.
///
/// Errors deliberately avoid embedding source bytes, credentials, or arbitrary
/// metadata values so callers can safely project them to CLI, API, or MCP.
#[derive(Debug, Error)]
pub enum ArtifactError {
    /// A field failed a bounded contract check.
    #[error("artifact field `{field}` is invalid: {reason}")]
    InvalidField {
        /// Stable field label.
        field: &'static str,
        /// Stable, non-secret reason code.
        reason: &'static str,
    },
    /// A portable schema version is unsupported.
    #[error("unsupported Artifact schema version")]
    UnsupportedSchema,
    /// A logical path failed containment or normalization rules.
    #[error("artifact path is unsafe: {0}")]
    UnsafePath(&'static str),
    /// An operation exceeded a documented safety budget.
    #[error("artifact {what} exceeds limit {limit}")]
    LimitExceeded {
        /// Stable budget label.
        what: &'static str,
        /// Maximum accepted value.
        limit: u64,
    },
    /// A record or revision could not be found.
    #[error("artifact {0} was not found")]
    NotFound(&'static str),
    /// Existing immutable state disagreed with the requested write.
    #[error("artifact conflict: {0}")]
    Conflict(&'static str),
    /// Another process currently holds the artifact mutation lock.
    #[error("artifact is busy")]
    Busy,
    /// The durable Skill Library metadata is corrupt or internally inconsistent.
    #[error("artifact Skill Library is degraded: {0}")]
    LibraryCorrupt(&'static str),
    /// Library metadata committed, but the paired Artifact promotion did not finish.
    #[error("artifact Skill Library commit {committed_version} requires reconciliation")]
    CommittedPending { committed_version: u64 },
    /// Safe-by-default export found content that resembles credential material.
    #[error("artifact export blocked because secret-like material was detected in `{path}`")]
    SecretMaterialDetected {
        /// Relative package path only. Never file contents.
        path: String,
    },
    /// Existing Agent Skills verification rejected a projected resource.
    #[error("Agent Skill resource verification failed")]
    SkillVerification,
    /// A caller-supplied logical file failed a stable package rule.
    #[error("logical Skill file `{path}` is invalid: {reason}")]
    LogicalSkillFile {
        /// Bounded logical path only; file contents are never included.
        path: String,
        /// Stable, non-secret reason code.
        reason: &'static str,
    },
    /// Local filesystem operation failed.
    #[error("artifact I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization or parsing failed.
    #[error("artifact JSON operation failed: {0}")]
    Json(#[from] serde_json::Error),
}

pub(crate) fn invalid(field: &'static str, reason: &'static str) -> ArtifactError {
    ArtifactError::InvalidField { field, reason }
}
