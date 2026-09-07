//! Transport-neutral Agent definition, revision, and session contracts.

use crate::access::{Capability, OwnerScope, PrincipalId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentState {
    Active,
    Suspended,
    Deleted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunningRevocationPolicy {
    StopAtSafeBoundary,
    StopImmediately,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRevision {
    pub version: u64,
    pub content_digest: String,
    pub repository_digest: String,
    pub image_digest: String,
    pub harness_digest: String,
    pub loadout_digest: String,
    pub catalog_generation: String,
    pub credential_references: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentDefinition {
    pub id: String,
    pub owner: OwnerScope,
    pub revision: AgentRevision,
    pub state: AgentState,
    pub required_capabilities: Vec<Capability>,
    pub authority_epoch: u64,
    pub publication_epoch: u64,
    pub revocation_policy: RunningRevocationPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSessionBinding {
    pub session_id: String,
    pub agent_id: String,
    pub agent_version: u64,
    pub principal: PrincipalId,
    pub owner: OwnerScope,
    pub authority_fingerprint: String,
    pub lease_expires_at: i64,
    pub catalog_generation: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentContractError {
    InvalidIdentifier,
    InvalidDigest,
    InvalidVersion,
    UnboundedCredentials,
}

impl AgentRevision {
    pub fn validate(&self) -> Result<(), AgentContractError> {
        if self.version == 0 {
            return Err(AgentContractError::InvalidVersion);
        }
        for digest in [
            &self.content_digest,
            &self.repository_digest,
            &self.image_digest,
            &self.harness_digest,
            &self.loadout_digest,
        ] {
            let valid = digest.strip_prefix("sha256:").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
            if !valid {
                return Err(AgentContractError::InvalidDigest);
            }
        }
        if self.credential_references.len() > 64
            || self.credential_references.iter().any(|v| !valid_id(v))
        {
            return Err(AgentContractError::UnboundedCredentials);
        }
        if !valid_id(&self.catalog_generation) {
            return Err(AgentContractError::InvalidIdentifier);
        }
        Ok(())
    }
}

impl AgentDefinition {
    pub fn validate(&self) -> Result<(), AgentContractError> {
        if !valid_id(&self.id) {
            return Err(AgentContractError::InvalidIdentifier);
        }
        self.revision.validate()
    }

    pub fn dispatchable(&self, current_epoch: u64, capabilities: &[Capability]) -> bool {
        self.state == AgentState::Active
            && self.authority_epoch == current_epoch
            && self
                .required_capabilities
                .iter()
                .all(|c| capabilities.contains(c))
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{OwnerScope, TeamId};

    fn digest() -> String {
        format!("sha256:{}", "a".repeat(64))
    }
    #[test]
    fn execution_is_pinned_and_revocation_denies_future_dispatch() {
        let mut definition = AgentDefinition {
            id: "agent-1".into(),
            owner: OwnerScope::Team(TeamId::new("team-1").unwrap()),
            revision: AgentRevision {
                version: 1,
                content_digest: digest(),
                repository_digest: digest(),
                image_digest: digest(),
                harness_digest: digest(),
                loadout_digest: digest(),
                catalog_generation: "catalog-1".into(),
                credential_references: vec!["credential-1".into()],
            },
            state: AgentState::Active,
            required_capabilities: vec![Capability::ScopeOperate],
            authority_epoch: 7,
            publication_epoch: 1,
            revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
        };
        definition.validate().unwrap();
        assert!(definition.dispatchable(7, &[Capability::ScopeOperate]));
        definition.state = AgentState::Suspended;
        assert!(!definition.dispatchable(7, &[Capability::ScopeOperate]));
        assert!(!definition.dispatchable(6, &[Capability::ScopeOperate]));
    }
}
