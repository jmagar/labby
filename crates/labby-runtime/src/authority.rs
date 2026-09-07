//! Surface-neutral authority snapshot and execution-lease contracts.
//!
//! These types carry already-resolved authority between product boundaries.
//! They do not authenticate, read policy, persist state, or contact another
//! service. A consumer must obtain a fresh epoch vector from its authority
//! owner and validate the lease at every declared safe boundary.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

pub use labby_primitives::access::{
    Capability, InstallationId, OwnerScope, PrincipalId, ProjectId, ResourceId, TeamId,
};

/// Version of the canonical epoch-vector encoding used by fingerprints.
pub const AUTHORITY_EPOCH_VECTOR_VERSION: u16 = 1;

/// Maximum lifetime accepted by the shared lease contract.
pub const MAX_AUTHORITY_LEASE_MILLIS: u64 = 5 * 60 * 1_000;

/// Epoch for one Team membership participating in a decision.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMembershipEpoch {
    pub team_id: String,
    pub epoch: u64,
}

/// Complete normalized invalidation input for one authority decision.
///
/// Optional values are absent only when that domain cannot affect the
/// decision. Team membership epochs are sorted and unique after construction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorityEpochVectorInput {
    pub version: u16,
    pub authority_schema_generation: u64,
    pub installation_epoch: u64,
    pub organization_epoch: u64,
    pub principal_epoch: u64,
    pub team_membership_epochs: Vec<TeamMembershipEpoch>,
    pub team_policy_epoch: Option<u64>,
    pub project_membership_epoch: Option<u64>,
    pub project_policy_epoch: Option<u64>,
    pub resource_policy_epoch: Option<u64>,
    pub gateway_catalog_generation: Option<u64>,
    pub depot_projection_watermark: Option<u64>,
    pub credential_generation: Option<u64>,
    pub session_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorityEpochVector {
    version: u16,
    authority_schema_generation: u64,
    installation_epoch: u64,
    organization_epoch: u64,
    principal_epoch: u64,
    team_membership_epochs: Vec<TeamMembershipEpoch>,
    team_policy_epoch: Option<u64>,
    project_membership_epoch: Option<u64>,
    project_policy_epoch: Option<u64>,
    resource_policy_epoch: Option<u64>,
    gateway_catalog_generation: Option<u64>,
    depot_projection_watermark: Option<u64>,
    credential_generation: Option<u64>,
    session_generation: u64,
}

impl AuthorityEpochVector {
    pub fn new(mut value: AuthorityEpochVectorInput) -> Result<Self, AuthorityContractError> {
        if value.version != AUTHORITY_EPOCH_VECTOR_VERSION {
            return Err(AuthorityContractError::UnsupportedEpochVectorVersion(
                value.version,
            ));
        }
        for membership in &value.team_membership_epochs {
            validate_token("team id", &membership.team_id)?;
        }
        value
            .team_membership_epochs
            .sort_by(|left, right| left.team_id.cmp(&right.team_id));
        for pair in value.team_membership_epochs.windows(2) {
            if pair[0].team_id == pair[1].team_id {
                return Err(AuthorityContractError::DuplicateTeamEpoch(
                    pair[0].team_id.clone(),
                ));
            }
        }
        Ok(Self {
            version: value.version,
            authority_schema_generation: value.authority_schema_generation,
            installation_epoch: value.installation_epoch,
            organization_epoch: value.organization_epoch,
            principal_epoch: value.principal_epoch,
            team_membership_epochs: value.team_membership_epochs,
            team_policy_epoch: value.team_policy_epoch,
            project_membership_epoch: value.project_membership_epoch,
            project_policy_epoch: value.project_policy_epoch,
            resource_policy_epoch: value.resource_policy_epoch,
            gateway_catalog_generation: value.gateway_catalog_generation,
            depot_projection_watermark: value.depot_projection_watermark,
            credential_generation: value.credential_generation,
            session_generation: value.session_generation,
        })
    }

    /// Stable SHA-256 fingerprint of the normalized versioned representation.
    pub fn fingerprint(&self) -> AuthorityEpochFingerprint {
        let encoded = serde_json::to_vec(self).expect("authority epoch vector is serializable");
        let digest = Sha256::digest(encoded);
        AuthorityEpochFingerprint(format!(
            "sha256:{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityEpochFingerprint(String);

impl AuthorityEpochFingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The exact subject, action, and resource to which a lease is bound.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityBinding {
    principal_id: PrincipalId,
    owner_scope: OwnerScope,
    capability: Capability,
    method: String,
    resource_id: ResourceId,
    intent_id: Option<ResourceId>,
}

impl AuthorityBinding {
    pub fn new(
        principal_id: PrincipalId,
        owner_scope: OwnerScope,
        capability: Capability,
        method: impl Into<String>,
        resource_id: ResourceId,
        intent_id: Option<ResourceId>,
    ) -> Result<Self, AuthorityContractError> {
        let value = Self {
            principal_id,
            owner_scope,
            capability,
            method: method.into(),
            resource_id,
            intent_id,
        };
        validate_token("method", &value.method)?;
        Ok(value)
    }

    pub fn principal_id(&self) -> &str {
        self.principal_id.as_str()
    }

    pub fn owner_scope(&self) -> &OwnerScope {
        &self.owner_scope
    }

    pub fn capability(&self) -> &Capability {
        &self.capability
    }

    pub fn resource_id(&self) -> &ResourceId {
        &self.resource_id
    }
}

/// Point at which long-lived work must re-observe current authority.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AuthoritySafeBoundary {
    BeforeDispatch,
    BeforeExternalEffect,
    BeforeChunk,
    BeforeCommit,
    BeforeRetainedResume,
}

/// Short-lived, action- and resource-bound execution authority.
///
/// This type deliberately has no serialization implementation. It is a local
/// runtime handle, not a bearer token or wire assertion.
#[derive(Clone, Debug)]
pub struct AuthorityLease {
    binding: AuthorityBinding,
    epoch_fingerprint: AuthorityEpochFingerprint,
    issued_at_millis: u64,
    expires_at_millis: u64,
    safe_boundaries: BTreeSet<AuthoritySafeBoundary>,
}

impl AuthorityLease {
    pub fn new(
        binding: AuthorityBinding,
        epochs: &AuthorityEpochVector,
        issued_at_millis: u64,
        expires_at_millis: u64,
        safe_boundaries: impl IntoIterator<Item = AuthoritySafeBoundary>,
    ) -> Result<Self, AuthorityContractError> {
        let lifetime = expires_at_millis
            .checked_sub(issued_at_millis)
            .filter(|value| *value > 0 && *value <= MAX_AUTHORITY_LEASE_MILLIS)
            .ok_or(AuthorityContractError::InvalidLeaseLifetime)?;
        debug_assert!(lifetime > 0);
        let safe_boundaries = safe_boundaries.into_iter().collect::<BTreeSet<_>>();
        if safe_boundaries.is_empty() {
            return Err(AuthorityContractError::NoSafeBoundaries);
        }
        Ok(Self {
            binding,
            epoch_fingerprint: epochs.fingerprint(),
            issued_at_millis,
            expires_at_millis,
            safe_boundaries,
        })
    }

    pub fn binding(&self) -> &AuthorityBinding {
        &self.binding
    }

    pub fn issued_at_millis(&self) -> u64 {
        self.issued_at_millis
    }

    pub fn expires_at_millis(&self) -> u64 {
        self.expires_at_millis
    }

    /// Revalidate at a safe boundary using explicitly supplied current time
    /// and a freshly resolved epoch vector.
    pub fn validate_at(
        &self,
        boundary: AuthoritySafeBoundary,
        now_millis: u64,
        current_epochs: &AuthorityEpochVector,
    ) -> Result<(), AuthorityLeaseError> {
        if !self.safe_boundaries.contains(&boundary) {
            return Err(AuthorityLeaseError::UndeclaredSafeBoundary(boundary));
        }
        if now_millis < self.issued_at_millis {
            return Err(AuthorityLeaseError::ClockBeforeIssue);
        }
        if now_millis >= self.expires_at_millis {
            return Err(AuthorityLeaseError::Expired);
        }
        if current_epochs.fingerprint() != self.epoch_fingerprint {
            return Err(AuthorityLeaseError::AuthorityChanged);
        }
        Ok(())
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum AuthorityContractError {
    #[error("invalid authority field: {0}")]
    InvalidField(&'static str),
    #[error("unsupported authority epoch vector version {0}")]
    UnsupportedEpochVectorVersion(u16),
    #[error("duplicate Team epoch for {0}")]
    DuplicateTeamEpoch(String),
    #[error("authority lease lifetime is zero, overlong, or overflows")]
    InvalidLeaseLifetime,
    #[error("authority lease declares no safe boundary")]
    NoSafeBoundaries,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AuthorityLeaseError {
    #[error("authority lease is expired")]
    Expired,
    #[error("authority changed since the lease was issued")]
    AuthorityChanged,
    #[error("clock is earlier than authority lease issue time")]
    ClockBeforeIssue,
    #[error("authority boundary {0:?} was not declared by this lease")]
    UndeclaredSafeBoundary(AuthoritySafeBoundary),
}

fn validate_token(field: &'static str, value: &str) -> Result<(), AuthorityContractError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(AuthorityContractError::InvalidField(field))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch_input(team_epochs: Vec<TeamMembershipEpoch>) -> AuthorityEpochVectorInput {
        AuthorityEpochVectorInput {
            version: AUTHORITY_EPOCH_VECTOR_VERSION,
            authority_schema_generation: 6,
            installation_epoch: 2,
            organization_epoch: 3,
            principal_epoch: 4,
            team_membership_epochs: team_epochs,
            team_policy_epoch: Some(5),
            project_membership_epoch: Some(6),
            project_policy_epoch: Some(7),
            resource_policy_epoch: Some(8),
            gateway_catalog_generation: Some(9),
            depot_projection_watermark: Some(10),
            credential_generation: Some(11),
            session_generation: 12,
        }
    }

    fn epochs(team_epochs: Vec<TeamMembershipEpoch>) -> AuthorityEpochVector {
        AuthorityEpochVector::new(epoch_input(team_epochs)).unwrap()
    }

    fn binding() -> AuthorityBinding {
        AuthorityBinding::new(
            PrincipalId::new("principal-1").unwrap(),
            OwnerScope::Team(TeamId::new("team-a").unwrap()),
            Capability::ScopeOperate,
            "POST",
            ResourceId::new("task-1").unwrap(),
            Some(ResourceId::new("intent-1").unwrap()),
        )
        .unwrap()
    }

    #[test]
    fn epoch_fingerprint_is_order_independent_after_normalization() {
        let left = epochs(vec![
            TeamMembershipEpoch {
                team_id: "team-b".into(),
                epoch: 2,
            },
            TeamMembershipEpoch {
                team_id: "team-a".into(),
                epoch: 1,
            },
        ]);
        let right = epochs(vec![
            TeamMembershipEpoch {
                team_id: "team-a".into(),
                epoch: 1,
            },
            TeamMembershipEpoch {
                team_id: "team-b".into(),
                epoch: 2,
            },
        ]);
        assert_eq!(left, right);
        assert_eq!(left.fingerprint(), right.fingerprint());
    }

    #[test]
    fn duplicate_team_epoch_is_rejected_instead_of_merged() {
        let mut input = epoch_input(Vec::new());
        input.team_membership_epochs = vec![
            TeamMembershipEpoch {
                team_id: "team-a".into(),
                epoch: 1,
            },
            TeamMembershipEpoch {
                team_id: "team-a".into(),
                epoch: 2,
            },
        ];
        let error = AuthorityEpochVector::new(input).unwrap_err();
        assert_eq!(
            error,
            AuthorityContractError::DuplicateTeamEpoch("team-a".into())
        );
    }

    #[test]
    fn lease_revalidates_expiry_epoch_and_declared_boundaries() {
        let epochs = epochs(Vec::new());
        let lease = AuthorityLease::new(
            binding(),
            &epochs,
            1_000,
            2_000,
            [
                AuthoritySafeBoundary::BeforeDispatch,
                AuthoritySafeBoundary::BeforeExternalEffect,
            ],
        )
        .unwrap();
        assert_eq!(
            lease.validate_at(AuthoritySafeBoundary::BeforeDispatch, 1_500, &epochs),
            Ok(())
        );
        assert_eq!(
            lease.validate_at(AuthoritySafeBoundary::BeforeDispatch, 2_000, &epochs),
            Err(AuthorityLeaseError::Expired)
        );
        let mut changed = epochs.clone();
        changed.principal_epoch += 1;
        assert_eq!(
            lease.validate_at(AuthoritySafeBoundary::BeforeDispatch, 1_500, &changed),
            Err(AuthorityLeaseError::AuthorityChanged)
        );
        assert_eq!(
            lease.validate_at(AuthoritySafeBoundary::BeforeCommit, 1_500, &epochs),
            Err(AuthorityLeaseError::UndeclaredSafeBoundary(
                AuthoritySafeBoundary::BeforeCommit
            ))
        );
    }

    #[test]
    fn lease_is_exactly_bound_and_lifetime_is_bounded() {
        let epochs = epochs(Vec::new());
        let lease = AuthorityLease::new(
            binding(),
            &epochs,
            10,
            20,
            [AuthoritySafeBoundary::BeforeDispatch],
        )
        .unwrap();
        assert_eq!(lease.binding().principal_id(), "principal-1");
        assert_eq!(lease.binding().capability().as_wire(), "scope.operate");
        assert!(matches!(lease.binding().owner_scope(), OwnerScope::Team(_)));
        assert!(matches!(
            AuthorityLease::new(
                binding(),
                &epochs,
                10,
                10 + MAX_AUTHORITY_LEASE_MILLIS + 1,
                [AuthoritySafeBoundary::BeforeDispatch]
            ),
            Err(AuthorityContractError::InvalidLeaseLifetime)
        ));
    }
}
