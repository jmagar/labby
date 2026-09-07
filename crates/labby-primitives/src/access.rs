//! Stable, transport-neutral multi-user authority vocabulary.
//!
//! These types describe authority. They do not authenticate a caller or make
//! an authorization decision; higher layers must resolve current durable state.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessVocabularyError {
    EmptyIdentifier,
    InvalidIdentifier,
    EmptyActionPart,
    InvalidActionPart,
}

impl fmt::Display for AccessVocabularyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyIdentifier => "identifier must not be empty",
            Self::InvalidIdentifier => "identifier must be canonical printable text",
            Self::EmptyActionPart => "action service and name must not be empty",
            Self::InvalidActionPart => "action service and name must be canonical printable text",
        })
    }
}

impl std::error::Error for AccessVocabularyError {}

fn validate_identifier(value: &str) -> Result<(), AccessVocabularyError> {
    if value.trim().is_empty() {
        return Err(AccessVocabularyError::EmptyIdentifier);
    }
    if value != value.trim() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(AccessVocabularyError::InvalidIdentifier);
    }
    Ok(())
}

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, AccessVocabularyError> {
                let value = value.into();
                validate_identifier(&value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

opaque_id!(InstallationId);
opaque_id!(TeamId);
opaque_id!(ProjectId);
opaque_id!(PrincipalId);
opaque_id!(ResourceId);

/// Exactly one durable owner. Publication state is deliberately not an owner.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OwnerScope {
    Installation(InstallationId),
    Team(TeamId),
    Project(ProjectId),
    Personal(PrincipalId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OwnerKind {
    Installation,
    Team,
    Project,
    Personal,
}

impl OwnerScope {
    pub const fn kind(&self) -> OwnerKind {
        match self {
            Self::Installation(_) => OwnerKind::Installation,
            Self::Team(_) => OwnerKind::Team,
            Self::Project(_) => OwnerKind::Project,
            Self::Personal(_) => OwnerKind::Personal,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Installation(id) => id.as_str(),
            Self::Team(id) => id.as_str(),
            Self::Project(id) => id.as_str(),
            Self::Personal(id) => id.as_str(),
        }
    }
}

/// Publication/discovery state, independent from durable ownership.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PublicationVisibility {
    Private,
    Unlisted,
    Public,
}

impl PublicationVisibility {
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Unlisted => "unlisted",
            Self::Public => "public",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "private" => Some(Self::Private),
            "unlisted" => Some(Self::Unlisted),
            "public" => Some(Self::Public),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CapabilitySchemaVersion(u16);

impl CapabilitySchemaVersion {
    pub const V1: Self = Self(1);

    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    pub const fn supported(self) -> bool {
        self.0 == Self::V1.0
    }
}

/// Closed v1 capability vocabulary. Unknown wire values must deny.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Capability {
    PlatformRead,
    PlatformManage,
    ScopeRead,
    ScopeOperate,
    ScopeCreate,
    ScopeManage,
    ScopeDelete,
    MembershipManage,
    OwnershipTransfer,
    PolicyExplain,
    AuditRead,
}

impl Capability {
    pub const SCHEMA_VERSION: CapabilitySchemaVersion = CapabilitySchemaVersion::V1;

    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::PlatformRead => "platform.read",
            Self::PlatformManage => "platform.manage",
            Self::ScopeRead => "scope.read",
            Self::ScopeOperate => "scope.operate",
            Self::ScopeCreate => "scope.create",
            Self::ScopeManage => "scope.manage",
            Self::ScopeDelete => "scope.delete",
            Self::MembershipManage => "membership.manage",
            Self::OwnershipTransfer => "ownership.transfer",
            Self::PolicyExplain => "policy.explain",
            Self::AuditRead => "audit.read",
        }
    }

    pub fn from_wire(version: CapabilitySchemaVersion, value: &str) -> Option<Self> {
        if !version.supported() {
            return None;
        }
        match value {
            "platform.read" => Some(Self::PlatformRead),
            "platform.manage" => Some(Self::PlatformManage),
            "scope.read" => Some(Self::ScopeRead),
            "scope.operate" => Some(Self::ScopeOperate),
            "scope.create" => Some(Self::ScopeCreate),
            "scope.manage" => Some(Self::ScopeManage),
            "scope.delete" => Some(Self::ScopeDelete),
            "membership.manage" => Some(Self::MembershipManage),
            "ownership.transfer" => Some(Self::OwnershipTransfer),
            "policy.explain" => Some(Self::PolicyExplain),
            "audit.read" => Some(Self::AuditRead),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RoleTemplate {
    PlatformAdmin,
    TeamOwner,
    TeamAdmin,
    TeamMember,
    PersonalUser,
    ProjectOwner,
    ProjectAdmin,
    ProjectMember,
    ProjectViewer,
}

const PLATFORM_ADMIN: &[Capability] = &[
    Capability::PlatformRead,
    Capability::PlatformManage,
    Capability::ScopeRead,
    Capability::ScopeOperate,
    Capability::ScopeCreate,
    Capability::ScopeManage,
    Capability::ScopeDelete,
    Capability::MembershipManage,
    Capability::OwnershipTransfer,
    Capability::PolicyExplain,
    Capability::AuditRead,
];
const OWNER: &[Capability] = &[
    Capability::ScopeRead,
    Capability::ScopeOperate,
    Capability::ScopeCreate,
    Capability::ScopeManage,
    Capability::ScopeDelete,
    Capability::MembershipManage,
    Capability::OwnershipTransfer,
    Capability::PolicyExplain,
    Capability::AuditRead,
];
const ADMIN: &[Capability] = &[
    Capability::ScopeRead,
    Capability::ScopeOperate,
    Capability::ScopeCreate,
    Capability::ScopeManage,
    Capability::MembershipManage,
    Capability::PolicyExplain,
    Capability::AuditRead,
];
const MEMBER: &[Capability] = &[
    Capability::ScopeRead,
    Capability::ScopeOperate,
    Capability::ScopeCreate,
];
const PERSONAL: &[Capability] = &[
    Capability::ScopeRead,
    Capability::ScopeOperate,
    Capability::ScopeCreate,
    Capability::ScopeManage,
    Capability::ScopeDelete,
];
const VIEWER: &[Capability] = &[Capability::ScopeRead];

impl RoleTemplate {
    /// Returns `None` for an unsupported capability schema, which callers must
    /// treat as a denial rather than silently mapping to the newest schema.
    pub const fn capabilities(
        self,
        version: CapabilitySchemaVersion,
    ) -> Option<&'static [Capability]> {
        if !version.supported() {
            return None;
        }
        Some(match self {
            Self::PlatformAdmin => PLATFORM_ADMIN,
            Self::TeamOwner | Self::ProjectOwner => OWNER,
            Self::TeamAdmin | Self::ProjectAdmin => ADMIN,
            Self::TeamMember | Self::ProjectMember => MEMBER,
            Self::PersonalUser => PERSONAL,
            Self::ProjectViewer => VIEWER,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ResourceFamily {
    Platform,
    Library,
    Project,
    Gateway,
    Stash,
    Agent,
    Task,
    DevContainer,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResourceRef {
    owner: OwnerScope,
    family: ResourceFamily,
    id: ResourceId,
}

impl ResourceRef {
    pub const fn new(owner: OwnerScope, family: ResourceFamily, id: ResourceId) -> Self {
        Self { owner, family, id }
    }

    pub const fn owner(&self) -> &OwnerScope {
        &self.owner
    }

    pub const fn family(&self) -> ResourceFamily {
        self.family
    }

    pub const fn id(&self) -> &ResourceId {
        &self.id
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ActionRef {
    service: String,
    action: String,
}

impl ActionRef {
    pub fn new(
        service: impl Into<String>,
        action: impl Into<String>,
    ) -> Result<Self, AccessVocabularyError> {
        let service = service.into();
        let action = action.into();
        if service.trim().is_empty() || action.trim().is_empty() {
            return Err(AccessVocabularyError::EmptyActionPart);
        }
        if [&service, &action].iter().any(|value| {
            value.as_str() != value.trim()
                || value.len() > 256
                || value.chars().any(char::is_control)
        }) {
            return Err(AccessVocabularyError::InvalidActionPart);
        }
        Ok(Self { service, action })
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn action(&self) -> &str {
        &self.action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_scope_carries_exactly_one_typed_owner() {
        let scope = OwnerScope::Team(TeamId::new("team-1").unwrap());
        assert_eq!(scope.kind(), OwnerKind::Team);
        assert_eq!(scope.id(), "team-1");
    }

    #[test]
    fn publication_visibility_is_not_an_owner_kind() {
        assert_eq!(
            PublicationVisibility::from_wire("public"),
            Some(PublicationVisibility::Public)
        );
        assert_eq!(PublicationVisibility::Public.as_wire(), "public");
        assert_eq!(PublicationVisibility::from_wire("team"), None);
    }

    #[test]
    fn capabilities_fail_closed_for_unknown_versions_and_values() {
        assert_eq!(
            Capability::from_wire(CapabilitySchemaVersion::V1, "scope.read"),
            Some(Capability::ScopeRead)
        );
        assert_eq!(
            Capability::from_wire(CapabilitySchemaVersion::new(2), "scope.read"),
            None
        );
        assert_eq!(
            Capability::from_wire(CapabilitySchemaVersion::V1, "scope.root"),
            None
        );
    }

    #[test]
    fn fixed_role_templates_preserve_privilege_boundaries() {
        let capabilities =
            |role: RoleTemplate| role.capabilities(CapabilitySchemaVersion::V1).unwrap();
        assert!(capabilities(RoleTemplate::PlatformAdmin).contains(&Capability::PlatformManage));
        for role in [
            RoleTemplate::TeamOwner,
            RoleTemplate::TeamAdmin,
            RoleTemplate::TeamMember,
            RoleTemplate::PersonalUser,
            RoleTemplate::ProjectOwner,
            RoleTemplate::ProjectAdmin,
            RoleTemplate::ProjectMember,
            RoleTemplate::ProjectViewer,
        ] {
            assert!(!capabilities(role).contains(&Capability::PlatformManage));
        }
        assert!(capabilities(RoleTemplate::TeamOwner).contains(&Capability::OwnershipTransfer));
        assert!(!capabilities(RoleTemplate::TeamAdmin).contains(&Capability::OwnershipTransfer));
        assert!(!capabilities(RoleTemplate::TeamMember).contains(&Capability::MembershipManage));
        assert_eq!(
            RoleTemplate::TeamMember.capabilities(CapabilitySchemaVersion::new(9)),
            None
        );
    }

    #[test]
    fn action_and_resource_references_validate_contract_identity() {
        let action = ActionRef::new("gateway", "gateway.add").unwrap();
        assert_eq!(action.service(), "gateway");
        assert_eq!(action.action(), "gateway.add");
        assert!(ActionRef::new("gateway", " ").is_err());

        let resource = ResourceRef::new(
            OwnerScope::Personal(PrincipalId::new("principal-1").unwrap()),
            ResourceFamily::Gateway,
            ResourceId::new("gateway-1").unwrap(),
        );
        assert_eq!(resource.owner().kind(), OwnerKind::Personal);
        assert_eq!(resource.family(), ResourceFamily::Gateway);
        assert_eq!(resource.id().as_str(), "gateway-1");
    }
}
