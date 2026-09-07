//! Tenant-qualified Gateway runtime identity and redacted credential custody.

use labby_primitives::access::OwnerScope;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GatewayAuthorityKey {
    pub owner: OwnerScope,
    pub project_id: Option<String>,
    pub loadout: String,
    pub authority_epoch: u64,
    pub credential_generation: u64,
}

impl GatewayAuthorityKey {
    #[must_use]
    pub fn partition_key(&self) -> String {
        // This value is routinely used in cache and pool keys. Hash the
        // length-delimited authority tuple so logs/debug output cannot disclose
        // tenant, project, loadout, or credential metadata and so differently
        // typed owners with the same textual id never alias.
        let owner_kind: &[u8] = match self.owner.kind() {
            labby_primitives::access::OwnerKind::Installation => b"installation",
            labby_primitives::access::OwnerKind::Team => b"team",
            labby_primitives::access::OwnerKind::Project => b"project",
            labby_primitives::access::OwnerKind::Personal => b"personal",
        };
        let mut digest = Sha256::new();
        for field in [
            owner_kind,
            self.owner.id().as_bytes(),
            self.project_id.as_deref().unwrap_or("").as_bytes(),
            self.loadout.as_bytes(),
            &self.authority_epoch.to_be_bytes(),
            &self.credential_generation.to_be_bytes(),
        ] {
            digest.update((field.len() as u64).to_be_bytes());
            digest.update(field);
        }
        let digest = digest.finalize();
        let mut encoded = String::with_capacity(3 + digest.len() * 2);
        encoded.push_str("g1:");
        for byte in digest {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
        }
        encoded
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamCredentialStatus {
    Active,
    Revoked,
}

/// Metadata only: secret bytes remain in the host credential store.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TeamCredentialBinding {
    pub binding_id: String,
    pub team_id: String,
    pub upstream_name: String,
    pub custodian_principal_id: String,
    pub generation: u64,
    pub rotated_at_millis: u64,
    pub status: TeamCredentialStatus,
}

impl TeamCredentialBinding {
    #[must_use]
    pub const fn usable(&self, expected_generation: u64) -> bool {
        matches!(self.status, TeamCredentialStatus::Active)
            && self.generation == expected_generation
    }

    /// Reject malformed metadata before it can become a cache or policy key.
    /// Secret material is intentionally absent from this projection.
    pub fn validate(&self) -> bool {
        self.generation > 0
            && self.rotated_at_millis > 0
            && [
                self.binding_id.as_str(),
                self.team_id.as_str(),
                self.upstream_name.as_str(),
                self.custodian_principal_id.as_str(),
            ]
            .into_iter()
            .all(|value| {
                !value.is_empty()
                    && value.trim() == value
                    && value.len() <= 256
                    && !value.chars().any(char::is_control)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_primitives::access::{OwnerScope, TeamId};

    #[test]
    fn partition_changes_at_authority_boundaries() {
        let base = GatewayAuthorityKey {
            owner: OwnerScope::Team(TeamId::new("a").unwrap()),
            project_id: Some("p".into()),
            loadout: "l".into(),
            authority_epoch: 1,
            credential_generation: 1,
        };
        let mut rotated = base.clone();
        rotated.credential_generation = 2;
        let mut revoked = base.clone();
        revoked.authority_epoch = 2;
        assert_ne!(base.partition_key(), rotated.partition_key());
        assert_ne!(base.partition_key(), revoked.partition_key());
        assert!(!base.partition_key().contains("default"));
    }

    #[test]
    fn credential_projection_contains_no_secret_material() {
        let binding = TeamCredentialBinding {
            binding_id: "b".into(),
            team_id: "a".into(),
            upstream_name: "u".into(),
            custodian_principal_id: "p".into(),
            generation: 3,
            rotated_at_millis: 4,
            status: TeamCredentialStatus::Active,
        };
        let json = serde_json::to_string(&binding).unwrap();
        assert!(!json.contains("token"));
        assert!(binding.usable(3));
        assert!(!binding.usable(2));
        assert!(binding.validate());
        assert!(serde_json::from_str::<TeamCredentialBinding>(
            r#"{"binding_id":"b","team_id":"a","upstream_name":"u","custodian_principal_id":"p","generation":3,"rotated_at_millis":4,"status":"active","token":"secret"}"#
        )
        .is_err());
    }

    #[test]
    fn typed_owner_kind_participates_in_partition_identity() {
        use labby_primitives::access::{PrincipalId, TeamId};
        let key = |owner| GatewayAuthorityKey {
            owner,
            project_id: None,
            loadout: "default".into(),
            authority_epoch: 1,
            credential_generation: 1,
        };
        assert_ne!(
            key(OwnerScope::Team(TeamId::new("same").unwrap())).partition_key(),
            key(OwnerScope::Personal(PrincipalId::new("same").unwrap())).partition_key()
        );
    }
}
