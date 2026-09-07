use std::fmt;

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        pub(super) struct $name(String);

        impl $name {
            pub(super) fn new(value: impl Into<String>) -> Result<Self, DomainError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(DomainError::EmptyId);
                }
                Ok(Self(value))
            }

            pub(super) fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

opaque_id!(PrincipalId);
opaque_id!(OrganizationId);
opaque_id!(ProjectId);
opaque_id!(TeamId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DomainError {
    EmptyId,
    EmptyLoadoutName,
    EmptyTeamName,
    InvalidLoadoutName,
    InvalidTeamName,
    OrganizationMismatch,
    EpochExhausted,
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyId => "identifier must not be empty",
            Self::EmptyLoadoutName => "loadout name must not be empty",
            Self::EmptyTeamName => "team name must not be empty",
            Self::InvalidLoadoutName => "loadout name must be canonical printable text",
            Self::InvalidTeamName => "team name must be canonical printable text",
            Self::OrganizationMismatch => "access-control records must share an organization",
            Self::EpochExhausted => "authority epoch is exhausted",
        })
    }
}

fn validate_team_name(name: &str) -> Result<(), DomainError> {
    if name.trim().is_empty() {
        return Err(DomainError::EmptyTeamName);
    }
    if name != name.trim() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(DomainError::InvalidTeamName);
    }
    Ok(())
}

/// Monotonic generation for one durable authority record or policy boundary.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct AuthorityEpoch(u64);

impl AuthorityEpoch {
    pub(super) const INITIAL: Self = Self(0);

    pub(super) const fn get(self) -> u64 {
        self.0
    }

    pub(super) fn from_persisted(value: i64) -> Option<Self> {
        u64::try_from(value).ok().map(Self)
    }

    pub(super) fn advance(&mut self) -> Result<Self, DomainError> {
        self.0 = self.0.checked_add(1).ok_or(DomainError::EpochExhausted)?;
        Ok(*self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlatformRole {
    Administrator,
}

impl PlatformRole {
    pub(super) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Administrator => "platform_admin",
        }
    }

    pub(super) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "platform_admin" => Some(Self::Administrator),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TeamRole {
    Owner,
    Admin,
    Member,
}

impl TeamRole {
    pub(super) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }

    pub(crate) const fn as_wire(self) -> &'static str {
        self.as_persisted()
    }

    pub(super) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TeamStatus {
    Active,
    Suspended,
    DeletionPending,
}

impl TeamStatus {
    pub(super) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::DeletionPending => "deletion_pending",
        }
    }

    pub(super) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            "deletion_pending" => Some(Self::DeletionPending),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MembershipStatus {
    Active,
    Suspended,
}

impl MembershipStatus {
    pub(super) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }

    pub(super) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Team {
    id: TeamId,
    organization_id: OrganizationId,
    name: String,
    status: TeamStatus,
    policy_epoch: AuthorityEpoch,
    membership_epoch: AuthorityEpoch,
}

impl Team {
    pub(super) fn new(
        id: TeamId,
        organization_id: OrganizationId,
        name: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        validate_team_name(&name)?;
        Ok(Self {
            id,
            organization_id,
            name,
            status: TeamStatus::Active,
            policy_epoch: AuthorityEpoch::INITIAL,
            membership_epoch: AuthorityEpoch::INITIAL,
        })
    }

    pub(super) fn id(&self) -> &TeamId {
        &self.id
    }

    pub(super) fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) const fn status(&self) -> TeamStatus {
        self.status
    }

    pub(super) const fn policy_epoch(&self) -> AuthorityEpoch {
        self.policy_epoch
    }

    pub(super) const fn membership_epoch(&self) -> AuthorityEpoch {
        self.membership_epoch
    }

    pub(super) fn set_status(&mut self, status: TeamStatus) -> Result<(), DomainError> {
        if self.status != status {
            self.policy_epoch.advance()?;
            self.status = status;
        }
        Ok(())
    }

    pub(super) fn note_membership_change(&mut self) -> Result<(), DomainError> {
        self.membership_epoch.advance()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TeamMembership {
    organization_id: OrganizationId,
    team_id: TeamId,
    principal_id: PrincipalId,
    role: TeamRole,
    status: MembershipStatus,
    authority_epoch: AuthorityEpoch,
}

impl TeamMembership {
    pub(super) fn new(
        principal: &Principal,
        team: &Team,
        role: TeamRole,
    ) -> Result<Self, DomainError> {
        if principal.organization_id() != team.organization_id() {
            return Err(DomainError::OrganizationMismatch);
        }
        Ok(Self {
            organization_id: team.organization_id().clone(),
            team_id: team.id().clone(),
            principal_id: principal.id().clone(),
            role,
            status: MembershipStatus::Active,
            authority_epoch: AuthorityEpoch::INITIAL,
        })
    }

    pub(super) fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    pub(super) fn team_id(&self) -> &TeamId {
        &self.team_id
    }

    pub(super) fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }

    pub(super) const fn role(&self) -> TeamRole {
        self.role
    }

    pub(super) const fn status(&self) -> MembershipStatus {
        self.status
    }

    pub(super) const fn authority_epoch(&self) -> AuthorityEpoch {
        self.authority_epoch
    }

    pub(super) fn set_role(&mut self, role: TeamRole) -> Result<(), DomainError> {
        if self.role != role {
            self.authority_epoch.advance()?;
            self.role = role;
        }
        Ok(())
    }

    pub(super) fn set_status(&mut self, status: MembershipStatus) -> Result<(), DomainError> {
        if self.status != status {
            self.authority_epoch.advance()?;
            self.status = status;
        }
        Ok(())
    }
}

impl std::error::Error for DomainError {}

pub(super) fn validate_loadout_name(loadout_name: &str) -> Result<(), DomainError> {
    if loadout_name.trim().is_empty() {
        return Err(DomainError::EmptyLoadoutName);
    }
    if loadout_name != loadout_name.trim()
        || loadout_name.len() > 128
        || loadout_name.chars().any(char::is_control)
    {
        return Err(DomainError::InvalidLoadoutName);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Organization {
    id: OrganizationId,
}

impl Organization {
    pub(super) fn new(id: OrganizationId) -> Self {
        Self { id }
    }

    pub(super) fn id(&self) -> &OrganizationId {
        &self.id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Principal {
    id: PrincipalId,
    organization_id: OrganizationId,
}

impl Principal {
    pub(super) fn new(id: PrincipalId, organization_id: OrganizationId) -> Self {
        Self {
            id,
            organization_id,
        }
    }

    pub(super) fn id(&self) -> &PrincipalId {
        &self.id
    }

    pub(super) fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Project {
    id: ProjectId,
    organization_id: OrganizationId,
}

impl Project {
    pub(super) fn new(id: ProjectId, organization_id: OrganizationId) -> Self {
        Self {
            id,
            organization_id,
        }
    }

    pub(super) fn id(&self) -> &ProjectId {
        &self.id
    }

    pub(super) fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Permission {
    ProjectRead,
    ProjectManage,
    AssetDiscover,
    AssetUse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl ProjectRole {
    const ADMIN_PERMISSIONS: [Permission; 4] = [
        Permission::ProjectRead,
        Permission::ProjectManage,
        Permission::AssetDiscover,
        Permission::AssetUse,
    ];
    const MEMBER_PERMISSIONS: [Permission; 3] = [
        Permission::ProjectRead,
        Permission::AssetDiscover,
        Permission::AssetUse,
    ];
    const VIEWER_PERMISSIONS: [Permission; 2] =
        [Permission::ProjectRead, Permission::AssetDiscover];

    pub(super) const fn permissions(self) -> &'static [Permission] {
        match self {
            Self::Owner | Self::Admin => &Self::ADMIN_PERMISSIONS,
            Self::Member => &Self::MEMBER_PERMISSIONS,
            Self::Viewer => &Self::VIEWER_PERMISSIONS,
        }
    }

    pub(super) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    pub(super) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    pub(crate) const fn as_wire(self) -> &'static str {
        self.as_persisted()
    }

    pub(super) const fn precedence(self) -> u8 {
        match self {
            Self::Owner => 4,
            Self::Admin => 3,
            Self::Member => 2,
            Self::Viewer => 1,
        }
    }

    pub(super) const fn max(self, other: Self) -> Self {
        if self.precedence() >= other.precedence() {
            self
        } else {
            other
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InvitationStatus {
    Pending,
    Accepted,
    Revoked,
    Expired,
}

impl InvitationStatus {
    pub(super) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
        }
    }

    pub(super) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            "revoked" => Some(Self::Revoked),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ProjectMembership {
    organization_id: OrganizationId,
    principal_id: PrincipalId,
    project_id: ProjectId,
    role: ProjectRole,
}

impl ProjectMembership {
    pub(super) fn new(
        principal: &Principal,
        project: &Project,
        role: ProjectRole,
    ) -> Result<Self, DomainError> {
        if principal.organization_id() != project.organization_id() {
            return Err(DomainError::OrganizationMismatch);
        }
        Ok(Self {
            organization_id: project.organization_id().clone(),
            principal_id: principal.id().clone(),
            project_id: project.id().clone(),
            role,
        })
    }

    pub(super) fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    pub(super) fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }

    pub(super) fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    pub(super) const fn role(&self) -> ProjectRole {
        self.role
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ProjectLoadout {
    organization_id: OrganizationId,
    project_id: ProjectId,
    loadout_name: String,
}

impl ProjectLoadout {
    pub(super) fn new(
        project: &Project,
        loadout_name: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let loadout_name = loadout_name.into();
        validate_loadout_name(&loadout_name)?;
        Ok(Self {
            organization_id: project.organization_id().clone(),
            project_id: project.id().clone(),
            loadout_name,
        })
    }

    pub(super) fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    pub(super) fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    pub(super) fn loadout_name(&self) -> &str {
        &self.loadout_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_ids_reject_empty_values_without_normalizing_valid_values() {
        assert!(PrincipalId::new("").is_err());
        assert!(OrganizationId::new("  ").is_err());
        assert!(ProjectId::new("\n").is_err());
        assert!(TeamId::new("\t").is_err());

        let id = PrincipalId::new(" Principal-A ").expect("non-empty ID");
        assert_eq!(id.as_str(), " Principal-A ");
    }

    #[test]
    fn platform_and_team_role_vocabulary_is_closed() {
        assert_eq!(
            PlatformRole::from_persisted("platform_admin"),
            Some(PlatformRole::Administrator)
        );
        assert_eq!(PlatformRole::Administrator.as_persisted(), "platform_admin");
        assert_eq!(PlatformRole::from_persisted("admin"), None);

        for role in [TeamRole::Owner, TeamRole::Admin, TeamRole::Member] {
            assert_eq!(TeamRole::from_persisted(role.as_persisted()), Some(role));
        }
        assert_eq!(TeamRole::from_persisted("viewer"), None);
    }

    #[test]
    fn team_and_membership_status_vocabulary_is_closed() {
        for status in [
            TeamStatus::Active,
            TeamStatus::Suspended,
            TeamStatus::DeletionPending,
        ] {
            assert_eq!(
                TeamStatus::from_persisted(status.as_persisted()),
                Some(status)
            );
        }
        assert_eq!(TeamStatus::from_persisted("deleted"), None);

        for status in [MembershipStatus::Active, MembershipStatus::Suspended] {
            assert_eq!(
                MembershipStatus::from_persisted(status.as_persisted()),
                Some(status)
            );
        }
        assert_eq!(MembershipStatus::from_persisted("removed"), None);
    }

    #[test]
    fn teams_require_canonical_names_and_start_active_at_epoch_zero() {
        let organization = Organization::new(OrganizationId::new("org-a").unwrap());
        assert!(
            Team::new(
                TeamId::new("team-a").unwrap(),
                organization.id().clone(),
                "  "
            )
            .is_err()
        );
        assert!(
            Team::new(
                TeamId::new("team-a").unwrap(),
                organization.id().clone(),
                " Team A "
            )
            .is_err()
        );
        let team = Team::new(
            TeamId::new("team-a").unwrap(),
            organization.id().clone(),
            "Team A",
        )
        .unwrap();
        assert_eq!(team.name(), "Team A");
        assert_eq!(team.status(), TeamStatus::Active);
        assert_eq!(team.policy_epoch().get(), 0);
        assert_eq!(team.membership_epoch().get(), 0);
    }

    #[test]
    fn team_membership_requires_one_organization() {
        let org_a = Organization::new(OrganizationId::new("org-a").unwrap());
        let org_b = Organization::new(OrganizationId::new("org-b").unwrap());
        let principal = Principal::new(PrincipalId::new("alice").unwrap(), org_a.id().clone());
        let team = Team::new(TeamId::new("team-b").unwrap(), org_b.id().clone(), "Team B").unwrap();

        assert_eq!(
            TeamMembership::new(&principal, &team, TeamRole::Member),
            Err(DomainError::OrganizationMismatch)
        );
    }

    #[test]
    fn authority_changes_advance_only_their_owned_epoch() {
        let organization = Organization::new(OrganizationId::new("org-a").unwrap());
        let principal = Principal::new(
            PrincipalId::new("alice").unwrap(),
            organization.id().clone(),
        );
        let mut team = Team::new(
            TeamId::new("team-a").unwrap(),
            organization.id().clone(),
            "Team A",
        )
        .unwrap();
        let mut membership = TeamMembership::new(&principal, &team, TeamRole::Member).unwrap();

        team.set_status(TeamStatus::Active).unwrap();
        assert_eq!(team.policy_epoch().get(), 0);
        team.set_status(TeamStatus::Suspended).unwrap();
        assert_eq!(team.policy_epoch().get(), 1);
        team.note_membership_change().unwrap();
        assert_eq!(team.membership_epoch().get(), 1);

        membership.set_role(TeamRole::Member).unwrap();
        assert_eq!(membership.authority_epoch().get(), 0);
        membership.set_role(TeamRole::Admin).unwrap();
        membership.set_status(MembershipStatus::Suspended).unwrap();
        assert_eq!(membership.authority_epoch().get(), 2);
        assert_eq!(membership.organization_id(), organization.id());
        assert_eq!(membership.team_id(), team.id());
        assert_eq!(membership.principal_id(), principal.id());
        assert_eq!(membership.role(), TeamRole::Admin);
        assert_eq!(membership.status(), MembershipStatus::Suspended);
    }

    #[test]
    fn authority_epochs_reject_negative_storage_and_overflow() {
        assert_eq!(AuthorityEpoch::from_persisted(-1), None);
        assert_eq!(AuthorityEpoch::from_persisted(7).unwrap().get(), 7);
        let mut exhausted = AuthorityEpoch(u64::MAX);
        assert_eq!(exhausted.advance(), Err(DomainError::EpochExhausted));
        assert_eq!(exhausted.get(), u64::MAX);
    }

    #[test]
    fn project_membership_requires_the_principal_and_project_to_share_an_organization() {
        let org_a = Organization::new(OrganizationId::new("org-a").unwrap());
        let org_b = Organization::new(OrganizationId::new("org-b").unwrap());
        let principal = Principal::new(PrincipalId::new("alice").unwrap(), org_a.id().clone());
        let project = Project::new(ProjectId::new("phoenix").unwrap(), org_b.id().clone());

        let error = ProjectMembership::new(&principal, &project, ProjectRole::Member).unwrap_err();
        assert_eq!(error, DomainError::OrganizationMismatch);
    }

    #[test]
    fn project_membership_retains_its_organization_identity() {
        let organization = Organization::new(OrganizationId::new("org-a").unwrap());
        let principal = Principal::new(
            PrincipalId::new("alice").unwrap(),
            organization.id().clone(),
        );
        let project = Project::new(
            ProjectId::new("phoenix").unwrap(),
            organization.id().clone(),
        );

        let membership = ProjectMembership::new(&principal, &project, ProjectRole::Member).unwrap();
        assert_eq!(membership.organization_id(), organization.id());
    }

    #[test]
    fn project_roles_expand_to_the_milestone_one_permission_set() {
        assert_eq!(
            ProjectRole::Owner.permissions(),
            &[
                Permission::ProjectRead,
                Permission::ProjectManage,
                Permission::AssetDiscover,
                Permission::AssetUse,
            ]
        );
        assert_eq!(
            ProjectRole::Admin.permissions(),
            &[
                Permission::ProjectRead,
                Permission::ProjectManage,
                Permission::AssetDiscover,
                Permission::AssetUse,
            ]
        );
        assert_eq!(
            ProjectRole::Member.permissions(),
            &[
                Permission::ProjectRead,
                Permission::AssetDiscover,
                Permission::AssetUse,
            ]
        );
        assert_eq!(
            ProjectRole::Viewer.permissions(),
            &[Permission::ProjectRead, Permission::AssetDiscover]
        );
    }

    #[test]
    fn project_role_precedence_is_explicit_and_stable() {
        assert_eq!(
            ProjectRole::Viewer.max(ProjectRole::Member),
            ProjectRole::Member
        );
        assert_eq!(
            ProjectRole::Member.max(ProjectRole::Admin),
            ProjectRole::Admin
        );
        assert_eq!(
            ProjectRole::Admin.max(ProjectRole::Owner),
            ProjectRole::Owner
        );
        assert_eq!(ProjectRole::Owner.as_persisted(), "owner");
    }

    #[test]
    fn a_project_has_at_most_one_non_empty_named_loadout_mapping() {
        let organization_id = OrganizationId::new("engineering").unwrap();
        let project = Project::new(ProjectId::new("phoenix").unwrap(), organization_id.clone());
        assert_eq!(
            ProjectLoadout::new(&project, "  ").unwrap_err(),
            DomainError::EmptyLoadoutName
        );

        let mapping = ProjectLoadout::new(&project, "production").unwrap();
        assert_eq!(mapping.organization_id(), &organization_id);
        assert_eq!(mapping.project_id(), project.id());
        assert_eq!(mapping.loadout_name(), "production");
    }
}
