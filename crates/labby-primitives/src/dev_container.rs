//! Transport-neutral Dev Container ownership and lifecycle vocabulary.

use std::collections::BTreeSet;
use std::fmt;

use crate::access::OwnerScope;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DevContainerContractError {
    EmptyIdentifier,
    InvalidIdentifier,
    InvalidImageDigest,
    InvalidLifecycleNonce,
    InvalidQuota,
    DuplicateSecretReference,
}

impl fmt::Display for DevContainerContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyIdentifier => "Dev Container identifier must not be empty",
            Self::InvalidIdentifier => "Dev Container identifier must be canonical printable text",
            Self::InvalidImageDigest => "Dev Container image must use a lowercase sha256 digest",
            Self::InvalidLifecycleNonce => {
                "Dev Container lifecycle nonce must contain at least 128 bits of canonical entropy"
            }
            Self::InvalidQuota => "Dev Container quota values must be non-zero",
            Self::DuplicateSecretReference => "Dev Container secret references must be unique",
        })
    }
}

impl std::error::Error for DevContainerContractError {}

fn validate_identifier(value: &str) -> Result<(), DevContainerContractError> {
    if value.trim().is_empty() {
        return Err(DevContainerContractError::EmptyIdentifier);
    }
    if value != value.trim() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(DevContainerContractError::InvalidIdentifier);
    }
    Ok(())
}

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, DevContainerContractError> {
                let value = value.into();
                validate_identifier(&value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

identifier!(DevContainerId);
identifier!(DevContainerTemplateId);
identifier!(SecretReference);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LifecycleNonce(String);

impl LifecycleNonce {
    pub fn new(value: impl Into<String>) -> Result<Self, DevContainerContractError> {
        let value = value.into();
        if !(32..=128).contains(&value.len())
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(DevContainerContractError::InvalidLifecycleNonce);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ImageDigest(String);

impl ImageDigest {
    pub fn new(value: impl Into<String>) -> Result<Self, DevContainerContractError> {
        let value = value.into();
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err(DevContainerContractError::InvalidImageDigest);
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(DevContainerContractError::InvalidImageDigest);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HostCapability {
    Privileged,
    HostFilesystem,
    ContainerRuntimeSocket,
    HostNetwork,
    HostDevice,
    KernelAdministration,
}

/// An empty set is the default. A template must explicitly name every host
/// capability it is allowed to request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HostCapabilityPolicy {
    approved: BTreeSet<HostCapability>,
}

impl HostCapabilityPolicy {
    pub fn deny_all() -> Self {
        Self::default()
    }

    pub fn approved(capabilities: impl IntoIterator<Item = HostCapability>) -> Self {
        Self {
            approved: capabilities.into_iter().collect(),
        }
    }

    pub fn allows(&self, capability: HostCapability) -> bool {
        self.approved.contains(&capability)
    }

    pub fn allows_all(&self, requested: &BTreeSet<HostCapability>) -> bool {
        requested.is_subset(&self.approved)
    }

    pub fn values(&self) -> &BTreeSet<HostCapability> {
        &self.approved
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DevContainerQuota {
    pub max_active_instances: u32,
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub max_lifetime_seconds: u64,
}

impl DevContainerQuota {
    pub fn validate(self) -> Result<Self, DevContainerContractError> {
        if self.max_active_instances == 0
            || self.cpu_millis == 0
            || self.memory_bytes == 0
            || self.disk_bytes == 0
            || self.max_lifetime_seconds == 0
        {
            return Err(DevContainerContractError::InvalidQuota);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesiredState {
    Running,
    Stopped,
    Deleted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservedState {
    Pending,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovedTemplate {
    id: DevContainerTemplateId,
    image: ImageDigest,
    quota_ceiling: DevContainerQuota,
    host_capabilities: HostCapabilityPolicy,
}

impl ApprovedTemplate {
    pub fn new(
        id: DevContainerTemplateId,
        image: ImageDigest,
        quota_ceiling: DevContainerQuota,
        host_capabilities: HostCapabilityPolicy,
    ) -> Result<Self, DevContainerContractError> {
        Ok(Self {
            id,
            image,
            quota_ceiling: quota_ceiling.validate()?,
            host_capabilities,
        })
    }

    pub fn id(&self) -> &DevContainerTemplateId {
        &self.id
    }

    pub fn image(&self) -> &ImageDigest {
        &self.image
    }

    pub const fn quota_ceiling(&self) -> DevContainerQuota {
        self.quota_ceiling
    }

    pub fn host_capabilities(&self) -> &HostCapabilityPolicy {
        &self.host_capabilities
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedDevContainer {
    id: DevContainerId,
    owner: OwnerScope,
    template_id: DevContainerTemplateId,
    image: ImageDigest,
    lifecycle_nonce: LifecycleNonce,
    desired_state: DesiredState,
    observed_state: ObservedState,
    secret_references: Vec<SecretReference>,
}

impl OwnedDevContainer {
    pub fn new(
        id: DevContainerId,
        owner: OwnerScope,
        template: &ApprovedTemplate,
        lifecycle_nonce: LifecycleNonce,
        secret_references: Vec<SecretReference>,
    ) -> Result<Self, DevContainerContractError> {
        let unique = secret_references.iter().collect::<BTreeSet<_>>();
        if unique.len() != secret_references.len() {
            return Err(DevContainerContractError::DuplicateSecretReference);
        }
        Ok(Self {
            id,
            owner,
            template_id: template.id.clone(),
            image: template.image.clone(),
            lifecycle_nonce,
            desired_state: DesiredState::Running,
            observed_state: ObservedState::Pending,
            secret_references,
        })
    }

    pub fn id(&self) -> &DevContainerId {
        &self.id
    }

    pub fn owner(&self) -> &OwnerScope {
        &self.owner
    }

    pub fn template_id(&self) -> &DevContainerTemplateId {
        &self.template_id
    }

    pub fn image(&self) -> &ImageDigest {
        &self.image
    }

    pub fn lifecycle_nonce(&self) -> &LifecycleNonce {
        &self.lifecycle_nonce
    }

    pub const fn desired_state(&self) -> DesiredState {
        self.desired_state
    }

    pub const fn observed_state(&self) -> ObservedState {
        self.observed_state
    }

    pub fn secret_references(&self) -> &[SecretReference] {
        &self.secret_references
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{OwnerKind, PrincipalId};

    fn quota() -> DevContainerQuota {
        DevContainerQuota {
            max_active_instances: 2,
            cpu_millis: 2_000,
            memory_bytes: 2 * 1024 * 1024 * 1024,
            disk_bytes: 20 * 1024 * 1024 * 1024,
            max_lifetime_seconds: 3_600,
        }
    }

    #[test]
    fn images_are_immutable_digest_references() {
        assert!(ImageDigest::new(format!("sha256:{}", "a".repeat(64))).is_ok());
        assert!(ImageDigest::new("ubuntu:latest").is_err());
        assert!(ImageDigest::new(format!("sha256:{}", "A".repeat(64))).is_err());
    }

    #[test]
    fn host_access_is_default_denied_and_explicitly_bounded() {
        let denied = HostCapabilityPolicy::deny_all();
        assert!(!denied.allows(HostCapability::Privileged));
        let approved = HostCapabilityPolicy::approved([HostCapability::HostNetwork]);
        assert!(approved.allows(HostCapability::HostNetwork));
        assert!(!approved.allows(HostCapability::HostFilesystem));
    }

    #[test]
    fn owned_instances_pin_template_image_nonce_and_secret_references() {
        let template = ApprovedTemplate::new(
            DevContainerTemplateId::new("rust-stable").unwrap(),
            ImageDigest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
            quota(),
            HostCapabilityPolicy::deny_all(),
        )
        .unwrap();
        let instance = OwnedDevContainer::new(
            DevContainerId::new("dc-1").unwrap(),
            OwnerScope::Personal(PrincipalId::new("principal-1").unwrap()),
            &template,
            LifecycleNonce::new("0123456789abcdef0123456789abcdef").unwrap(),
            vec![SecretReference::new("secret-ref-1").unwrap()],
        )
        .unwrap();
        assert_eq!(instance.owner().kind(), OwnerKind::Personal);
        assert_eq!(instance.image(), template.image());
        assert_eq!(instance.desired_state(), DesiredState::Running);
        assert_eq!(instance.observed_state(), ObservedState::Pending);
        assert_eq!(instance.secret_references().len(), 1);
    }

    #[test]
    fn duplicate_secret_references_and_zero_quotas_are_rejected() {
        let reference = SecretReference::new("secret-ref-1").unwrap();
        let template = ApprovedTemplate::new(
            DevContainerTemplateId::new("rust-stable").unwrap(),
            ImageDigest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
            quota(),
            HostCapabilityPolicy::deny_all(),
        )
        .unwrap();
        assert_eq!(
            OwnedDevContainer::new(
                DevContainerId::new("dc-1").unwrap(),
                OwnerScope::Personal(PrincipalId::new("principal-1").unwrap()),
                &template,
                LifecycleNonce::new("0123456789abcdef0123456789abcdef").unwrap(),
                vec![reference.clone(), reference],
            ),
            Err(DevContainerContractError::DuplicateSecretReference)
        );
        assert_eq!(
            DevContainerQuota {
                max_active_instances: 0,
                ..quota()
            }
            .validate(),
            Err(DevContainerContractError::InvalidQuota)
        );
    }
}
