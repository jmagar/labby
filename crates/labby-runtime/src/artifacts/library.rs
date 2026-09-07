//! Durable, surface-neutral Skill Library authority.
//!
//! The Artifact store remains authoritative for immutable bytes and authored heads. This
//! module persists only Labby-local ownership and lifecycle state. Identity values are opaque
//! projections: callers must derive them from the canonical access-control records.

#![allow(
    dead_code,
    reason = "the sealed mutation primitive is wired to the canonical access adapter in a later bead"
)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::canonical_json;
use super::local_io::{
    SnapshotFile, read_json, sync_directory, write_bytes_atomic_with_faults,
    write_json_atomic_with_faults,
};
use super::model::{ArtifactRecord, ArtifactRevision};
use super::validation::{validate_id, validate_reference_id};
use super::{ArtifactError, ArtifactStore, MaterializedSkill, invalid};

pub const LIBRARY_SCHEMA_VERSION: u8 = 1;
pub const OWNERSHIP_PROJECTION_SCHEMA_VERSION: u8 = 1;
const MAX_ID_BYTES: usize = 256;
const MAX_RECEIPTS: usize = 1024;
const MAX_AUDIT_INTENTS: usize = 1024;
const MAX_TIMESTAMP_BYTES: usize = 64;
pub(crate) const MAX_LIBRARY_STATE_BYTES: u64 = 4 * 1024 * 1024;
// A full 64 MiB Skill package expands to just under 86 MiB in padded base64. Keep enough
// bounded headroom for the manifest and transaction metadata without returning to serde's
// much larger JSON numeric-array representation for `Vec<u8>`.
const MAX_PENDING_SKILL_TRANSACTION_BYTES: u64 = 96 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingSkillTransaction {
    schema_version: u8,
    scope_digest: String,
    request_digest: String,
    expected_library_version: u64,
    tenant_id: LibraryTenantId,
    actor_id: LibraryActorId,
    idempotency_key: String,
    artifact_id: String,
    revision_id: String,
    revision: ArtifactRevision,
    prior_record: Option<ArtifactRecord>,
    next_record: ArtifactRecord,
    files: Vec<PendingSkillFile>,
    transaction_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingSkillFile {
    path: String,
    #[serde(with = "base64_bytes")]
    bytes: Vec<u8>,
}

mod base64_bytes {
    use std::fmt;

    use base64::Engine as _;
    use serde::de::{SeqAccess, Visitor};
    use serde::{Deserializer, Serializer};

    pub(super) fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Base64OrLegacyBytes;

        impl<'de> Visitor<'de> for Base64OrLegacyBytes {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("base64 text or a legacy byte array")
            }

            fn visit_str<E>(self, encoded: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(E::custom)
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut bytes = Vec::with_capacity(
                    sequence
                        .size_hint()
                        .unwrap_or(0)
                        .min(super::super::validation::MAX_SKILL_PACKAGE_BYTES),
                );
                while let Some(byte) = sequence.next_element::<u8>()? {
                    if bytes.len() == super::super::validation::MAX_SKILL_PACKAGE_BYTES {
                        return Err(serde::de::Error::custom("legacy byte array exceeds cap"));
                    }
                    bytes.push(byte);
                }
                Ok(bytes)
            }
        }

        deserializer.deserialize_any(Base64OrLegacyBytes)
    }
}

impl PendingSkillTransaction {
    fn compute_digest(&self) -> Result<String, ArtifactError> {
        let mut payload = self.clone();
        payload.transaction_digest.clear();
        if payload.schema_version == 1 {
            // Version 1 journals used serde's default `Vec<u8>` representation. Preserve that
            // exact canonical payload when verifying an intent written by an older Labby.
            let mut legacy = serde_json::to_value(&payload)?;
            let files = legacy
                .get_mut("files")
                .and_then(serde_json::Value::as_array_mut)
                .ok_or(ArtifactError::LibraryCorrupt(
                    "invalid_pending_skill_transaction",
                ))?;
            for (file, source) in files.iter_mut().zip(&payload.files) {
                file.as_object_mut()
                    .and_then(|object| {
                        object.insert(
                            "bytes".to_owned(),
                            serde_json::Value::Array(
                                source
                                    .bytes
                                    .iter()
                                    .copied()
                                    .map(serde_json::Value::from)
                                    .collect(),
                            ),
                        )
                    })
                    .ok_or(ArtifactError::LibraryCorrupt(
                        "invalid_pending_skill_transaction",
                    ))?;
            }
            canonical_json::digest(&legacy)
        } else {
            canonical_json::digest(&payload)
        }
    }

    fn validate(&self) -> Result<(), ArtifactError> {
        if !matches!(self.schema_version, 1 | 2)
            || self.transaction_digest != self.compute_digest()?
            || self.revision.id != self.revision_id
            || self.next_record.descriptor.id != self.artifact_id
            || self.next_record.current_revision_id != self.revision_id
            || self
                .prior_record
                .as_ref()
                .is_some_and(|record| record.descriptor.id != self.artifact_id)
        {
            return Err(ArtifactError::LibraryCorrupt(
                "invalid_pending_skill_transaction",
            ));
        }
        validate_id(&self.artifact_id, "artifact_id")?;
        validate_reference_id(&self.revision_id, "revision_id")?;
        self.revision.verify_content_digest()?;
        self.next_record.validate()?;
        if let Some(prior) = &self.prior_record {
            prior.validate()?;
        }
        let total_bytes = self.files.iter().try_fold(0usize, |total, file| {
            total
                .checked_add(file.bytes.len())
                .ok_or(ArtifactError::LibraryCorrupt(
                    "pending_skill_payload_too_large",
                ))
        })?;
        if total_bytes > super::validation::MAX_SKILL_PACKAGE_BYTES {
            return Err(ArtifactError::LibraryCorrupt(
                "pending_skill_payload_too_large",
            ));
        }
        let components = self
            .revision
            .components
            .iter()
            .map(|component| (&component.path, &component.digest))
            .collect::<BTreeMap<_, _>>();
        let file_paths = self
            .files
            .iter()
            .map(|file| &file.path)
            .collect::<std::collections::BTreeSet<_>>();
        if components.len() != self.revision.components.len()
            || file_paths.len() != self.files.len()
            || components
                .keys()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                != file_paths
            || self.files.iter().any(|file| {
                components
                    .get(&file.path)
                    .is_none_or(|expected| **expected != canonical_json::sha256_bytes(&file.bytes))
            })
        {
            return Err(ArtifactError::LibraryCorrupt(
                "pending_skill_payload_mismatch",
            ));
        }
        Ok(())
    }
}

macro_rules! opaque_projection_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Construct from an accepted canonical access-control identifier projection.
            /// Construct an identifier from the canonical access-runtime projection.
            ///
            /// Product adapters must obtain this value from the accepted AccessRuntime record;
            /// client input, identity-provider claims, email, and display names are not canonical.
            pub fn from_canonical_projection(
                value: impl Into<String>,
            ) -> Result<Self, ArtifactError> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > MAX_ID_BYTES
                    || value.trim() != value
                    || value.chars().any(char::is_control)
                {
                    return Err(invalid($field, "invalid_canonical_projection"));
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

opaque_projection_id!(LibraryTenantId, "tenant_id");
opaque_projection_id!(LibraryActorId, "actor_id");

/// Durable owner namespace. Missing values in v1 records migrate as personal.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryOwnerKind {
    #[default]
    Personal,
    Team,
    Project,
}

impl LibraryOwnerKind {
    fn is_personal(value: &Self) -> bool {
        *value == Self::Personal
    }
}

/// Canonical local ownership projection. It deliberately contains no auth-provider facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryOwnership {
    pub schema_version: u8,
    pub tenant_id: LibraryTenantId,
    pub owner_id: LibraryActorId,
    #[serde(default, skip_serializing_if = "LibraryOwnerKind::is_personal")]
    pub owner_kind: LibraryOwnerKind,
}

impl LibraryOwnership {
    pub fn canonical(tenant_id: LibraryTenantId, owner_id: LibraryActorId) -> Self {
        Self {
            schema_version: OWNERSHIP_PROJECTION_SCHEMA_VERSION,
            tenant_id,
            owner_id,
            owner_kind: LibraryOwnerKind::Personal,
        }
    }

    pub fn scoped(
        tenant_id: LibraryTenantId,
        owner_kind: LibraryOwnerKind,
        owner_id: LibraryActorId,
    ) -> Self {
        Self {
            schema_version: OWNERSHIP_PROJECTION_SCHEMA_VERSION,
            tenant_id,
            owner_id,
            owner_kind,
        }
    }

    #[must_use]
    pub const fn owner_kind(&self) -> LibraryOwnerKind {
        self.owner_kind
    }

    fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != OWNERSHIP_PROJECTION_SCHEMA_VERSION {
            return Err(ArtifactError::UnsupportedSchema);
        }
        LibraryTenantId::from_canonical_projection(self.tenant_id.0.clone())?;
        LibraryActorId::from_canonical_projection(self.owner_id.0.clone())?;
        Ok(())
    }
}

/// Bind a newly created local Skill's physical Artifact identity to its durable owner namespace.
///
/// The human name and canonical Skill URI remain stable, while the opaque Artifact id (and thus
/// every ArtifactStore lock, head, revision, workspace and CAS path) becomes owner-qualified.
/// Existing v1 personal records keep their original ids; this function is only for new creates.
pub fn qualify_materialized_skill_owner(
    materialized: &mut MaterializedSkill,
    ownership: &LibraryOwnership,
) -> Result<(), ArtifactError> {
    ownership.validate()?;
    let descriptor = &materialized.interchange.descriptor;
    let source_identity = canonical_json::to_canonical_vec(&(
        "labby.library.owner/v1",
        ownership,
        &descriptor.kind,
        &descriptor.namespace,
        &descriptor.name,
    ))?;
    let source_identity = String::from_utf8(source_identity)
        .map_err(|_| ArtifactError::Conflict("owner_identity_encoding"))?;
    materialized.interchange.descriptor.id = super::model::ArtifactDescriptor::for_source_identity(
        &descriptor.kind,
        &descriptor.namespace,
        &descriptor.name,
        &source_identity,
    )?
    .id;
    materialized.interchange.validate()?;
    Ok(())
}

/// Authorization decision already made by the canonical product access layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryGrant {
    Owner,
    Admin,
}

/// Canonical request actor plus its already-authorized mutation grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryAuthorization {
    schema_version: u8,
    tenant_id: LibraryTenantId,
    actor_id: LibraryActorId,
    grant: LibraryGrant,
    owner_kind: LibraryOwnerKind,
    scope_id: LibraryActorId,
}

impl LibraryAuthorization {
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "constructed by the canonical access adapter in a later bead"
        )
    )]
    /// Construct the sealed authority projection after canonical access authorization.
    ///
    /// This is a dependency-safe seam: `labby-runtime` cannot depend upward on the product's
    /// AccessRuntime. Only the canonical AccessRuntime adapter may call this constructor, and it
    /// must do so immediately after resolving current membership and authorizing this exact
    /// mutation. Transport claims and caller-supplied owner, tenant, role, or grant values must
    /// never reach this constructor.
    pub fn from_authorized_access_projection(
        tenant_id: LibraryTenantId,
        actor_id: LibraryActorId,
        grant: LibraryGrant,
    ) -> Self {
        let scope_id = actor_id.clone();
        Self {
            schema_version: OWNERSHIP_PROJECTION_SCHEMA_VERSION,
            tenant_id,
            actor_id,
            grant,
            owner_kind: LibraryOwnerKind::Personal,
            scope_id,
        }
    }

    pub fn from_authorized_scope_projection(
        tenant_id: LibraryTenantId,
        actor_id: LibraryActorId,
        owner_kind: LibraryOwnerKind,
        scope_id: LibraryActorId,
    ) -> Self {
        Self {
            schema_version: OWNERSHIP_PROJECTION_SCHEMA_VERSION,
            tenant_id,
            actor_id,
            grant: LibraryGrant::Owner,
            owner_kind,
            scope_id,
        }
    }

    fn validate_for(&self, ownership: &LibraryOwnership) -> Result<(), ArtifactError> {
        if self.schema_version != OWNERSHIP_PROJECTION_SCHEMA_VERSION {
            return Err(ArtifactError::UnsupportedSchema);
        }
        LibraryTenantId::from_canonical_projection(self.tenant_id.0.clone())?;
        LibraryActorId::from_canonical_projection(self.actor_id.0.clone())?;
        if self.tenant_id != ownership.tenant_id {
            return Err(ArtifactError::NotFound("library_record"));
        }
        let owner_kind_matches = self.owner_kind == ownership.owner_kind;
        let owner_id_matches = self.scope_id == ownership.owner_id;
        let owner_scope_matches = owner_kind_matches && owner_id_matches;
        if self.grant == LibraryGrant::Owner && !owner_scope_matches {
            return Err(ArtifactError::NotFound("library_record"));
        }
        Ok(())
    }
}

/// Bounded canonical instant used by durable library metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LibraryTimestamp(String);

impl LibraryTimestamp {
    pub fn parse(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        let parsed = value
            .parse::<jiff::Timestamp>()
            .map_err(|_| invalid("timestamp", "invalid_timestamp"))?;
        let canonical = parsed.to_string();
        if canonical.len() > MAX_TIMESTAMP_BYTES {
            return Err(invalid("timestamp", "invalid_timestamp"));
        }
        Ok(Self(canonical))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillVisibility {
    Private,
    Tenant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillLibraryFile {
    pub path: String,
    pub digest: String,
    pub size: u64,
    pub media_type: Option<String>,
}

/// One Labby-local Skill record. Revision bytes remain owned by [`ArtifactStore`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillLibraryRecord {
    pub artifact_id: String,
    pub name: String,
    pub ownership: LibraryOwnership,
    pub visibility: SkillVisibility,
    pub archived: bool,
    pub active_revision_id: Option<String>,
    #[serde(default)]
    pub latest_revision_id: String,
    #[serde(default)]
    pub latest_revision_files: Vec<SkillLibraryFile>,
    /// Descriptor metadata normalized into the durable snapshot search index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_metadata: Vec<String>,
    #[serde(default)]
    pub provenance_provider: Option<String>,
    #[serde(default)]
    pub materialized: bool,
    pub created_at: LibraryTimestamp,
    pub updated_at: LibraryTimestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacySkillLibraryRecord {
    artifact_id: String,
    name: String,
    ownership: LibraryOwnership,
    visibility: SkillVisibility,
    archived: bool,
    active_revision_id: Option<String>,
    created_at: LibraryTimestamp,
    updated_at: LibraryTimestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyLibrarySnapshot {
    schema_version: u8,
    version: u64,
    active_generation_digest: String,
    records: BTreeMap<String, LegacySkillLibraryRecord>,
    active_names: BTreeMap<String, String>,
    receipts: BTreeMap<String, LibraryReceipt>,
    audit_intents: Vec<LibraryAuditIntent>,
}

impl LegacyLibrarySnapshot {
    fn compute_digest(&self) -> Result<String, ArtifactError> {
        #[derive(Serialize)]
        struct LegacyGeneration<'a> {
            schema_version: u8,
            version: u64,
            records: &'a BTreeMap<String, LegacySkillLibraryRecord>,
            active_names: &'a BTreeMap<String, String>,
            receipts: &'a BTreeMap<String, LibraryReceipt>,
            audit_intents: &'a [LibraryAuditIntent],
        }
        let receipts = self
            .receipts
            .iter()
            .map(|(key, receipt)| {
                let mut receipt = receipt.clone();
                receipt.response_facts = None;
                (key.clone(), receipt)
            })
            .collect::<BTreeMap<_, _>>();
        canonical_json::digest(&LegacyGeneration {
            schema_version: self.schema_version,
            version: self.version,
            records: &self.records,
            active_names: &self.active_names,
            receipts: &receipts,
            audit_intents: &self.audit_intents,
        })
    }

    fn verify_digest(&self) -> Result<(), ArtifactError> {
        if self.compute_digest()? != self.active_generation_digest {
            return Err(ArtifactError::LibraryCorrupt("generation_digest_mismatch"));
        }
        Ok(())
    }

    fn into_current(self) -> LibrarySnapshot {
        LibrarySnapshot {
            schema_version: self.schema_version,
            version: self.version,
            active_generation_digest: self.active_generation_digest,
            records: self
                .records
                .into_iter()
                .map(|(id, record)| {
                    (
                        id,
                        SkillLibraryRecord {
                            artifact_id: record.artifact_id,
                            name: record.name,
                            ownership: record.ownership,
                            visibility: record.visibility,
                            archived: record.archived,
                            active_revision_id: record.active_revision_id,
                            latest_revision_id: String::new(),
                            latest_revision_files: Vec::new(),
                            search_metadata: Vec::new(),
                            provenance_provider: None,
                            materialized: false,
                            created_at: record.created_at,
                            updated_at: record.updated_at,
                        },
                    )
                })
                .collect(),
            active_names: self.active_names,
            receipts: self.receipts,
            audit_intents: self.audit_intents,
        }
    }
}

impl SkillLibraryRecord {
    fn validate_metadata(&self) -> Result<(), ArtifactError> {
        validate_id(&self.artifact_id, "artifact_id")?;
        validate_skill_name(&self.name)?;
        self.ownership.validate()?;
        if LibraryTimestamp::parse(self.created_at.0.clone())? != self.created_at
            || LibraryTimestamp::parse(self.updated_at.0.clone())? != self.updated_at
        {
            return Err(invalid("timestamp", "not_canonical"));
        }
        if self.archived && self.active_revision_id.is_some() {
            return Err(ArtifactError::LibraryCorrupt("archived_active_record"));
        }
        if let Some(revision) = &self.active_revision_id {
            validate_reference_id(revision, "active_revision_id")?;
        }
        if !self.latest_revision_id.is_empty() {
            validate_reference_id(&self.latest_revision_id, "latest_revision_id")?;
        }
        Ok(())
    }

    fn validate(&self, store: &ArtifactStore) -> Result<(), ArtifactError> {
        self.validate_metadata()?;
        let artifact = store.get(&self.artifact_id)?;
        if artifact.descriptor.kind != "skill" {
            return Err(ArtifactError::Conflict("library_artifact_not_skill"));
        }
        if artifact.descriptor.name != self.name {
            return Err(ArtifactError::Conflict("library_name_mismatch"));
        }
        if let Some(revision) = &self.active_revision_id {
            store.revision(&self.artifact_id, revision)?;
        }
        Ok(())
    }
}

/// Security-relevant request binding retained with a terminal receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryIdempotency {
    pub key: String,
    pub request_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_audit: Option<LibraryDurableAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryAuditIntent {
    pub sequence: u64,
    pub action: String,
    pub tenant_id: LibraryTenantId,
    pub actor_id: LibraryActorId,
    /// Exact owner namespace affected by this commit. Legacy v1 entries omit it and are
    /// reconciled from the referenced record while loading the snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<LibraryOwnership>,
    pub artifact_id: String,
    pub request_digest: String,
    pub committed_at: LibraryTimestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_audit: Option<LibraryDurableAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryReceipt {
    pub sequence: u64,
    pub scope_digest: String,
    pub tenant_id: LibraryTenantId,
    pub actor_id: LibraryActorId,
    /// Exact owner namespace bound into the idempotency scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<LibraryOwnership>,
    pub action: String,
    pub artifact_id: String,
    pub idempotency_key: String,
    pub request_digest: String,
    pub committed_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_facts: Option<LibraryMutationReceiptFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_audit: Option<LibraryDurableAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryMutationReceiptFacts {
    pub active_revision_id: Option<String>,
    pub canonical_uri: Option<String>,
    pub old_generation: u64,
    pub new_generation: u64,
    pub committed_library_version: u64,
    pub library_digest: String,
    pub relist_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryDurableAudit {
    pub schema_version: u32,
    pub correlation_id: String,
    pub action: String,
    pub target_digest: String,
    pub revision_digest: Option<String>,
    pub tenant_id: LibraryTenantId,
    pub actor_id: LibraryActorId,
    pub surface: String,
    pub policy_revision: u64,
    pub committed_version: Option<u64>,
    pub published_version: Option<u64>,
    pub outcome: String,
    pub stage: String,
    pub replayed: bool,
}

impl LibraryDurableAudit {
    fn validate_for(&self, receipt: &LibraryReceipt) -> Result<(), ArtifactError> {
        if self.schema_version != 1
            || self.correlation_id.is_empty()
            || self.correlation_id.len() > 256
            || self.correlation_id.chars().any(char::is_control)
            || !terminal_audit_action_matches(&self.action, &receipt.action)
            || self.tenant_id != receipt.tenant_id
            || self.actor_id != receipt.actor_id
            || self.committed_version != Some(receipt.committed_version)
            || !matches!(
                self.surface.as_str(),
                "api" | "cli" | "mcp" | "resources/read"
            )
            || !matches!(self.outcome.as_str(), "committed" | "failed")
            || !matches!(self.stage.as_str(), "commit" | "publication" | "response")
        {
            return Err(ArtifactError::LibraryCorrupt("invalid_terminal_audit"));
        }
        validate_digest(&self.target_digest)
            .map_err(|_| ArtifactError::LibraryCorrupt("invalid_terminal_audit"))?;
        if let Some(digest) = &self.revision_digest {
            validate_digest(digest)
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_terminal_audit"))?;
        }
        Ok(())
    }
}

fn terminal_audit_action_matches(audit_action: &str, receipt_action: &str) -> bool {
    ["artifacts.", "skill_library."]
        .into_iter()
        .filter_map(|prefix| audit_action.strip_prefix(prefix))
        .any(|product| {
            product == receipt_action || (product == "import" && receipt_action == "create")
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryMutationOutcome {
    Committed(LibraryReceipt, LibraryMutationSeal),
    Replayed(LibraryReceipt, LibraryMutationSeal),
}

/// Opaque proof that a mutation outcome was produced by [`ArtifactStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryMutationSeal {
    receipt_digest: String,
}

impl LibraryMutationSeal {
    fn for_receipt(receipt: &LibraryReceipt) -> Result<Self, ArtifactError> {
        Ok(Self {
            receipt_digest: canonical_json::digest(receipt)?,
        })
    }

    fn validate(&self, receipt: &LibraryReceipt) -> Result<(), ArtifactError> {
        if self.receipt_digest != canonical_json::digest(receipt)? {
            return Err(ArtifactError::Conflict("invalid_mutation_outcome_seal"));
        }
        Ok(())
    }
}

/// Durable boundaries in the authored-bytes plus Library metadata transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillTransactionBoundary {
    IntentWrite,
    IntentFileSync,
    IntentRename,
    IntentParentSync,
    LibraryWrite,
    LibraryFileSync,
    LibraryRename,
    LibraryParentSync,
    PromotionWrite,
    PromotionFileSync,
    PromotionRename,
    PromotionParentSync,
    AppliedWrite,
    AppliedFileSync,
    AppliedRename,
    AppliedParentSync,
}

impl LibraryMutationOutcome {
    #[must_use]
    pub fn receipt(&self) -> &LibraryReceipt {
        match self {
            Self::Committed(receipt, _) | Self::Replayed(receipt, _) => receipt,
        }
    }

    #[must_use]
    pub const fn is_replay(&self) -> bool {
        matches!(self, Self::Replayed(..))
    }

    fn seal(&self) -> &LibraryMutationSeal {
        match self {
            Self::Committed(_, seal) | Self::Replayed(_, seal) => seal,
        }
    }
}

/// Complete committed library generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibrarySnapshot {
    pub schema_version: u8,
    pub version: u64,
    pub active_generation_digest: String,
    pub records: BTreeMap<String, SkillLibraryRecord>,
    pub active_names: BTreeMap<String, String>,
    pub receipts: BTreeMap<String, LibraryReceipt>,
    pub audit_intents: Vec<LibraryAuditIntent>,
}

impl Default for LibrarySnapshot {
    fn default() -> Self {
        let mut state = Self {
            schema_version: LIBRARY_SCHEMA_VERSION,
            version: 0,
            active_generation_digest: String::new(),
            records: BTreeMap::new(),
            active_names: BTreeMap::new(),
            receipts: BTreeMap::new(),
            audit_intents: Vec::new(),
        };
        state.active_generation_digest = state
            .compute_digest()
            .expect("empty generation is serializable");
        state
    }
}

impl LibrarySnapshot {
    fn active_name_key(ownership: &LibraryOwnership, name: &str) -> Result<String, ArtifactError> {
        canonical_json::digest(&(ownership, name))
    }

    fn compute_digest(&self) -> Result<String, ArtifactError> {
        #[derive(Serialize)]
        struct Generation<'a> {
            schema_version: u8,
            version: u64,
            records: &'a BTreeMap<String, SkillLibraryRecord>,
            active_names: &'a BTreeMap<String, String>,
            receipts: &'a BTreeMap<String, LibraryReceipt>,
            audit_intents: &'a [LibraryAuditIntent],
        }
        let receipts = self
            .receipts
            .iter()
            .map(|(key, receipt)| {
                let mut receipt = receipt.clone();
                receipt.response_facts = None;
                (key.clone(), receipt)
            })
            .collect::<BTreeMap<_, _>>();
        canonical_json::digest(&Generation {
            schema_version: self.schema_version,
            version: self.version,
            records: &self.records,
            active_names: &self.active_names,
            receipts: &receipts,
            audit_intents: &self.audit_intents,
        })
    }

    pub(crate) fn validate_metadata(&self) -> Result<(), ArtifactError> {
        if self.schema_version != LIBRARY_SCHEMA_VERSION {
            return Err(ArtifactError::UnsupportedSchema);
        }
        if self.compute_digest()? != self.active_generation_digest {
            return Err(ArtifactError::LibraryCorrupt("generation_digest_mismatch"));
        }
        for (id, record) in &self.records {
            if id != &record.artifact_id {
                return Err(ArtifactError::LibraryCorrupt("record_key_mismatch"));
            }
            record.validate_metadata()?;
        }
        let mut expected = BTreeMap::new();
        for record in self
            .records
            .values()
            .filter(|record| record.active_revision_id.is_some())
        {
            if expected
                .insert(
                    Self::active_name_key(&record.ownership, &record.name)?,
                    record.artifact_id.clone(),
                )
                .is_some()
            {
                return Err(ArtifactError::LibraryCorrupt("duplicate_active_name"));
            }
        }
        let legacy_active_names = self
            .records
            .values()
            .filter(|record| record.active_revision_id.is_some())
            .map(|record| (record.name.clone(), record.artifact_id.clone()))
            .collect::<BTreeMap<_, _>>();
        if expected != self.active_names && legacy_active_names != self.active_names {
            return Err(ArtifactError::LibraryCorrupt("active_index_mismatch"));
        }
        if self.receipts.len() > MAX_RECEIPTS {
            return Err(ArtifactError::LibraryCorrupt("receipt_limit"));
        }
        if self.audit_intents.len() > MAX_AUDIT_INTENTS {
            return Err(ArtifactError::LibraryCorrupt("audit_limit"));
        }
        let mut receipt_sequences = std::collections::BTreeSet::new();
        for (key, receipt) in &self.receipts {
            validate_digest(key).map_err(|_| ArtifactError::LibraryCorrupt("receipt_key"))?;
            if key != &receipt.scope_digest
                || receipt.sequence == 0
                || !receipt_sequences.insert(receipt.sequence)
                || receipt.committed_version != receipt.sequence
                || receipt.committed_version > self.version
            {
                return Err(ArtifactError::LibraryCorrupt("invalid_receipt"));
            }
            validate_digest(&receipt.request_digest)
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_receipt_digest"))?;
            if let Some(digest) = &receipt.transaction_digest {
                validate_digest(digest)
                    .map_err(|_| ArtifactError::LibraryCorrupt("invalid_transaction_digest"))?;
            }
            LibraryTenantId::from_canonical_projection(receipt.tenant_id.0.clone())
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_receipt_actor"))?;
            LibraryActorId::from_canonical_projection(receipt.actor_id.0.clone())
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_receipt_actor"))?;
            validate_action(&receipt.action)
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_receipt_action"))?;
            validate_id(&receipt.artifact_id, "artifact_id")
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_receipt_reference"))?;
            let is_refresh = receipt.action == "refresh" && receipt.artifact_id == "library";
            let receipt_record = (!is_refresh)
                .then(|| {
                    self.records.values().find(|record| {
                        record.artifact_id == receipt.artifact_id
                            && receipt
                                .ownership
                                .as_ref()
                                .is_none_or(|owner| owner == &record.ownership)
                    })
                })
                .flatten();
            if !is_refresh && receipt_record.is_none() {
                return Err(ArtifactError::LibraryCorrupt("invalid_receipt_reference"));
            }
            if !is_refresh
                && receipt_record
                    .is_none_or(|record| record.ownership.tenant_id != receipt.tenant_id)
            {
                return Err(ArtifactError::LibraryCorrupt("receipt_tenant_mismatch"));
            }
            if receipt.idempotency_key.is_empty() || receipt.idempotency_key.len() > 256 {
                return Err(ArtifactError::LibraryCorrupt("invalid_receipt_key"));
            }
            let expected_scope = receipt_scope_digest(
                &receipt.tenant_id,
                &receipt.actor_id,
                receipt.ownership.as_ref(),
                &receipt.action,
                &receipt.artifact_id,
                &receipt.idempotency_key,
            )?;
            if expected_scope != *key {
                return Err(ArtifactError::LibraryCorrupt("receipt_scope_mismatch"));
            }
            if let Some(audit) = &receipt.terminal_audit {
                audit.validate_for(receipt)?;
            }
        }
        let mut previous = 0;
        for audit in &self.audit_intents {
            if audit.sequence == 0 || audit.sequence <= previous || audit.sequence > self.version {
                return Err(ArtifactError::LibraryCorrupt("invalid_audit_sequence"));
            }
            previous = audit.sequence;
            validate_id(&audit.artifact_id, "artifact_id")
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_reference"))?;
            let is_refresh = audit.action == "refresh" && audit.artifact_id == "library";
            let audit_record = (!is_refresh)
                .then(|| {
                    self.records.values().find(|record| {
                        record.artifact_id == audit.artifact_id
                            && audit
                                .ownership
                                .as_ref()
                                .is_none_or(|owner| owner == &record.ownership)
                    })
                })
                .flatten();
            if !is_refresh && audit_record.is_none() {
                return Err(ArtifactError::LibraryCorrupt("invalid_audit_reference"));
            }
            validate_digest(&audit.request_digest)
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_digest"))?;
            if let Some(digest) = &audit.transaction_digest {
                validate_digest(digest)
                    .map_err(|_| ArtifactError::LibraryCorrupt("invalid_transaction_digest"))?;
            }
            let parsed_timestamp = LibraryTimestamp::parse(audit.committed_at.0.clone())
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_timestamp"))?;
            if parsed_timestamp != audit.committed_at {
                return Err(ArtifactError::LibraryCorrupt("invalid_audit_timestamp"));
            }
            LibraryTenantId::from_canonical_projection(audit.tenant_id.0.clone())
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_actor"))?;
            LibraryActorId::from_canonical_projection(audit.actor_id.0.clone())
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_actor"))?;
            validate_action(&audit.action)
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_action"))?;
            if !is_refresh
                && audit_record.is_none_or(|record| record.ownership.tenant_id != audit.tenant_id)
            {
                return Err(ArtifactError::LibraryCorrupt("audit_tenant_mismatch"));
            }
        }
        let audits_by_sequence = self
            .audit_intents
            .iter()
            .map(|audit| (audit.sequence, audit))
            .collect::<std::collections::HashMap<_, _>>();
        for receipt in self.receipts.values() {
            if !audits_by_sequence
                .get(&receipt.sequence)
                .is_some_and(|audit| {
                    audit.tenant_id == receipt.tenant_id
                        && audit.actor_id == receipt.actor_id
                        && audit.ownership == receipt.ownership
                        && audit.action == receipt.action
                        && audit.artifact_id == receipt.artifact_id
                        && audit.request_digest == receipt.request_digest
                        && audit.transaction_digest == receipt.transaction_digest
                        && audit.terminal_audit == receipt.terminal_audit
                })
            {
                return Err(ArtifactError::LibraryCorrupt("receipt_audit_mismatch"));
            }
        }
        Ok(())
    }

    pub(crate) fn validate(&self, store: &ArtifactStore) -> Result<(), ArtifactError> {
        self.validate_metadata()?;
        for record in self.records.values() {
            record.validate(store)?;
        }
        Ok(())
    }

    /// Tenant-qualified discoverable records. Archived records are never returned.
    pub fn list_for_tenant(&self, tenant: &LibraryTenantId) -> Vec<&SkillLibraryRecord> {
        self.records
            .values()
            .filter(|record| !record.archived && &record.ownership.tenant_id == tenant)
            .collect()
    }

    pub fn get_for_tenant(
        &self,
        tenant: &LibraryTenantId,
        artifact_id: &str,
    ) -> Option<&SkillLibraryRecord> {
        self.records.values().find(|record| {
            record.artifact_id == artifact_id
                && !record.archived
                && &record.ownership.tenant_id == tenant
        })
    }
}

/// One durable compare-and-swap mutation.
#[derive(Debug, Clone)]
// Create owns the complete bounded record so the transaction journal can be
// cloned and validated atomically; boxing it would complicate the durable
// mutation vocabulary without reducing retained data.
#[allow(clippy::large_enum_variant)]
pub enum LibraryMutation {
    Create {
        record: SkillLibraryRecord,
    },
    SetVisibility {
        artifact_id: String,
        visibility: SkillVisibility,
        updated_at: LibraryTimestamp,
    },
    Activate {
        artifact_id: String,
        revision_id: String,
        updated_at: LibraryTimestamp,
    },
    Save {
        artifact_id: String,
        revision_id: String,
        updated_at: LibraryTimestamp,
    },
    Rollback {
        artifact_id: String,
        revision_id: String,
        updated_at: LibraryTimestamp,
    },
    Deactivate {
        artifact_id: String,
        updated_at: LibraryTimestamp,
    },
    Archive {
        artifact_id: String,
        updated_at: LibraryTimestamp,
    },
    Refresh {
        artifact_id: String,
    },
}

impl LibraryMutation {
    fn action(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::SetVisibility { .. } => "set_visibility",
            Self::Activate { .. } => "activate",
            Self::Save { .. } => "save",
            Self::Rollback { .. } => "rollback",
            Self::Deactivate { .. } => "deactivate",
            Self::Archive { .. } => "archive",
            Self::Refresh { .. } => "refresh",
        }
    }
    fn artifact_id(&self) -> &str {
        match self {
            Self::Create { record } => &record.artifact_id,
            Self::SetVisibility { artifact_id, .. }
            | Self::Activate { artifact_id, .. }
            | Self::Save { artifact_id, .. }
            | Self::Rollback { artifact_id, .. }
            | Self::Deactivate { artifact_id, .. }
            | Self::Archive { artifact_id, .. } => artifact_id,
            Self::Refresh { artifact_id } => artifact_id,
        }
    }
}

impl ArtifactStore {
    /// Load and fully verify the committed library generation.
    pub fn library_snapshot(&self) -> Result<LibrarySnapshot, ArtifactError> {
        let _lock = self.library_lock()?;
        self.recover_pending_skill_transaction_locked()?;
        let mut snapshot = match self.read_library_snapshot() {
            Ok(snapshot) => snapshot,
            Err(ArtifactError::LibraryCorrupt("generation_digest_mismatch")) => {
                let path = self.root.join("library").join("state.json");
                let legacy: LegacyLibrarySnapshot = read_json(&path, MAX_LIBRARY_STATE_BYTES)
                    .map_err(|_| ArtifactError::LibraryCorrupt("invalid_legacy_snapshot"))?;
                legacy.verify_digest()?;
                legacy.into_current()
            }
            Err(error) => return Err(error),
        };
        let mut changed = false;
        let scoped_active_names = snapshot
            .records
            .values()
            .filter(|record| record.active_revision_id.is_some())
            .map(|record| {
                Ok((
                    LibrarySnapshot::active_name_key(&record.ownership, &record.name)?,
                    record.artifact_id.clone(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, ArtifactError>>()?;
        if snapshot.active_names != scoped_active_names {
            snapshot.active_names = scoped_active_names;
            changed = true;
        }
        for record in snapshot.records.values_mut() {
            if record.latest_revision_id.is_empty() || !record.materialized {
                let artifact = self.get(&record.artifact_id)?;
                let revision = self.revision(&record.artifact_id, &artifact.current_revision_id)?;
                record.latest_revision_id = artifact.current_revision_id;
                record.latest_revision_files = revision
                    .components
                    .into_iter()
                    .map(|component| SkillLibraryFile {
                        path: component.path,
                        digest: component.digest,
                        size: component.size,
                        media_type: component.media_type,
                    })
                    .collect();
                record.provenance_provider = artifact.provenance.provider;
                record.materialized = true;
                changed = true;
            }
        }
        if changed {
            snapshot.active_generation_digest = snapshot.compute_digest()?;
            snapshot.validate(self)?;
            self.persist_library_snapshot(&snapshot)?;
        }
        Ok(snapshot)
    }

    /// Atomically bind newly authored/imported bytes to one Library mutation.
    ///
    /// The pending intent is durable before library metadata changes. Artifact head/workspace
    /// promotion happens only after the library CAS commits, and every public library read repairs
    /// a committed-but-not-yet-promoted intent before returning state.
    #[allow(clippy::too_many_arguments)]
    pub fn mutate_library_with_materialized_outcome(
        &self,
        authorization: &LibraryAuthorization,
        target_ownership: &LibraryOwnership,
        expected_version: u64,
        idempotency: LibraryIdempotency,
        mutation: LibraryMutation,
        committed_at: LibraryTimestamp,
        mut materialized: MaterializedSkill,
        expected_revision_id: Option<&str>,
        mut fault: impl FnMut(SkillTransactionBoundary) -> Result<(), ArtifactError>,
    ) -> Result<LibraryMutationOutcome, ArtifactError> {
        target_ownership.validate()?;
        authorization.validate_for(target_ownership)?;
        validate_idempotency(&idempotency)?;
        let artifact_id = mutation.artifact_id().to_owned();
        validate_id(&artifact_id, "artifact_id")?;
        if materialized.interchange.descriptor.id != artifact_id {
            return Err(ArtifactError::Conflict("library_artifact_identity_changed"));
        }
        let revision_id = materialized.interchange.revision.id.clone();
        match &mutation {
            LibraryMutation::Create { .. } => {}
            LibraryMutation::Save {
                revision_id: expected,
                ..
            } if expected == &revision_id => {}
            _ => return Err(ArtifactError::Conflict("invalid_materialized_mutation")),
        }
        let scope_digest = receipt_scope_digest(
            &authorization.tenant_id,
            &authorization.actor_id,
            Some(target_ownership),
            mutation.action(),
            &artifact_id,
            &idempotency.key,
        )?;
        let _library_lock = self.library_lock()?;
        self.recover_pending_skill_transaction_locked()?;
        let current = self.read_library_snapshot_unvalidated()?;
        current.validate_metadata()?;
        if let Some(receipt) = current.receipts.get(&scope_digest) {
            if receipt.request_digest != idempotency.request_digest {
                return Err(ArtifactError::Conflict("idempotency_binding_changed"));
            }
            validate_replay_receipt(
                receipt,
                &scope_digest,
                authorization,
                mutation.action(),
                &artifact_id,
                &idempotency.key,
                current.version,
            )?;
            return Ok(LibraryMutationOutcome::Replayed(
                receipt.clone(),
                LibraryMutationSeal::for_receipt(receipt)?,
            ));
        }
        if current.version != expected_version {
            return Err(ArtifactError::Conflict("library_version_changed"));
        }

        let _artifact_lock = self.lock(&artifact_id)?;
        let prior_record = self.read_record_optional(&artifact_id)?;
        match (prior_record.as_ref(), expected_revision_id) {
            (None, None) => {}
            (Some(record), Some(expected)) if record.current_revision_id == expected => {}
            (None, Some(_)) => return Err(ArtifactError::NotFound("record")),
            (Some(_), None) => return Err(ArtifactError::Conflict("artifact_exists")),
            (Some(_), Some(_)) => return Err(ArtifactError::Conflict("revision_changed")),
        }
        if let Some(record) = &prior_record {
            materialized.interchange.revision.parent_revision_id =
                Some(record.current_revision_id.clone());
        }
        let files = materialized_skill_files(&materialized)?;
        let next_record = materialized_skill_record(&materialized, prior_record.as_ref())?;
        let mut pending = PendingSkillTransaction {
            schema_version: 2,
            scope_digest: scope_digest.clone(),
            request_digest: idempotency.request_digest.clone(),
            expected_library_version: expected_version,
            tenant_id: authorization.tenant_id.clone(),
            actor_id: authorization.actor_id.clone(),
            idempotency_key: idempotency.key.clone(),
            artifact_id: artifact_id.clone(),
            revision_id,
            revision: materialized.interchange.revision.clone(),
            prior_record,
            next_record,
            files: files
                .iter()
                .map(|file| PendingSkillFile {
                    path: file.path.clone(),
                    bytes: file.bytes.clone(),
                })
                .collect(),
            transaction_digest: String::new(),
        };
        pending.transaction_digest = pending.compute_digest()?;
        self.persist_pending_skill_transaction(&pending, &mut fault)?;

        let mut state = current;
        if matches!(mutation, LibraryMutation::Save { .. }) {
            let library_record = state
                .records
                .get_mut(&artifact_id)
                .ok_or(ArtifactError::NotFound("library_record"))?;
            library_record.latest_revision_id = materialized.interchange.revision.id.clone();
            library_record.latest_revision_files = materialized
                .interchange
                .revision
                .components
                .iter()
                .map(|component| SkillLibraryFile {
                    path: component.path.clone(),
                    digest: component.digest.clone(),
                    size: component.size,
                    media_type: component.media_type.clone(),
                })
                .collect();
            library_record.provenance_provider =
                materialized.interchange.provenance.provider.clone();
            library_record.materialized = true;
        }
        apply_mutation(
            &mut state,
            authorization,
            target_ownership,
            mutation.clone(),
        )?;
        state.version = state
            .version
            .checked_add(1)
            .ok_or(ArtifactError::Conflict("library_version_exhausted"))?;
        let mut receipt = LibraryReceipt {
            sequence: state.version,
            scope_digest: scope_digest.clone(),
            tenant_id: authorization.tenant_id.clone(),
            actor_id: authorization.actor_id.clone(),
            ownership: Some(target_ownership.clone()),
            action: mutation.action().to_owned(),
            artifact_id: artifact_id.clone(),
            idempotency_key: idempotency.key,
            request_digest: idempotency.request_digest.clone(),
            committed_version: state.version,
            transaction_digest: Some(pending.transaction_digest.clone()),
            response_facts: None,
            terminal_audit: idempotency.terminal_audit.clone(),
        };
        state.receipts.insert(scope_digest, receipt.clone());
        state.audit_intents.push(LibraryAuditIntent {
            sequence: state.version,
            action: mutation.action().to_owned(),
            tenant_id: authorization.tenant_id.clone(),
            actor_id: authorization.actor_id.clone(),
            ownership: Some(target_ownership.clone()),
            artifact_id,
            request_digest: idempotency.request_digest,
            committed_at,
            transaction_digest: Some(pending.transaction_digest.clone()),
            terminal_audit: idempotency.terminal_audit.clone(),
        });
        enforce_retention(&mut state);
        state.active_generation_digest = state.compute_digest()?;
        receipt.response_facts = Some(receipt_facts(&state, &receipt));
        state
            .receipts
            .insert(receipt.scope_digest.clone(), receipt.clone());
        state.validate_metadata()?;
        self.persist_library_snapshot_with_faults(&state, &mut fault)?;
        let committed_version = receipt.committed_version;
        let promoted = (|| {
            self.promote_pending_skill_transaction(&pending, &mut fault)?;
            self.clear_pending_skill_transaction(&pending.transaction_digest, &mut fault)
        })();
        if promoted.is_err() {
            return Err(ArtifactError::CommittedPending { committed_version });
        }
        let seal = LibraryMutationSeal::for_receipt(&receipt)?;
        Ok(LibraryMutationOutcome::Committed(receipt, seal))
    }

    /// Commit ownership, lifecycle, active-name index, receipt, and audit intent atomically.
    /// Commit one mutation authorized by the canonical AccessRuntime adapter.
    ///
    /// `authorization` must be created with [`LibraryAuthorization::from_authorized_access_projection`]
    /// immediately after final-boundary authorization. This storage layer verifies projection
    /// consistency but deliberately does not recreate product membership policy.
    pub fn mutate_library(
        &self,
        authorization: &LibraryAuthorization,
        target_ownership: &LibraryOwnership,
        expected_version: u64,
        idempotency: LibraryIdempotency,
        mutation: LibraryMutation,
        committed_at: LibraryTimestamp,
    ) -> Result<LibraryReceipt, ArtifactError> {
        self.mutate_library_outcome(
            authorization,
            target_ownership,
            expected_version,
            idempotency,
            mutation,
            committed_at,
        )
        .map(|outcome| outcome.receipt().clone())
    }

    pub fn mutate_library_outcome(
        &self,
        authorization: &LibraryAuthorization,
        target_ownership: &LibraryOwnership,
        expected_version: u64,
        idempotency: LibraryIdempotency,
        mutation: LibraryMutation,
        committed_at: LibraryTimestamp,
    ) -> Result<LibraryMutationOutcome, ArtifactError> {
        target_ownership.validate()?;
        authorization.validate_for(target_ownership)?;
        validate_id(mutation.artifact_id(), "artifact_id")?;
        validate_idempotency(&idempotency)?;
        let scope_digest = receipt_scope_digest(
            &authorization.tenant_id,
            &authorization.actor_id,
            Some(target_ownership),
            mutation.action(),
            mutation.artifact_id(),
            &idempotency.key,
        )?;
        let action = mutation.action().to_string();
        let target_artifact_id = mutation.artifact_id().to_string();
        // Artifact verification is deliberately outside the library-wide lock. Revisions are
        // immutable, so the verified facts remain valid while the short CAS commit runs.
        let prevalidated = self.read_library_snapshot()?;
        prevalidate_mutation(self, &mutation)?;
        let _lock = self.library_lock()?;
        let current = self.read_library_snapshot_unvalidated()?;
        // A matching receipt is authoritative only inside a completely valid committed metadata
        // generation. Keep Artifact byte/revision verification outside this lock, but never let a
        // forged or torn receipt bypass receipt/audit/index integrity checks.
        current.validate_metadata()?;
        // Resolve a concurrently committed identical request before comparing the stale
        // pre-lock generation. This gives duplicate contenders the winner's terminal receipt.
        if let Some(receipt) = current.receipts.get(&scope_digest) {
            if receipt.request_digest != idempotency.request_digest {
                return Err(ArtifactError::Conflict("idempotency_binding_changed"));
            }
            validate_replay_receipt(
                receipt,
                &scope_digest,
                authorization,
                &action,
                &target_artifact_id,
                &idempotency.key,
                current.version,
            )?;
            return Ok(LibraryMutationOutcome::Replayed(
                receipt.clone(),
                LibraryMutationSeal::for_receipt(receipt)?,
            ));
        }
        if current != prevalidated {
            return Err(ArtifactError::Conflict("library_version_changed"));
        }
        let mut state = prevalidated;
        if state.version != expected_version {
            return Err(ArtifactError::Conflict("library_version_changed"));
        }
        apply_mutation(&mut state, authorization, target_ownership, mutation)?;
        state.version = state
            .version
            .checked_add(1)
            .ok_or(ArtifactError::Conflict("library_version_exhausted"))?;
        let mut receipt = LibraryReceipt {
            sequence: state.version,
            scope_digest: scope_digest.clone(),
            tenant_id: authorization.tenant_id.clone(),
            actor_id: authorization.actor_id.clone(),
            ownership: Some(target_ownership.clone()),
            action: action.clone(),
            artifact_id: target_artifact_id.clone(),
            idempotency_key: idempotency.key,
            request_digest: idempotency.request_digest.clone(),
            committed_version: state.version,
            transaction_digest: None,
            response_facts: None,
            terminal_audit: idempotency.terminal_audit.clone(),
        };
        state.receipts.insert(scope_digest, receipt.clone());
        state.audit_intents.push(LibraryAuditIntent {
            sequence: state.version,
            action,
            tenant_id: authorization.tenant_id.clone(),
            actor_id: authorization.actor_id.clone(),
            ownership: Some(target_ownership.clone()),
            artifact_id: target_artifact_id,
            request_digest: idempotency.request_digest,
            committed_at,
            transaction_digest: None,
            terminal_audit: idempotency.terminal_audit.clone(),
        });
        enforce_retention(&mut state);
        state.active_generation_digest = state.compute_digest()?;
        receipt.response_facts = Some(receipt_facts(&state, &receipt));
        state
            .receipts
            .insert(receipt.scope_digest.clone(), receipt.clone());
        state.validate_metadata()?;
        let serialized = canonical_json::to_canonical_vec(&state)?;
        if serialized.len() as u64 > MAX_LIBRARY_STATE_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "library_state_bytes",
                limit: MAX_LIBRARY_STATE_BYTES,
            });
        }
        self.persist_library_snapshot(&state)?;
        let seal = LibraryMutationSeal::for_receipt(&receipt)?;
        Ok(LibraryMutationOutcome::Committed(receipt, seal))
    }

    /// Resolve an already committed request before repeating Artifact persistence.
    pub fn replay_library_mutation(
        &self,
        authorization: &LibraryAuthorization,
        target_ownership: &LibraryOwnership,
        idempotency: &LibraryIdempotency,
        mutation: &LibraryMutation,
    ) -> Result<Option<LibraryReceipt>, ArtifactError> {
        target_ownership.validate()?;
        authorization.validate_for(target_ownership)?;
        validate_id(mutation.artifact_id(), "artifact_id")?;
        validate_idempotency(idempotency)?;
        let scope_digest = receipt_scope_digest(
            &authorization.tenant_id,
            &authorization.actor_id,
            Some(target_ownership),
            mutation.action(),
            mutation.artifact_id(),
            &idempotency.key,
        )?;
        let _lock = self.library_lock()?;
        let current = self.read_library_snapshot_unvalidated()?;
        current.validate_metadata()?;
        let Some(receipt) = current.receipts.get(&scope_digest) else {
            return Ok(None);
        };
        if receipt.request_digest != idempotency.request_digest {
            return Err(ArtifactError::Conflict("idempotency_binding_changed"));
        }
        validate_replay_receipt(
            receipt,
            &scope_digest,
            authorization,
            mutation.action(),
            mutation.artifact_id(),
            &idempotency.key,
            current.version,
        )?;
        Ok(Some(receipt.clone()))
    }

    /// Persist the sole allowed post-commit terminal transition.
    ///
    /// The mutation outcome is an unforgeable capability produced by this store. The
    /// stored receipt must still exactly match it, and only a committed/commit audit
    /// may transition to failed/response while preserving every binding field.
    pub fn update_library_terminal_audit(
        &self,
        outcome: &LibraryMutationOutcome,
        terminal: LibraryDurableAudit,
    ) -> Result<(), ArtifactError> {
        let capability = outcome.receipt();
        outcome.seal().validate(capability)?;
        validate_digest(&capability.scope_digest)?;
        let _lock = self.library_lock()?;
        self.recover_pending_skill_transaction_locked()?;
        let mut state = self.read_library_snapshot_unvalidated()?;
        state.validate_metadata()?;
        let receipt = state
            .receipts
            .get_mut(&capability.scope_digest)
            .ok_or(ArtifactError::NotFound("library_receipt"))?;
        if receipt != capability {
            return Err(ArtifactError::Conflict("terminal_audit_capability_stale"));
        }
        terminal.validate_for(receipt)?;
        let Some(previous) = receipt.terminal_audit.as_ref() else {
            return Err(ArtifactError::LibraryCorrupt("missing_terminal_audit"));
        };
        let mut expected = previous.clone();
        expected.outcome = "failed".to_owned();
        expected.stage = "response".to_owned();
        expected.published_version = terminal.published_version;
        if previous.outcome != "committed" || previous.stage != "commit" || terminal != expected {
            return Err(ArtifactError::Conflict("invalid_terminal_audit_transition"));
        }
        receipt.terminal_audit = Some(terminal.clone());
        let audit = state
            .audit_intents
            .iter_mut()
            .find(|audit| audit.sequence == receipt.sequence)
            .ok_or(ArtifactError::LibraryCorrupt("receipt_audit_mismatch"))?;
        audit.terminal_audit = Some(terminal);
        state.active_generation_digest = state.compute_digest()?;
        state.validate_metadata()?;
        self.persist_library_snapshot(&state)
    }

    fn pending_skill_transaction_path(&self) -> std::path::PathBuf {
        self.root.join("library").join("pending-skill.json")
    }

    fn persist_pending_skill_transaction(
        &self,
        pending: &PendingSkillTransaction,
        fault: &mut impl FnMut(SkillTransactionBoundary) -> Result<(), ArtifactError>,
    ) -> Result<(), ArtifactError> {
        let path = self.pending_skill_transaction_path();
        let bytes = canonical_json::to_canonical_vec(pending)?;
        if bytes.len() as u64 > MAX_PENDING_SKILL_TRANSACTION_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "pending_skill_transaction_bytes",
                limit: MAX_PENDING_SKILL_TRANSACTION_BYTES,
            });
        }
        write_bytes_atomic_with_faults(
            &path,
            &bytes,
            [
                SkillTransactionBoundary::IntentWrite,
                SkillTransactionBoundary::IntentFileSync,
                SkillTransactionBoundary::IntentRename,
                SkillTransactionBoundary::IntentParentSync,
            ],
            fault,
        )
    }

    fn clear_pending_skill_transaction(
        &self,
        transaction_digest: &str,
        fault: &mut impl FnMut(SkillTransactionBoundary) -> Result<(), ArtifactError>,
    ) -> Result<(), ArtifactError> {
        let path = self.pending_skill_transaction_path();
        let applied = self.root.join("library").join("applied-skill.json");
        write_json_atomic_with_faults(
            &applied,
            &transaction_digest,
            [
                SkillTransactionBoundary::AppliedWrite,
                SkillTransactionBoundary::AppliedFileSync,
                SkillTransactionBoundary::AppliedRename,
                SkillTransactionBoundary::AppliedParentSync,
            ],
            fault,
        )?;
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        std::fs::remove_file(applied)?;
        sync_directory(path.parent().ok_or(ArtifactError::UnsafePath("library"))?)?;
        Ok(())
    }

    fn promote_pending_skill_transaction(
        &self,
        pending: &PendingSkillTransaction,
        fault: &mut impl FnMut(SkillTransactionBoundary) -> Result<(), ArtifactError>,
    ) -> Result<(), ArtifactError> {
        pending.validate()?;
        let files = pending
            .files
            .iter()
            .map(|file| SnapshotFile {
                path: file.path.clone(),
                bytes: file.bytes.clone(),
                unix_mode: pending
                    .revision
                    .components
                    .iter()
                    .find(|component| component.path == file.path)
                    .and_then(|component| component.unix_mode()),
            })
            .collect::<Vec<_>>();
        self.persist_revision_with_faults(&pending.revision, &pending.artifact_id, &files, fault)?;
        self.materialize_workspace(&pending.artifact_id, &files)?;
        self.persist_record_with_faults(
            &pending.next_record,
            [
                SkillTransactionBoundary::PromotionWrite,
                SkillTransactionBoundary::PromotionFileSync,
                SkillTransactionBoundary::PromotionRename,
                SkillTransactionBoundary::PromotionParentSync,
            ],
            fault,
        )
    }

    fn recover_pending_skill_transaction_locked(&self) -> Result<(), ArtifactError> {
        let path = self.pending_skill_transaction_path();
        if !path.exists() {
            return Ok(());
        }
        let pending: PendingSkillTransaction =
            read_json(&path, MAX_PENDING_SKILL_TRANSACTION_BYTES)?;
        pending.validate()?;
        let state = self.read_library_snapshot_unvalidated()?;
        state.validate_metadata()?;
        let committed_version = pending.expected_library_version.checked_add(1).ok_or(
            ArtifactError::LibraryCorrupt("invalid_pending_skill_transaction"),
        )?;
        let committed = state
            .receipts
            .get(&pending.scope_digest)
            .is_some_and(|receipt| {
                receipt.request_digest == pending.request_digest
                    && receipt.committed_version == committed_version
                    && receipt.tenant_id == pending.tenant_id
                    && receipt.actor_id == pending.actor_id
                    && receipt.idempotency_key == pending.idempotency_key
                    && receipt.artifact_id == pending.artifact_id
                    && receipt.transaction_digest.as_deref()
                        == Some(pending.transaction_digest.as_str())
            });
        if !committed && state.version >= committed_version {
            return Err(ArtifactError::LibraryCorrupt(
                "pending_skill_receipt_mismatch",
            ));
        }
        let current_record = self.read_record_optional(&pending.artifact_id)?;
        if current_record != pending.prior_record
            && current_record.as_ref() != Some(&pending.next_record)
        {
            return Err(ArtifactError::LibraryCorrupt(
                "pending_skill_prior_state_mismatch",
            ));
        }
        if committed {
            let _artifact_lock = self.lock(&pending.artifact_id)?;
            self.promote_pending_skill_transaction(&pending, &mut |_| Ok(()))?;
        }
        self.clear_pending_skill_transaction(&pending.transaction_digest, &mut |_| Ok(()))
    }
}

fn materialized_skill_files(
    materialized: &MaterializedSkill,
) -> Result<Vec<SnapshotFile>, ArtifactError> {
    let total_bytes = materialized
        .resources
        .values()
        .try_fold(0usize, |total, bytes| total.checked_add(bytes.len()))
        .ok_or(ArtifactError::LimitExceeded {
            what: "skill_package_size",
            limit: super::validation::MAX_SKILL_PACKAGE_BYTES as u64,
        })?;
    if total_bytes > super::validation::MAX_SKILL_PACKAGE_BYTES {
        return Err(ArtifactError::LimitExceeded {
            what: "skill_package_size",
            limit: super::validation::MAX_SKILL_PACKAGE_BYTES as u64,
        });
    }
    let root = format!(
        "skill://labby/{}/",
        materialized.interchange.descriptor.name
    );
    materialized
        .resources
        .iter()
        .map(|(uri, bytes)| {
            let path = uri
                .strip_prefix(&root)
                .ok_or(ArtifactError::Conflict("materialized_uri_root"))?;
            let unix_mode = materialized
                .interchange
                .revision
                .components
                .iter()
                .find(|component| component.path == path)
                .and_then(|component| component.unix_mode());
            Ok(SnapshotFile {
                path: path.to_owned(),
                bytes: bytes.clone(),
                unix_mode,
            })
        })
        .collect()
}

fn materialized_skill_record(
    materialized: &MaterializedSkill,
    prior: Option<&ArtifactRecord>,
) -> Result<ArtifactRecord, ArtifactError> {
    let revision_id = materialized.interchange.revision.id.clone();
    let mut revision_ids = prior.map_or_else(Vec::new, |record| record.revision_ids.clone());
    if !revision_ids.contains(&revision_id) {
        revision_ids.push(revision_id.clone());
    }
    let record = ArtifactRecord {
        schema_version: 1,
        descriptor: materialized.interchange.descriptor.clone(),
        current_revision_id: revision_id,
        revision_ids,
        provenance: materialized.interchange.provenance.clone(),
        license: materialized.interchange.license.clone(),
        lineage: prior.map_or_else(
            || materialized.interchange.lineage.clone(),
            |record| record.lineage.clone(),
        ),
        publication: prior.map_or_else(
            || materialized.interchange.publication.clone(),
            |record| record.publication.clone(),
        ),
    };
    record.validate()?;
    Ok(record)
}

fn validate_replay_receipt(
    receipt: &LibraryReceipt,
    scope_digest: &str,
    authorization: &LibraryAuthorization,
    action: &str,
    artifact_id: &str,
    idempotency_key: &str,
    current_version: u64,
) -> Result<(), ArtifactError> {
    if receipt.scope_digest != scope_digest
        || receipt.tenant_id != authorization.tenant_id
        || receipt.actor_id != authorization.actor_id
        || receipt.action != action
        || receipt.artifact_id != artifact_id
        || receipt.idempotency_key != idempotency_key
        || receipt.sequence == 0
        || receipt.committed_version != receipt.sequence
        || receipt.committed_version > current_version
    {
        return Err(ArtifactError::LibraryCorrupt("invalid_replay_receipt"));
    }
    Ok(())
}

fn enforce_retention(state: &mut LibrarySnapshot) {
    while state.receipts.len() > MAX_RECEIPTS {
        if let Some(key) = state
            .receipts
            .iter()
            .min_by_key(|(_, receipt)| receipt.sequence)
            .map(|(key, _)| key.clone())
        {
            state.receipts.remove(&key);
        }
    }
    if state.audit_intents.len() > MAX_AUDIT_INTENTS {
        state
            .audit_intents
            .drain(..state.audit_intents.len() - MAX_AUDIT_INTENTS);
    }
}

fn receipt_facts(state: &LibrarySnapshot, receipt: &LibraryReceipt) -> LibraryMutationReceiptFacts {
    let record = state.records.values().find(|record| {
        record.artifact_id == receipt.artifact_id
            && receipt
                .ownership
                .as_ref()
                .is_none_or(|owner| owner == &record.ownership)
    });
    let active_revision_id = record.and_then(|record| record.active_revision_id.clone());
    let canonical_uri = record
        .filter(|record| record.active_revision_id.is_some())
        .map(|record| format!("skill://labby/{}/SKILL.md", record.name));
    LibraryMutationReceiptFacts {
        active_revision_id,
        canonical_uri,
        old_generation: receipt.committed_version.saturating_sub(1),
        new_generation: receipt.committed_version,
        committed_library_version: receipt.committed_version,
        library_digest: state.active_generation_digest.clone(),
        relist_required: matches!(
            receipt.action.as_str(),
            "activate" | "deactivate" | "archive" | "rollback" | "refresh"
        ),
    }
}

// Helpers avoid persisting arbitrary caller data beyond bounded canonical fields.
fn apply_mutation(
    state: &mut LibrarySnapshot,
    authorization: &LibraryAuthorization,
    target_ownership: &LibraryOwnership,
    mutation: LibraryMutation,
) -> Result<(), ArtifactError> {
    match mutation {
        LibraryMutation::Create { record } => {
            if &record.ownership != target_ownership {
                return Err(ArtifactError::Conflict("ownership_mismatch"));
            }
            record.validate_metadata()?;
            if state.records.contains_key(&record.artifact_id) {
                return Err(ArtifactError::Conflict("library_record_exists"));
            }
            state.records.insert(record.artifact_id.clone(), record);
        }
        LibraryMutation::SetVisibility {
            artifact_id,
            visibility,
            updated_at,
        } => {
            let record = authorized_record(state, authorization, target_ownership, &artifact_id)?;
            record.visibility = visibility;
            record.updated_at = updated_at;
        }
        LibraryMutation::Activate {
            artifact_id,
            revision_id,
            updated_at,
        } => {
            let (name, archived) = {
                let record =
                    authorized_record(state, authorization, target_ownership, &artifact_id)?;
                (record.name.clone(), record.archived)
            };
            if archived {
                return Err(ArtifactError::Conflict("archived_skill"));
            }
            let active_key = LibrarySnapshot::active_name_key(target_ownership, &name)?;
            if state
                .active_names
                .get(&active_key)
                .is_some_and(|owner| owner != &artifact_id)
            {
                return Err(ArtifactError::Conflict("active_name_taken"));
            }
            let record = authorized_record(state, authorization, target_ownership, &artifact_id)?;
            record.active_revision_id = Some(revision_id);
            record.updated_at = updated_at;
            state.active_names.insert(active_key, artifact_id);
        }
        LibraryMutation::Save {
            artifact_id,
            revision_id: _,
            updated_at,
        } => {
            let record = authorized_record(state, authorization, target_ownership, &artifact_id)?;
            record.updated_at = updated_at;
        }
        LibraryMutation::Rollback {
            artifact_id,
            revision_id,
            updated_at,
        } => {
            let (name, archived) = {
                let record =
                    authorized_record(state, authorization, target_ownership, &artifact_id)?;
                (record.name.clone(), record.archived)
            };
            if archived {
                return Err(ArtifactError::Conflict("archived_skill"));
            }
            let active_key = LibrarySnapshot::active_name_key(target_ownership, &name)?;
            if state
                .active_names
                .get(&active_key)
                .is_some_and(|owner| owner != &artifact_id)
            {
                return Err(ArtifactError::Conflict("active_name_taken"));
            }
            let record = authorized_record(state, authorization, target_ownership, &artifact_id)?;
            record.active_revision_id = Some(revision_id);
            record.updated_at = updated_at;
            state.active_names.insert(active_key, artifact_id);
        }
        LibraryMutation::Deactivate {
            artifact_id,
            updated_at,
        } => {
            let name = authorized_record(state, authorization, target_ownership, &artifact_id)?
                .name
                .clone();
            state
                .active_names
                .remove(&LibrarySnapshot::active_name_key(target_ownership, &name)?);
            let record = authorized_record(state, authorization, target_ownership, &artifact_id)?;
            record.active_revision_id = None;
            record.updated_at = updated_at;
        }
        LibraryMutation::Archive {
            artifact_id,
            updated_at,
        } => {
            let name = authorized_record(state, authorization, target_ownership, &artifact_id)?
                .name
                .clone();
            state
                .active_names
                .remove(&LibrarySnapshot::active_name_key(target_ownership, &name)?);
            let record = authorized_record(state, authorization, target_ownership, &artifact_id)?;
            record.active_revision_id = None;
            record.archived = true;
            record.updated_at = updated_at;
        }
        LibraryMutation::Refresh { .. } => {}
    }
    Ok(())
}

fn prevalidate_mutation(
    store: &ArtifactStore,
    mutation: &LibraryMutation,
) -> Result<(), ArtifactError> {
    match mutation {
        LibraryMutation::Create { record } => record.validate(store),
        LibraryMutation::Activate {
            artifact_id,
            revision_id,
            ..
        } => store.revision(artifact_id, revision_id).map(|_| ()),
        LibraryMutation::Save {
            artifact_id,
            revision_id,
            ..
        }
        | LibraryMutation::Rollback {
            artifact_id,
            revision_id,
            ..
        } => store.revision(artifact_id, revision_id).map(|_| ()),
        LibraryMutation::SetVisibility { .. }
        | LibraryMutation::Deactivate { .. }
        | LibraryMutation::Archive { .. }
        | LibraryMutation::Refresh { .. } => Ok(()),
    }
}

fn authorized_record<'a>(
    state: &'a mut LibrarySnapshot,
    authorization: &LibraryAuthorization,
    target_ownership: &LibraryOwnership,
    artifact_id: &str,
) -> Result<&'a mut SkillLibraryRecord, ArtifactError> {
    let record = state
        .records
        .values_mut()
        .find(|record| record.artifact_id == artifact_id && &record.ownership == target_ownership)
        .ok_or(ArtifactError::NotFound("library_record"))?;
    authorization.validate_for(&record.ownership)?;
    Ok(record)
}

fn validate_skill_name(name: &str) -> Result<(), ArtifactError> {
    if name.is_empty()
        || name.len() > 128
        || name.trim() != name
        || name
            .chars()
            .any(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
    {
        return Err(invalid("name", "invalid_skill_name"));
    }
    Ok(())
}
fn validate_idempotency(value: &LibraryIdempotency) -> Result<(), ArtifactError> {
    if value.key.is_empty() || value.key.len() > 256 {
        return Err(invalid("idempotency_key", "invalid"));
    }
    validate_digest(&value.request_digest)?;
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), ArtifactError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid("digest", "invalid"));
    }
    Ok(())
}

fn validate_action(action: &str) -> Result<(), ArtifactError> {
    if !matches!(
        action,
        "create"
            | "set_visibility"
            | "save"
            | "activate"
            | "deactivate"
            | "archive"
            | "rollback"
            | "refresh"
    ) {
        return Err(invalid("action", "invalid"));
    }
    Ok(())
}

fn receipt_scope_digest(
    tenant_id: &LibraryTenantId,
    actor_id: &LibraryActorId,
    ownership: Option<&LibraryOwnership>,
    action: &str,
    artifact_id: &str,
    idempotency_key: &str,
) -> Result<String, ArtifactError> {
    match ownership {
        Some(ownership) => canonical_json::digest(&(
            tenant_id,
            actor_id,
            ownership,
            action,
            artifact_id,
            idempotency_key,
        )),
        None => {
            canonical_json::digest(&(tenant_id, actor_id, action, artifact_id, idempotency_key))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use tempfile::{TempDir, tempdir};

    use super::*;
    use crate::artifacts::store::LibraryPersistFault;
    use crate::artifacts::{
        ArtifactImportRequest, ArtifactProvenance, LogicalSkillFile, materialize_logical_skill,
    };

    fn ownership(tenant: &str, owner: &str) -> LibraryOwnership {
        LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection(tenant).unwrap(),
            LibraryActorId::from_canonical_projection(owner).unwrap(),
        )
    }

    fn owner_auth(owner: &LibraryOwnership) -> LibraryAuthorization {
        if owner.owner_kind() == LibraryOwnerKind::Personal {
            LibraryAuthorization::from_authorized_access_projection(
                owner.tenant_id.clone(),
                owner.owner_id.clone(),
                LibraryGrant::Owner,
            )
        } else {
            LibraryAuthorization::from_authorized_scope_projection(
                owner.tenant_id.clone(),
                owner.owner_id.clone(),
                owner.owner_kind(),
                owner.owner_id.clone(),
            )
        }
    }

    fn ts(value: &str) -> LibraryTimestamp {
        LibraryTimestamp::parse(value).unwrap()
    }

    #[test]
    fn ownership_migrates_missing_kind_to_personal_and_round_trips_scoped_owners() {
        let legacy = serde_json::json!({
            "schemaVersion": 1,
            "tenantId": "org-1",
            "ownerId": "principal-1"
        });
        let migrated: LibraryOwnership = serde_json::from_value(legacy).unwrap();
        assert_eq!(migrated.owner_kind(), LibraryOwnerKind::Personal);
        assert!(
            serde_json::to_value(&migrated)
                .unwrap()
                .get("ownerKind")
                .is_none()
        );

        for kind in [LibraryOwnerKind::Team, LibraryOwnerKind::Project] {
            let scoped = LibraryOwnership::scoped(
                LibraryTenantId::from_canonical_projection("org-1").unwrap(),
                kind,
                LibraryActorId::from_canonical_projection("scope-1").unwrap(),
            );
            let reopened: LibraryOwnership =
                serde_json::from_value(serde_json::to_value(&scoped).unwrap()).unwrap();
            assert_eq!(reopened, scoped);
        }
    }

    #[test]
    fn new_skill_artifact_identity_is_owner_qualified() {
        let mut personal = materialized("same-name", "body");
        let mut team = personal.clone();
        let personal_owner = ownership("org-a", "alice");
        let team_owner = LibraryOwnership::scoped(
            personal_owner.tenant_id.clone(),
            LibraryOwnerKind::Team,
            LibraryActorId::from_canonical_projection("team-a").unwrap(),
        );
        qualify_materialized_skill_owner(&mut personal, &personal_owner).unwrap();
        qualify_materialized_skill_owner(&mut team, &team_owner).unwrap();
        assert_ne!(
            personal.interchange.descriptor.id,
            team.interchange.descriptor.id
        );
        assert_eq!(personal.interchange.descriptor.name, "same-name");
        assert_eq!(team.interchange.descriptor.name, "same-name");
        assert_eq!(
            personal.interchange.revision.id, team.interchange.revision.id,
            "content addressing remains independent of ownership"
        );
    }

    #[test]
    fn receipt_scope_is_owner_qualified() {
        let tenant = LibraryTenantId::from_canonical_projection("org-a").unwrap();
        let actor = LibraryActorId::from_canonical_projection("admin").unwrap();
        let team_a = LibraryOwnership::scoped(
            tenant.clone(),
            LibraryOwnerKind::Team,
            LibraryActorId::from_canonical_projection("team-a").unwrap(),
        );
        let team_b = LibraryOwnership::scoped(
            tenant.clone(),
            LibraryOwnerKind::Team,
            LibraryActorId::from_canonical_projection("team-b").unwrap(),
        );
        let a = receipt_scope_digest(
            &tenant,
            &actor,
            Some(&team_a),
            "activate",
            "artifact",
            "same-key",
        )
        .unwrap();
        let b = receipt_scope_digest(
            &tenant,
            &actor,
            Some(&team_b),
            "activate",
            "artifact",
            "same-key",
        )
        .unwrap();
        assert_ne!(a, b);
    }

    fn idem(key: &str) -> LibraryIdempotency {
        LibraryIdempotency {
            key: key.to_string(),
            request_digest: canonical_json::digest(&key).unwrap(),
            terminal_audit: None,
        }
    }

    fn materialized(name: &str, body: &str) -> MaterializedSkill {
        materialize_logical_skill(
            name,
            vec![LogicalSkillFile::new(
                "SKILL.md",
                format!("---\nname: {name}\ndescription: Test\n---\n{body}\n"),
            )],
            ArtifactProvenance::default(),
        )
        .unwrap()
    }

    fn maximal_materialized(name: &str) -> MaterializedSkill {
        let mut logical = Vec::with_capacity(crate::skills::limits::MAX_RESOURCES_PER_SKILL);
        let skill_md = format!("---\nname: {name}\ndescription: Maximal\n---\n");
        let mut remaining =
            usize::try_from(crate::skills::limits::MAX_SKILL_TOTAL_BYTES).unwrap() - skill_md.len();
        logical.push(LogicalSkillFile::new("SKILL.md", skill_md));
        let supporting = crate::skills::limits::MAX_RESOURCES_PER_SKILL - 1;
        for index in 1..=supporting {
            let files_left = supporting - index + 1;
            let bytes = remaining.div_ceil(files_left);
            remaining -= bytes;
            logical.push(LogicalSkillFile::new(
                format!("resource-{index:02}.txt"),
                "x".repeat(bytes),
            ));
        }
        materialize_logical_skill(name, logical, ArtifactProvenance::default()).unwrap()
    }

    fn terminal_audit(
        owner: &LibraryOwnership,
        artifact_id: &str,
        committed_version: u64,
    ) -> LibraryDurableAudit {
        LibraryDurableAudit {
            schema_version: 1,
            correlation_id: "correlation-1".to_owned(),
            action: "artifacts.create".to_owned(),
            target_digest: canonical_json::digest(&artifact_id).unwrap(),
            revision_digest: None,
            tenant_id: owner.tenant_id.clone(),
            actor_id: owner.owner_id.clone(),
            surface: "mcp".to_owned(),
            policy_revision: 7,
            committed_version: Some(committed_version),
            published_version: Some(committed_version),
            outcome: "committed".to_owned(),
            stage: "commit".to_owned(),
            replayed: false,
        }
    }

    #[test]
    fn terminal_audit_accepts_legacy_skill_library_action_names() {
        let owner = ownership("org-a", "alice");
        let artifact_id = "art_legacy";
        let mut audit = terminal_audit(&owner, artifact_id, 1);
        let mut receipt = LibraryReceipt {
            sequence: 1,
            scope_digest: canonical_json::digest(&"scope").unwrap(),
            tenant_id: owner.tenant_id.clone(),
            actor_id: owner.owner_id.clone(),
            ownership: Some(owner.clone()),
            action: "create".to_owned(),
            artifact_id: artifact_id.to_owned(),
            idempotency_key: "legacy-import".to_owned(),
            request_digest: canonical_json::digest(&"request").unwrap(),
            committed_version: 1,
            transaction_digest: None,
            response_facts: None,
            terminal_audit: None,
        };

        audit.action = "skill_library.import".to_owned();
        audit.validate_for(&receipt).unwrap();

        receipt.action = "activate".to_owned();
        audit.action = "skill_library.activate".to_owned();
        audit.validate_for(&receipt).unwrap();
    }

    #[test]
    fn terminal_audit_transition_is_actor_bound_monotonic_and_restart_durable() {
        let root = tempdir().unwrap();
        let store = ArtifactStore::new(root.path()).unwrap();
        let owner = ownership("org-a", "alice");
        let candidate = materialized("audit-demo", "body");
        let artifact_id = candidate.interchange.descriptor.id.clone();
        let mut idempotency = idem("audited-create");
        idempotency.terminal_audit = Some(terminal_audit(&owner, &artifact_id, 1));
        let outcome = store
            .mutate_library_with_materialized_outcome(
                &owner_auth(&owner),
                &owner,
                0,
                idempotency,
                LibraryMutation::Create {
                    record: SkillLibraryRecord {
                        artifact_id: artifact_id.clone(),
                        name: "audit-demo".to_owned(),
                        ownership: owner.clone(),
                        visibility: SkillVisibility::Private,
                        archived: false,
                        active_revision_id: None,
                        latest_revision_id: candidate.interchange.revision.id.clone(),
                        latest_revision_files: Vec::new(),
                        search_metadata: Vec::new(),
                        provenance_provider: None,
                        materialized: false,
                        created_at: ts("2026-08-26T00:00:00Z"),
                        updated_at: ts("2026-08-26T00:00:00Z"),
                    },
                },
                ts("2026-08-26T00:00:00Z"),
                candidate,
                None,
                |_| Ok(()),
            )
            .unwrap();

        let mut forged = outcome.receipt().terminal_audit.clone().unwrap();
        forged.actor_id = LibraryActorId::from_canonical_projection("mallory").unwrap();
        forged.outcome = "failed".to_owned();
        forged.stage = "response".to_owned();
        assert!(matches!(
            store.update_library_terminal_audit(&outcome, forged),
            Err(ArtifactError::LibraryCorrupt("invalid_terminal_audit"))
        ));

        let mut response_failure = outcome.receipt().terminal_audit.clone().unwrap();
        response_failure.outcome = "failed".to_owned();
        response_failure.stage = "response".to_owned();
        store
            .update_library_terminal_audit(&outcome, response_failure.clone())
            .unwrap();
        assert!(matches!(
            store.update_library_terminal_audit(&outcome, response_failure.clone()),
            Err(ArtifactError::Conflict("terminal_audit_capability_stale"))
        ));

        let reopened = ArtifactStore::new(root.path()).unwrap();
        let persisted = reopened
            .library_snapshot()
            .unwrap()
            .receipts
            .get(&outcome.receipt().scope_digest)
            .unwrap()
            .terminal_audit
            .clone();
        assert_eq!(persisted, Some(response_failure));
    }

    #[test]
    fn stale_materialized_library_cas_never_changes_artifact_authority() {
        let root = tempdir().unwrap();
        let store = ArtifactStore::new(root.path()).unwrap();
        let owner = ownership("org-a", "alice");
        let candidate = materialized("demo", "body");
        let artifact_id = candidate.interchange.descriptor.id.clone();
        let revision_id = candidate.interchange.revision.id.clone();
        let result = store.mutate_library_with_materialized_outcome(
            &owner_auth(&owner),
            &owner,
            1,
            idem("stale-create"),
            LibraryMutation::Create {
                record: SkillLibraryRecord {
                    artifact_id: artifact_id.clone(),
                    name: "demo".to_owned(),
                    ownership: owner.clone(),
                    visibility: SkillVisibility::Private,
                    archived: false,
                    active_revision_id: None,
                    latest_revision_id: revision_id.clone(),
                    latest_revision_files: Vec::new(),
                    search_metadata: Vec::new(),
                    provenance_provider: None,
                    materialized: false,
                    created_at: ts("2026-08-26T00:00:00Z"),
                    updated_at: ts("2026-08-26T00:00:00Z"),
                },
            },
            ts("2026-08-26T00:00:00Z"),
            candidate,
            None,
            |_| Ok(()),
        );
        assert!(matches!(
            result,
            Err(ArtifactError::Conflict("library_version_changed"))
        ));
        assert!(matches!(
            store.get(&artifact_id),
            Err(ArtifactError::NotFound("record"))
        ));
        assert!(matches!(
            store.revision(&artifact_id, &revision_id),
            Err(ArtifactError::NotFound("revision"))
        ));
        assert!(!store.pending_skill_transaction_path().exists());
    }

    #[test]
    fn pending_journal_accepts_exact_skill_budget_and_rejects_cap_plus_one_before_commit() {
        let root = tempdir().unwrap();
        let store = ArtifactStore::new(root.path()).unwrap();
        let owner = ownership("org-a", "alice");
        let candidate = maximal_materialized("maximal");
        assert_eq!(
            candidate.resources.values().map(Vec::len).sum::<usize>(),
            usize::try_from(crate::skills::limits::MAX_SKILL_TOTAL_BYTES).unwrap()
        );
        let artifact_id = candidate.interchange.descriptor.id.clone();
        let revision_id = candidate.interchange.revision.id.clone();
        let outcome = store.mutate_library_with_materialized_outcome(
            &owner_auth(&owner),
            &owner,
            0,
            idem("maximal-create"),
            LibraryMutation::Create {
                record: SkillLibraryRecord {
                    artifact_id: artifact_id.clone(),
                    name: "maximal".to_owned(),
                    ownership: owner.clone(),
                    visibility: SkillVisibility::Private,
                    archived: false,
                    active_revision_id: None,
                    latest_revision_id: revision_id.clone(),
                    latest_revision_files: Vec::new(),
                    search_metadata: Vec::new(),
                    provenance_provider: None,
                    materialized: false,
                    created_at: ts("2026-08-26T00:00:00Z"),
                    updated_at: ts("2026-08-26T00:00:00Z"),
                },
            },
            ts("2026-08-26T00:00:00Z"),
            candidate,
            None,
            |boundary| {
                if boundary == SkillTransactionBoundary::PromotionWrite {
                    Err(ArtifactError::Conflict("injected_transaction_fault"))
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(
            outcome,
            Err(ArtifactError::CommittedPending {
                committed_version: 1
            })
        ));
        let journal = store.pending_skill_transaction_path();
        let journal_bytes = std::fs::read(&journal).unwrap();
        assert!(journal_bytes.len() as u64 <= MAX_PENDING_SKILL_TRANSACTION_BYTES);
        assert!(journal_bytes.len() < 90 * 1024 * 1024);
        let pending: PendingSkillTransaction =
            serde_json::from_slice(&journal_bytes).expect("compact journal remains recoverable");
        pending.validate().unwrap();
        let legacy: PendingSkillFile =
            serde_json::from_str(r#"{"path":"legacy","bytes":[0,1,255]}"#).unwrap();
        assert_eq!(legacy.bytes, [0, 1, 255]);

        let reopened = ArtifactStore::new(root.path()).unwrap();
        assert_eq!(
            reopened
                .library_snapshot()
                .unwrap()
                .records
                .get(&artifact_id)
                .unwrap()
                .latest_revision_id,
            revision_id
        );
        assert!(!journal.exists());

        let overflow_root = tempdir().unwrap();
        let overflow_store = ArtifactStore::new(overflow_root.path()).unwrap();
        let mut overflow = materialized("overflow", "body");
        let uri = overflow.resources.keys().next().unwrap().clone();
        overflow.resources.insert(
            uri,
            vec![b'x'; super::super::validation::MAX_SKILL_PACKAGE_BYTES + 1],
        );
        let overflow_artifact_id = overflow.interchange.descriptor.id.clone();
        let result = overflow_store.mutate_library_with_materialized_outcome(
            &owner_auth(&owner),
            &owner,
            0,
            idem("overflow-create"),
            LibraryMutation::Create {
                record: SkillLibraryRecord {
                    artifact_id: overflow_artifact_id,
                    name: "overflow".to_owned(),
                    ownership: owner.clone(),
                    visibility: SkillVisibility::Private,
                    archived: false,
                    active_revision_id: None,
                    latest_revision_id: overflow.interchange.revision.id.clone(),
                    latest_revision_files: Vec::new(),
                    search_metadata: Vec::new(),
                    provenance_provider: None,
                    materialized: false,
                    created_at: ts("2026-08-26T00:00:00Z"),
                    updated_at: ts("2026-08-26T00:00:00Z"),
                },
            },
            ts("2026-08-26T00:00:00Z"),
            overflow,
            None,
            |_| Ok(()),
        );
        assert!(matches!(
            result,
            Err(ArtifactError::LimitExceeded {
                what: "skill_package_size",
                limit
            }) if limit == super::super::validation::MAX_SKILL_PACKAGE_BYTES as u64
        ));
        assert_eq!(overflow_store.library_snapshot().unwrap().version, 0);
        assert!(!overflow_store.pending_skill_transaction_path().exists());
    }

    #[test]
    fn committed_legacy_numeric_array_journal_recovers_with_its_v1_digest() {
        let root = tempdir().unwrap();
        let store = ArtifactStore::new(root.path()).unwrap();
        let owner = ownership("org-a", "alice");
        let candidate = materialized("legacy-recovery", "legacy body");
        let artifact_id = candidate.interchange.descriptor.id.clone();
        let outcome = store.mutate_library_with_materialized_outcome(
            &owner_auth(&owner),
            &owner,
            0,
            idem("legacy-recovery-create"),
            LibraryMutation::Create {
                record: SkillLibraryRecord {
                    artifact_id: artifact_id.clone(),
                    name: "legacy-recovery".to_owned(),
                    ownership: owner.clone(),
                    visibility: SkillVisibility::Private,
                    archived: false,
                    active_revision_id: None,
                    latest_revision_id: candidate.interchange.revision.id.clone(),
                    latest_revision_files: Vec::new(),
                    search_metadata: Vec::new(),
                    provenance_provider: None,
                    materialized: false,
                    created_at: ts("2026-08-26T00:00:00Z"),
                    updated_at: ts("2026-08-26T00:00:00Z"),
                },
            },
            ts("2026-08-26T00:00:00Z"),
            candidate,
            None,
            |boundary| {
                if boundary == SkillTransactionBoundary::PromotionWrite {
                    Err(ArtifactError::Conflict("injected_transaction_fault"))
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(
            outcome,
            Err(ArtifactError::CommittedPending { .. })
        ));

        let path = store.pending_skill_transaction_path();
        let mut pending: PendingSkillTransaction =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        pending.schema_version = 1;
        pending.transaction_digest = pending.compute_digest().unwrap();

        // A committed v1 receipt and audit refer to the v1 transaction digest.
        let mut state = store.read_library_snapshot_unvalidated().unwrap();
        let receipt = state.receipts.get_mut(&pending.scope_digest).unwrap();
        receipt.transaction_digest = Some(pending.transaction_digest.clone());
        state
            .audit_intents
            .iter_mut()
            .find(|audit| audit.sequence == receipt.sequence)
            .unwrap()
            .transaction_digest = Some(pending.transaction_digest.clone());
        state.active_generation_digest = state.compute_digest().unwrap();
        store.persist_library_snapshot(&state).unwrap();

        // Recreate the exact legacy on-disk representation: JSON byte arrays, not base64.
        let mut legacy = serde_json::to_value(&pending).unwrap();
        for (file, source) in legacy["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .zip(&pending.files)
        {
            file["bytes"] = serde_json::Value::Array(
                source
                    .bytes
                    .iter()
                    .copied()
                    .map(serde_json::Value::from)
                    .collect(),
            );
        }
        std::fs::write(&path, canonical_json::to_canonical_vec(&legacy).unwrap()).unwrap();

        let reopened = ArtifactStore::new(root.path()).unwrap();
        let snapshot = reopened.library_snapshot().unwrap();
        assert_eq!(snapshot.version, 1);
        assert!(snapshot.records.contains_key(&artifact_id));
        assert!(reopened.get(&artifact_id).is_ok());
        assert!(!path.exists());
    }

    #[test]
    fn every_materialized_transaction_crash_boundary_recovers_to_one_complete_pair() {
        let boundaries = [
            SkillTransactionBoundary::IntentWrite,
            SkillTransactionBoundary::IntentFileSync,
            SkillTransactionBoundary::IntentRename,
            SkillTransactionBoundary::IntentParentSync,
            SkillTransactionBoundary::LibraryWrite,
            SkillTransactionBoundary::LibraryFileSync,
            SkillTransactionBoundary::LibraryRename,
            SkillTransactionBoundary::LibraryParentSync,
            SkillTransactionBoundary::PromotionWrite,
            SkillTransactionBoundary::PromotionFileSync,
            SkillTransactionBoundary::PromotionRename,
            SkillTransactionBoundary::PromotionParentSync,
            SkillTransactionBoundary::AppliedWrite,
            SkillTransactionBoundary::AppliedFileSync,
            SkillTransactionBoundary::AppliedRename,
            SkillTransactionBoundary::AppliedParentSync,
        ];
        for boundary in boundaries {
            let root = tempdir().unwrap();
            let store = ArtifactStore::new(root.path()).unwrap();
            let owner = ownership("org-a", "alice");
            let candidate = materialized("demo", "body");
            let artifact_id = candidate.interchange.descriptor.id.clone();
            let revision_id = candidate.interchange.revision.id.clone();
            let result = store.mutate_library_with_materialized_outcome(
                &owner_auth(&owner),
                &owner,
                0,
                idem("create"),
                LibraryMutation::Create {
                    record: SkillLibraryRecord {
                        artifact_id: artifact_id.clone(),
                        name: "demo".to_owned(),
                        ownership: owner.clone(),
                        visibility: SkillVisibility::Private,
                        archived: false,
                        active_revision_id: None,
                        latest_revision_id: revision_id.clone(),
                        latest_revision_files: Vec::new(),
                        search_metadata: Vec::new(),
                        provenance_provider: None,
                        materialized: false,
                        created_at: ts("2026-08-26T00:00:00Z"),
                        updated_at: ts("2026-08-26T00:00:00Z"),
                    },
                },
                ts("2026-08-26T00:00:00Z"),
                candidate,
                None,
                |observed| {
                    if observed == boundary {
                        Err(ArtifactError::Conflict("injected_transaction_fault"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err(), "{boundary:?} must interrupt the response");

            let reopened = ArtifactStore::new(root.path()).unwrap();
            let snapshot = reopened.library_snapshot().unwrap();
            let committed = snapshot.records.contains_key(&artifact_id);
            assert_eq!(
                reopened.get(&artifact_id).is_ok(),
                committed,
                "head/library mismatch after {boundary:?}"
            );
            assert_eq!(
                reopened.revision(&artifact_id, &revision_id).is_ok(),
                committed,
                "revision/library mismatch after {boundary:?}"
            );
            assert!(!reopened.pending_skill_transaction_path().exists());
        }
    }

    #[test]
    fn tampered_pending_transaction_is_rejected_without_clearing_evidence() {
        for field in ["files", "revision", "record", "tenant", "actor", "scope"] {
            let root = tempdir().unwrap();
            let store = ArtifactStore::new(root.path()).unwrap();
            let owner = ownership("org-a", "alice");
            let candidate = materialized("tamper-demo", "body");
            let artifact_id = candidate.interchange.descriptor.id.clone();
            let result = store.mutate_library_with_materialized_outcome(
                &owner_auth(&owner),
                &owner,
                0,
                idem("create"),
                LibraryMutation::Create {
                    record: SkillLibraryRecord {
                        artifact_id,
                        name: "tamper-demo".to_owned(),
                        ownership: owner.clone(),
                        visibility: SkillVisibility::Private,
                        archived: false,
                        active_revision_id: None,
                        latest_revision_id: candidate.interchange.revision.id.clone(),
                        latest_revision_files: Vec::new(),
                        search_metadata: Vec::new(),
                        provenance_provider: None,
                        materialized: false,
                        created_at: ts("2026-08-26T00:00:00Z"),
                        updated_at: ts("2026-08-26T00:00:00Z"),
                    },
                },
                ts("2026-08-26T00:00:00Z"),
                candidate,
                None,
                |boundary| {
                    if boundary == SkillTransactionBoundary::PromotionWrite {
                        Err(ArtifactError::Conflict("injected_transaction_fault"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(matches!(
                result,
                Err(ArtifactError::CommittedPending { .. })
            ));
            let path = store.pending_skill_transaction_path();
            let mut pending: PendingSkillTransaction =
                read_json(&path, MAX_PENDING_SKILL_TRANSACTION_BYTES).unwrap();
            match field {
                "files" => pending.files[0].bytes.push(b'!'),
                "revision" => pending.revision.id = canonical_json::digest(&"revision").unwrap(),
                "record" => pending.next_record.descriptor.name = "modified".to_owned(),
                "tenant" => {
                    pending.tenant_id =
                        LibraryTenantId::from_canonical_projection("org-b").unwrap();
                }
                "actor" => {
                    pending.actor_id =
                        LibraryActorId::from_canonical_projection("mallory").unwrap();
                }
                "scope" => pending.scope_digest = canonical_json::digest(&"scope").unwrap(),
                _ => unreachable!(),
            }
            pending.transaction_digest = pending.compute_digest().unwrap();
            write_json_atomic_with_faults(
                &path,
                &pending,
                [
                    SkillTransactionBoundary::IntentWrite,
                    SkillTransactionBoundary::IntentFileSync,
                    SkillTransactionBoundary::IntentRename,
                    SkillTransactionBoundary::IntentParentSync,
                ],
                &mut |_| Ok(()),
            )
            .unwrap();

            assert!(matches!(
                ArtifactStore::new(root.path()).unwrap().library_snapshot(),
                Err(ArtifactError::LibraryCorrupt(_))
            ));
            assert!(path.exists(), "tampered {field} evidence must remain");
        }
    }

    #[test]
    fn old_pending_journal_cannot_overwrite_a_newer_artifact_head() {
        let root = tempdir().unwrap();
        let store = ArtifactStore::new(root.path()).unwrap();
        let owner = ownership("org-a", "alice");
        let first = materialized("old-journal", "one");
        let artifact_id = first.interchange.descriptor.id.clone();
        let create = LibraryMutation::Create {
            record: SkillLibraryRecord {
                artifact_id: artifact_id.clone(),
                name: "old-journal".to_owned(),
                ownership: owner.clone(),
                visibility: SkillVisibility::Private,
                archived: false,
                active_revision_id: None,
                latest_revision_id: first.interchange.revision.id.clone(),
                latest_revision_files: Vec::new(),
                search_metadata: Vec::new(),
                provenance_provider: None,
                materialized: false,
                created_at: ts("2026-08-26T00:00:00Z"),
                updated_at: ts("2026-08-26T00:00:00Z"),
            },
        };
        let result = store.mutate_library_with_materialized_outcome(
            &owner_auth(&owner),
            &owner,
            0,
            idem("create-old"),
            create,
            ts("2026-08-26T00:00:00Z"),
            first,
            None,
            |boundary| {
                if boundary == SkillTransactionBoundary::PromotionWrite {
                    Err(ArtifactError::Conflict("injected_transaction_fault"))
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(
            result,
            Err(ArtifactError::CommittedPending { .. })
        ));
        let pending_path = store.pending_skill_transaction_path();
        let old_journal = std::fs::read(&pending_path).unwrap();
        assert_eq!(store.library_snapshot().unwrap().version, 1);

        let second = materialized("old-journal", "two");
        let second_revision = second.interchange.revision.id.clone();
        let prior_revision = store.get(&artifact_id).unwrap().current_revision_id;
        store
            .mutate_library_with_materialized_outcome(
                &owner_auth(&owner),
                &owner,
                1,
                idem("save-new"),
                LibraryMutation::Save {
                    artifact_id: artifact_id.clone(),
                    revision_id: second_revision.clone(),
                    updated_at: ts("2026-08-26T00:01:00Z"),
                },
                ts("2026-08-26T00:01:00Z"),
                second,
                Some(&prior_revision),
                |_| Ok(()),
            )
            .unwrap();
        std::fs::write(&pending_path, old_journal).unwrap();

        assert!(matches!(
            ArtifactStore::new(root.path()).unwrap().library_snapshot(),
            Err(ArtifactError::LibraryCorrupt(
                "pending_skill_prior_state_mismatch"
            ))
        ));
        assert_eq!(
            store.get(&artifact_id).unwrap().current_revision_id,
            second_revision
        );
        assert!(pending_path.exists());
    }

    fn add_skill(
        store: &ArtifactStore,
        source: &TempDir,
        namespace: &str,
        name: &str,
        owner: &LibraryOwnership,
        expected: u64,
    ) -> (String, String) {
        std::fs::write(
            source.path().join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Test\n---\nBody\n"),
        )
        .unwrap();
        let record = store
            .import_local(
                ArtifactImportRequest::new("skill", namespace, name),
                source.path(),
            )
            .unwrap();
        let artifact_id = record.descriptor.id.clone();
        let revision_id = record.current_revision_id.clone();
        store
            .mutate_library(
                &owner_auth(owner),
                owner,
                expected,
                idem(&format!("create-{namespace}")),
                LibraryMutation::Create {
                    record: SkillLibraryRecord {
                        artifact_id: artifact_id.clone(),
                        name: name.to_string(),
                        ownership: owner.clone(),
                        visibility: SkillVisibility::Private,
                        archived: false,
                        active_revision_id: None,
                        latest_revision_id: revision_id.clone(),
                        latest_revision_files: Vec::new(),
                        search_metadata: Vec::new(),
                        provenance_provider: None,
                        materialized: false,
                        created_at: ts("2026-08-26T00:00:00Z"),
                        updated_at: ts("2026-08-26T00:00:00Z"),
                    },
                },
                ts("2026-08-26T00:00:00Z"),
            )
            .unwrap();
        (artifact_id, revision_id)
    }

    #[test]
    fn metadata_round_trip_restart_tenant_isolation_and_archive_without_delete() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner_a = ownership("org-a", "alice");
        let (artifact, revision) = add_skill(&store, &source, "team-a", "demo", &owner_a, 0);
        let activated = store
            .mutate_library(
                &owner_auth(&owner_a),
                &owner_a,
                1,
                idem("activate"),
                LibraryMutation::Activate {
                    artifact_id: artifact.clone(),
                    revision_id: revision.clone(),
                    updated_at: ts("2026-08-26T00:01:00Z"),
                },
                ts("2026-08-26T00:01:00Z"),
            )
            .unwrap();
        assert_eq!(activated.committed_version, 2);

        let reopened = ArtifactStore::new(&root).unwrap();
        let snapshot = reopened.library_snapshot().unwrap();
        assert_eq!(
            snapshot
                .get_for_tenant(&owner_a.tenant_id, &artifact)
                .unwrap()
                .active_revision_id
                .as_deref(),
            Some(revision.as_str())
        );
        assert!(
            snapshot
                .get_for_tenant(&ownership("org-b", "bob").tenant_id, &artifact)
                .is_none()
        );

        reopened
            .mutate_library(
                &owner_auth(&owner_a),
                &owner_a,
                2,
                idem("archive"),
                LibraryMutation::Archive {
                    artifact_id: artifact.clone(),
                    updated_at: ts("2026-08-26T00:02:00Z"),
                },
                ts("2026-08-26T00:02:00Z"),
            )
            .unwrap();
        let snapshot = reopened.library_snapshot().unwrap();
        assert!(
            snapshot
                .get_for_tenant(&owner_a.tenant_id, &artifact)
                .is_none()
        );
        assert!(snapshot.records[&artifact].archived);
        reopened
            .revision(&artifact, &revision)
            .expect("archive must retain immutable bytes");
    }

    #[test]
    fn save_activate_rollback_deactivate_archive_lifecycle_is_exact() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, first_revision) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        std::fs::write(
            source.path().join("SKILL.md"),
            "---\nname: demo\ndescription: Test\n---\nSecond\n",
        )
        .unwrap();
        let second_revision = store
            .import_local(
                ArtifactImportRequest::new("skill", "team-a", "demo"),
                source.path(),
            )
            .unwrap()
            .current_revision_id;

        let operations = [
            LibraryMutation::Save {
                artifact_id: artifact.clone(),
                revision_id: second_revision.clone(),
                updated_at: ts("2026-08-26T00:01:00Z"),
            },
            LibraryMutation::Activate {
                artifact_id: artifact.clone(),
                revision_id: second_revision.clone(),
                updated_at: ts("2026-08-26T00:02:00Z"),
            },
            LibraryMutation::Rollback {
                artifact_id: artifact.clone(),
                revision_id: first_revision.clone(),
                updated_at: ts("2026-08-26T00:03:00Z"),
            },
            LibraryMutation::Deactivate {
                artifact_id: artifact.clone(),
                updated_at: ts("2026-08-26T00:04:00Z"),
            },
            LibraryMutation::Archive {
                artifact_id: artifact.clone(),
                updated_at: ts("2026-08-26T00:05:00Z"),
            },
        ];
        for (index, mutation) in operations.into_iter().enumerate() {
            store
                .mutate_library(
                    &owner_auth(&owner),
                    &owner,
                    index as u64 + 1,
                    idem(&format!("lifecycle-{index}")),
                    mutation,
                    ts("2026-08-26T00:10:00Z"),
                )
                .unwrap();
        }
        let snapshot = store.library_snapshot().unwrap();
        assert_eq!(snapshot.version, 6);
        assert!(snapshot.records[&artifact].archived);
        assert!(snapshot.records[&artifact].active_revision_id.is_none());
        store.revision(&artifact, &first_revision).unwrap();
        store.revision(&artifact, &second_revision).unwrap();
    }

    #[test]
    fn stale_cas_and_changed_idempotency_binding_fail() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let error = store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                0,
                idem("stale"),
                LibraryMutation::Deactivate {
                    artifact_id: artifact.clone(),
                    updated_at: ts("2026-08-26T00:03:00Z"),
                },
                ts("2026-08-26T00:03:00Z"),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::Conflict("library_version_changed")
        ));

        let mutation = LibraryMutation::SetVisibility {
            artifact_id: artifact,
            visibility: SkillVisibility::Tenant,
            updated_at: ts("2026-08-26T00:04:00Z"),
        };
        store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                1,
                idem("same"),
                mutation.clone(),
                ts("2026-08-26T00:04:00Z"),
            )
            .unwrap();
        let mut changed = idem("same");
        changed.request_digest = canonical_json::digest(&"different").unwrap();
        let error = store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                2,
                changed,
                mutation,
                ts("2026-08-26T00:04:00Z"),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::Conflict("idempotency_binding_changed")
        ));
    }

    #[test]
    fn concurrent_identical_idempotency_requests_return_one_equal_receipt() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let barrier = Arc::new(Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let root = root.clone();
                let owner = owner.clone();
                let artifact = artifact.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let store = ArtifactStore::new(root).unwrap();
                    barrier.wait();
                    store.mutate_library(
                        &owner_auth(&owner),
                        &owner,
                        1,
                        idem("identical-concurrent-request"),
                        LibraryMutation::SetVisibility {
                            artifact_id: artifact,
                            visibility: SkillVisibility::Tenant,
                            updated_at: ts("2026-08-26T00:04:30Z"),
                        },
                        ts("2026-08-26T00:04:30Z"),
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let receipts = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(receipts[0], receipts[1]);
        assert_eq!(receipts[0].committed_version, 2);
        let snapshot = ArtifactStore::new(root)
            .unwrap()
            .library_snapshot()
            .unwrap();
        assert_eq!(snapshot.version, 2);
        assert_eq!(snapshot.receipts.len(), 2);
        assert_eq!(snapshot.audit_intents.len(), 2);
    }

    #[test]
    fn concurrent_same_name_activation_has_exactly_one_winner() {
        let data = tempdir().unwrap();
        let source_a = tempdir().unwrap();
        let source_b = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (a, rev_a) = add_skill(&store, &source_a, "team-a", "shared", &owner, 0);
        let (b, rev_b) = add_skill(&store, &source_b, "team-b", "shared", &owner, 1);
        let barrier = Arc::new(Barrier::new(3));
        let handles = [(a, rev_a, "a"), (b, rev_b, "b")]
            .into_iter()
            .map(|(artifact_id, revision_id, key)| {
                let root = root.clone();
                let owner = owner.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let store = ArtifactStore::new(root).unwrap();
                    barrier.wait();
                    store.mutate_library(
                        &owner_auth(&owner),
                        &owner,
                        2,
                        idem(key),
                        LibraryMutation::Activate {
                            artifact_id,
                            revision_id,
                            updated_at: ts("2026-08-26T00:05:00Z"),
                        },
                        ts("2026-08-26T00:05:00Z"),
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            ArtifactStore::new(&root)
                .unwrap()
                .library_snapshot()
                .unwrap()
                .active_names
                .len(),
            1
        );
    }

    #[test]
    fn corrupt_or_truncated_generation_fails_closed() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        add_skill(
            &store,
            &source,
            "team-a",
            "demo",
            &ownership("org-a", "alice"),
            0,
        );
        std::fs::write(root.join("library/state.json"), b"{\"schemaVersion\":1").unwrap();
        assert!(matches!(
            ArtifactStore::new(&root).unwrap().library_snapshot(),
            Err(ArtifactError::LibraryCorrupt("invalid_json"))
        ));
    }

    fn write_snapshot(root: &std::path::Path, snapshot: &mut LibrarySnapshot) {
        snapshot.active_generation_digest = snapshot.compute_digest().unwrap();
        std::fs::write(
            root.join("library/state.json"),
            canonical_json::to_canonical_vec(snapshot).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn admin_mutation_attributes_receipt_and_audit_to_actual_actor() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let admin = LibraryAuthorization::from_authorized_access_projection(
            owner.tenant_id.clone(),
            LibraryActorId::from_canonical_projection("bob").unwrap(),
            LibraryGrant::Admin,
        );
        let receipt = store
            .mutate_library(
                &admin,
                &owner,
                1,
                idem("admin-change"),
                LibraryMutation::SetVisibility {
                    artifact_id: artifact,
                    visibility: SkillVisibility::Tenant,
                    updated_at: ts("2026-08-26T01:00:00Z"),
                },
                ts("2026-08-26T01:00:00Z"),
            )
            .unwrap();
        assert_eq!(receipt.actor_id.as_str(), "bob");
        let snapshot = store.library_snapshot().unwrap();
        assert_eq!(
            snapshot.audit_intents.last().unwrap().actor_id.as_str(),
            "bob"
        );
        assert_eq!(
            snapshot
                .records
                .values()
                .next()
                .unwrap()
                .ownership
                .owner_id
                .as_str(),
            "alice"
        );
    }

    #[test]
    fn invalid_oversized_timestamp_preserves_last_good_generation() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let error = store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                1,
                idem("oversized-time"),
                LibraryMutation::SetVisibility {
                    artifact_id: artifact,
                    visibility: SkillVisibility::Tenant,
                    updated_at: LibraryTimestamp("x".repeat(MAX_TIMESTAMP_BYTES + 1)),
                },
                ts("2026-08-26T01:30:00Z"),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::InvalidField {
                field: "timestamp",
                ..
            }
        ));
        let reopened = ArtifactStore::new(&root)
            .unwrap()
            .library_snapshot()
            .unwrap();
        assert_eq!(reopened.version, 1);
        assert_eq!(reopened.receipts.len(), 1);
    }

    #[test]
    fn forged_receipt_and_audit_fail_even_with_recomputed_generation_digest() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        add_skill(
            &store,
            &source,
            "team-a",
            "demo",
            &ownership("org-a", "alice"),
            0,
        );

        let mut forged_receipt = store.library_snapshot().unwrap();
        forged_receipt
            .receipts
            .values_mut()
            .next()
            .unwrap()
            .actor_id = LibraryActorId::from_canonical_projection("mallory").unwrap();
        write_snapshot(&root, &mut forged_receipt);
        assert!(matches!(
            store.library_snapshot(),
            Err(ArtifactError::LibraryCorrupt("receipt_scope_mismatch"))
        ));

        let mut forged_audit = forged_receipt;
        let receipt = forged_audit.receipts.values_mut().next().unwrap();
        receipt.actor_id = LibraryActorId::from_canonical_projection("alice").unwrap();
        forged_audit.audit_intents[0].artifact_id = "art_missing".into();
        write_snapshot(&root, &mut forged_audit);
        assert!(matches!(
            store.library_snapshot(),
            Err(ArtifactError::LibraryCorrupt("invalid_audit_reference"))
        ));
    }

    #[test]
    fn matching_replay_receipt_cannot_bypass_missing_audit_validation() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let record = store.library_snapshot().unwrap().records[&artifact].clone();

        let mut forged = store.library_snapshot().unwrap();
        forged.audit_intents.clear();
        write_snapshot(&root, &mut forged);

        let error = store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                0,
                idem("create-team-a"),
                LibraryMutation::Create { record },
                ts("2026-08-26T00:00:00Z"),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::LibraryCorrupt("receipt_audit_mismatch")
        ));
    }

    #[test]
    fn persistence_faults_never_reopen_a_partial_or_older_active_generation() {
        for fault in [
            LibraryPersistFault::Write,
            LibraryPersistFault::FileSync,
            LibraryPersistFault::Commit,
            LibraryPersistFault::DirectorySync,
            LibraryPersistFault::Enospc,
        ] {
            let data = tempdir().unwrap();
            let source = tempdir().unwrap();
            let root = data.path().join("store");
            let store = ArtifactStore::new(&root).unwrap();
            let owner = ownership("org-a", "alice");
            let (artifact, revision) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
            store.inject_library_persist_fault(fault);
            let error = store
                .mutate_library(
                    &owner_auth(&owner),
                    &owner,
                    1,
                    idem("faulted-activate"),
                    LibraryMutation::Activate {
                        artifact_id: artifact.clone(),
                        revision_id: revision.clone(),
                        updated_at: ts("2026-08-26T04:00:00Z"),
                    },
                    ts("2026-08-26T04:00:00Z"),
                )
                .unwrap_err();
            assert!(matches!(error, ArtifactError::Io(_)), "stage {fault:?}");

            let reopened = ArtifactStore::new(&root)
                .unwrap()
                .library_snapshot()
                .unwrap();
            if fault == LibraryPersistFault::DirectorySync {
                assert_eq!(reopened.version, 2);
                let active_key = LibrarySnapshot::active_name_key(&owner, "demo").unwrap();
                assert_eq!(reopened.active_names.get(&active_key), Some(&artifact));
                assert_eq!(
                    reopened.records[&artifact].active_revision_id.as_deref(),
                    Some(revision.as_str())
                );
            } else {
                assert_eq!(reopened.version, 1, "stage {fault:?}");
                assert!(reopened.active_names.is_empty(), "stage {fault:?}");
                assert!(reopened.records[&artifact].active_revision_id.is_none());
            }
        }
    }

    #[test]
    fn waiting_writer_makes_bounded_progress_after_library_lock_release() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let lock = store.library_lock().unwrap();
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        let writer = std::thread::spawn({
            let root = root.clone();
            let owner = owner.clone();
            move || {
                let store = ArtifactStore::new(root).unwrap();
                let result = store.mutate_library(
                    &owner_auth(&owner),
                    &owner,
                    1,
                    idem("bounded-writer"),
                    LibraryMutation::SetVisibility {
                        artifact_id: artifact,
                        visibility: SkillVisibility::Tenant,
                        updated_at: ts("2026-08-26T04:30:00Z"),
                    },
                    ts("2026-08-26T04:30:00Z"),
                );
                sent.send(result).unwrap();
            }
        });
        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "writer must wait while the commit lock is held"
        );
        drop(lock);
        let receipt = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("writer makes bounded progress after release")
            .unwrap();
        assert_eq!(receipt.committed_version, 2);
        writer.join().unwrap();
    }

    #[test]
    fn duplicate_active_names_fail_closed_on_reopen() {
        let data = tempdir().unwrap();
        let source_a = tempdir().unwrap();
        let source_b = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (a, rev_a) = add_skill(&store, &source_a, "team-a", "shared", &owner, 0);
        let (b, rev_b) = add_skill(&store, &source_b, "team-b", "shared", &owner, 1);
        let mut snapshot = store.library_snapshot().unwrap();
        snapshot.records.get_mut(&a).unwrap().active_revision_id = Some(rev_a);
        snapshot.records.get_mut(&b).unwrap().active_revision_id = Some(rev_b);
        snapshot.active_names.insert("shared".into(), a);
        write_snapshot(&root, &mut snapshot);
        assert!(matches!(
            ArtifactStore::new(&root).unwrap().library_snapshot(),
            Err(ArtifactError::LibraryCorrupt("duplicate_active_name"))
        ));
    }

    #[test]
    fn identical_active_names_are_isolated_by_owner_scope() {
        let data = tempdir().unwrap();
        let source_a = tempdir().unwrap();
        let source_b = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let team_a = LibraryOwnership::scoped(
            LibraryTenantId::from_canonical_projection("org-a").unwrap(),
            LibraryOwnerKind::Team,
            LibraryActorId::from_canonical_projection("team-a").unwrap(),
        );
        let team_b = LibraryOwnership::scoped(
            LibraryTenantId::from_canonical_projection("org-a").unwrap(),
            LibraryOwnerKind::Team,
            LibraryActorId::from_canonical_projection("team-b").unwrap(),
        );
        let auth = |owner: &LibraryOwnership| {
            LibraryAuthorization::from_authorized_scope_projection(
                owner.tenant_id.clone(),
                LibraryActorId::from_canonical_projection("admin").unwrap(),
                owner.owner_kind(),
                owner.owner_id.clone(),
            )
        };
        let (a, rev_a) = add_skill(&store, &source_a, "team-a", "shared", &team_a, 0);
        let (b, rev_b) = add_skill(&store, &source_b, "team-b", "shared", &team_b, 1);
        store
            .mutate_library(
                &auth(&team_a),
                &team_a,
                2,
                idem("activate-a"),
                LibraryMutation::Activate {
                    artifact_id: a.clone(),
                    revision_id: rev_a,
                    updated_at: ts("2026-08-26T05:00:00Z"),
                },
                ts("2026-08-26T05:00:00Z"),
            )
            .unwrap();
        store
            .mutate_library(
                &auth(&team_b),
                &team_b,
                3,
                idem("activate-b"),
                LibraryMutation::Activate {
                    artifact_id: b.clone(),
                    revision_id: rev_b,
                    updated_at: ts("2026-08-26T05:00:01Z"),
                },
                ts("2026-08-26T05:00:01Z"),
            )
            .unwrap();
        let snapshot = store.library_snapshot().unwrap();
        assert_eq!(snapshot.active_names.len(), 2);
        assert_eq!(
            snapshot
                .active_names
                .get(&LibrarySnapshot::active_name_key(&team_a, "shared").unwrap()),
            Some(&a)
        );
        assert_eq!(
            snapshot
                .active_names
                .get(&LibrarySnapshot::active_name_key(&team_b, "shared").unwrap()),
            Some(&b)
        );
    }

    #[test]
    fn archived_active_record_fails_closed_on_reopen() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, revision) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let mut snapshot = store.library_snapshot().unwrap();
        let record = snapshot.records.get_mut(&artifact).unwrap();
        record.archived = true;
        record.active_revision_id = Some(revision);
        snapshot.active_names.insert("demo".into(), artifact);
        write_snapshot(&root, &mut snapshot);
        assert!(matches!(
            ArtifactStore::new(&root).unwrap().library_snapshot(),
            Err(ArtifactError::LibraryCorrupt("archived_active_record"))
        ));
    }

    #[test]
    fn concurrent_deactivate_and_archive_cas_has_one_winner() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, revision) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                1,
                idem("activate-race"),
                LibraryMutation::Activate {
                    artifact_id: artifact.clone(),
                    revision_id: revision,
                    updated_at: ts("2026-08-26T03:00:00Z"),
                },
                ts("2026-08-26T03:00:00Z"),
            )
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let handles = [false, true]
            .into_iter()
            .map(|archive| {
                let root = root.clone();
                let owner = owner.clone();
                let artifact = artifact.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let store = ArtifactStore::new(root).unwrap();
                    let mutation = if archive {
                        LibraryMutation::Archive {
                            artifact_id: artifact,
                            updated_at: ts("2026-08-26T03:01:00Z"),
                        }
                    } else {
                        LibraryMutation::Deactivate {
                            artifact_id: artifact,
                            updated_at: ts("2026-08-26T03:01:00Z"),
                        }
                    };
                    barrier.wait();
                    store.mutate_library(
                        &owner_auth(&owner),
                        &owner,
                        2,
                        idem(if archive {
                            "archive-race"
                        } else {
                            "deactivate-race"
                        }),
                        mutation,
                        ts("2026-08-26T03:01:00Z"),
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let state = ArtifactStore::new(root)
            .unwrap()
            .library_snapshot()
            .unwrap();
        assert_eq!(state.version, 3);
        assert!(state.active_names.is_empty());
    }

    #[test]
    fn artifact_io_is_not_convoyed_by_the_library_commit_lock() {
        let data = tempdir().unwrap();
        let source_a = tempdir().unwrap();
        let source_b = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source_a, "team-a", "demo", &owner, 0);

        let _library_lock = store.library_lock().unwrap();
        std::fs::write(
            source_b.path().join("SKILL.md"),
            "---\nname: other\ndescription: Test\n---\nBody\n",
        )
        .unwrap();
        store
            .import_local(
                ArtifactImportRequest::new("skill", "team-b", "other"),
                source_b.path(),
            )
            .expect("unrelated immutable Artifact I/O uses its own artifact lock");

        let error = store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                1,
                idem("missing-revision"),
                LibraryMutation::Activate {
                    artifact_id: artifact,
                    revision_id: "rev_missing".into(),
                    updated_at: ts("2026-08-26T03:30:00Z"),
                },
                ts("2026-08-26T03:30:00Z"),
            )
            .unwrap_err();
        assert!(matches!(error, ArtifactError::NotFound("revision")));
    }

    #[test]
    fn cap_plus_one_reopens_and_evicts_by_sequence_not_digest_order() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let auth = owner_auth(&owner);
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let first_key = (0_u64..10_000)
            .map(|index| format!("reverse-{index}"))
            .find(|key| {
                receipt_scope_digest(
                    &auth.tenant_id,
                    &auth.actor_id,
                    Some(&owner),
                    "set_visibility",
                    &artifact,
                    key,
                )
                .unwrap()
                .as_bytes()[7]
                    >= b'e'
            })
            .unwrap();
        let first_scope = receipt_scope_digest(
            &auth.tenant_id,
            &auth.actor_id,
            Some(&owner),
            "set_visibility",
            &artifact,
            &first_key,
        )
        .unwrap();
        let mut snapshot = store.library_snapshot().unwrap();
        snapshot.receipts.clear();
        snapshot.audit_intents.clear();
        for index in 0..=MAX_RECEIPTS {
            let sequence = index as u64 + 1;
            let key = if index == 0 {
                first_key.clone()
            } else {
                format!("retained-{index}")
            };
            let scope = receipt_scope_digest(
                &auth.tenant_id,
                &auth.actor_id,
                Some(&owner),
                "set_visibility",
                &artifact,
                &key,
            )
            .unwrap();
            let request_digest = canonical_json::digest(&key).unwrap();
            snapshot.receipts.insert(
                scope.clone(),
                LibraryReceipt {
                    sequence,
                    scope_digest: scope,
                    tenant_id: auth.tenant_id.clone(),
                    actor_id: auth.actor_id.clone(),
                    ownership: Some(owner.clone()),
                    action: "set_visibility".into(),
                    artifact_id: artifact.clone(),
                    idempotency_key: key,
                    request_digest: request_digest.clone(),
                    committed_version: sequence,
                    transaction_digest: None,
                    response_facts: None,
                    terminal_audit: None,
                },
            );
            snapshot.audit_intents.push(LibraryAuditIntent {
                sequence,
                action: "set_visibility".into(),
                tenant_id: auth.tenant_id.clone(),
                actor_id: auth.actor_id.clone(),
                ownership: Some(owner.clone()),
                artifact_id: artifact.clone(),
                request_digest,
                committed_at: ts("2026-08-26T02:00:00Z"),
                transaction_digest: None,
                terminal_audit: None,
            });
        }
        snapshot.version = MAX_RECEIPTS as u64 + 1;
        enforce_retention(&mut snapshot);
        write_snapshot(&root, &mut snapshot);
        let snapshot = ArtifactStore::new(&root)
            .unwrap()
            .library_snapshot()
            .unwrap();
        assert_eq!(snapshot.receipts.len(), MAX_RECEIPTS);
        assert_eq!(snapshot.audit_intents.len(), MAX_AUDIT_INTENTS);
        assert!(!snapshot.receipts.contains_key(&first_scope));
        assert!(snapshot.receipts.keys().any(|scope| scope < &first_scope));
        assert_eq!(
            snapshot
                .receipts
                .values()
                .map(|receipt| receipt.sequence)
                .min(),
            Some(2)
        );

        // Once the bounded retention window has deliberately forgotten a receipt, replay is a
        // fresh CAS mutation rather than an incorrectly attributed replay of another digest key.
        let replay = store
            .mutate_library(
                &auth,
                &owner,
                MAX_RECEIPTS as u64 + 1,
                idem(&first_key),
                LibraryMutation::SetVisibility {
                    artifact_id: artifact,
                    visibility: SkillVisibility::Tenant,
                    updated_at: ts("2026-08-26T02:01:00Z"),
                },
                ts("2026-08-26T02:01:00Z"),
            )
            .unwrap();
        assert_eq!(replay.committed_version, MAX_RECEIPTS as u64 + 2);
        assert_eq!(replay.idempotency_key, first_key);
    }

    #[test]
    fn legacy_index_digest_is_verified_before_one_time_hydration() {
        let root = tempdir().unwrap();
        let store = ArtifactStore::new(root.path()).unwrap();
        let owner = ownership("org-a", "alice");
        let candidate = materialized("legacy-index", "body");
        let artifact_id = candidate.interchange.descriptor.id.clone();
        store
            .mutate_library_with_materialized_outcome(
                &owner_auth(&owner),
                &owner,
                0,
                idem("legacy-create"),
                LibraryMutation::Create {
                    record: SkillLibraryRecord {
                        artifact_id: artifact_id.clone(),
                        name: "legacy-index".into(),
                        ownership: owner.clone(),
                        visibility: SkillVisibility::Private,
                        archived: false,
                        active_revision_id: None,
                        latest_revision_id: candidate.interchange.revision.id.clone(),
                        latest_revision_files: Vec::new(),
                        search_metadata: Vec::new(),
                        provenance_provider: None,
                        materialized: false,
                        created_at: ts("2026-08-26T00:00:00Z"),
                        updated_at: ts("2026-08-26T00:00:00Z"),
                    },
                },
                ts("2026-08-26T00:00:00Z"),
                candidate,
                None,
                |_| Ok(()),
            )
            .unwrap();
        let current = store.read_library_snapshot_unvalidated().unwrap();
        let mut legacy = LegacyLibrarySnapshot {
            schema_version: current.schema_version,
            version: current.version,
            active_generation_digest: String::new(),
            records: current
                .records
                .into_iter()
                .map(|(id, record)| {
                    (
                        id,
                        LegacySkillLibraryRecord {
                            artifact_id: record.artifact_id,
                            name: record.name,
                            ownership: record.ownership,
                            visibility: record.visibility,
                            archived: record.archived,
                            active_revision_id: record.active_revision_id,
                            created_at: record.created_at,
                            updated_at: record.updated_at,
                        },
                    )
                })
                .collect(),
            active_names: current.active_names,
            receipts: current.receipts,
            audit_intents: current.audit_intents,
        };
        legacy.active_generation_digest = legacy.compute_digest().unwrap();
        let authentic = serde_json::to_value(&legacy).unwrap();
        let path = root.path().join("library/state.json");
        let mut tampered_snapshots = Vec::new();
        for (field, value) in [
            ("ownership", serde_json::json!(null)),
            ("visibility", serde_json::json!("tenant")),
            ("activeRevisionId", serde_json::json!("sha256:forged")),
        ] {
            let mut tampered = authentic.clone();
            let record = tampered["records"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .next()
                .unwrap();
            record[field] = value;
            tampered_snapshots.push(tampered);
        }
        for (field, value) in [
            ("version", serde_json::json!(99)),
            ("receipts", serde_json::json!({})),
            ("activeNames", serde_json::json!({"forged": "artifact"})),
        ] {
            let mut tampered = authentic.clone();
            tampered[field] = value;
            tampered_snapshots.push(tampered);
        }
        for tampered in tampered_snapshots {
            std::fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
            assert!(matches!(
                store.library_snapshot(),
                Err(ArtifactError::LibraryCorrupt(_))
            ));
        }
        std::fs::write(&path, serde_json::to_vec(&authentic).unwrap()).unwrap();
        let migrated = store.library_snapshot().unwrap();
        let record = &migrated.records[&artifact_id];
        assert!(record.materialized);
        assert!(!record.latest_revision_id.is_empty());
        assert_eq!(record.latest_revision_files[0].path, "SKILL.md");
    }
}
