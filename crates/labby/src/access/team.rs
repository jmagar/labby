use labby_auth::VerifiedIdentity;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};

use super::domain::{ProjectRole, TeamRole};
use super::error::{AccessStoreError, AccessStoreResult};
use super::read::resolve_principal;
use super::store::map_sqlite_error;

const MAX_NAME: usize = 128;
const MAX_ID: usize = 128;
const MAX_INVITATION_TTL_SECONDS: i64 = 7 * 24 * 60 * 60;

#[derive(Clone, Debug)]
pub(crate) struct CreateTeamInput {
    actor: VerifiedIdentity,
    team_id: String,
    name: String,
}

impl CreateTeamInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        team_id: impl Into<String>,
        name: impl Into<String>,
    ) -> AccessStoreResult<Self> {
        let team_id = team_id.into();
        let name = name.into();
        if !valid_id(&team_id) || !valid_name(&name) {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            team_id,
            name,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AddTeamMemberInput {
    actor: VerifiedIdentity,
    team_id: String,
    principal_id: String,
    role: TeamRole,
}

impl AddTeamMemberInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        team_id: impl Into<String>,
        principal_id: impl Into<String>,
        role: TeamRole,
    ) -> AccessStoreResult<Self> {
        let team_id = team_id.into();
        let principal_id = principal_id.into();
        if !valid_id(&team_id) || !valid_id(&principal_id) {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            team_id,
            principal_id,
            role,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TeamMembershipInput {
    actor: VerifiedIdentity,
    team_id: String,
    principal_id: String,
}

impl TeamMembershipInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        team_id: impl Into<String>,
        principal_id: impl Into<String>,
    ) -> AccessStoreResult<Self> {
        let team_id = team_id.into();
        let principal_id = principal_id.into();
        if !valid_id(&team_id) || !valid_id(&principal_id) {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            team_id,
            principal_id,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PlatformAdministratorInput {
    actor: VerifiedIdentity,
    principal_id: String,
}

impl PlatformAdministratorInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        principal_id: impl Into<String>,
    ) -> AccessStoreResult<Self> {
        let principal_id = principal_id.into();
        if !valid_id(&principal_id) {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            principal_id,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeamSnapshot {
    pub(crate) organization_id: String,
    pub(crate) team_id: String,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) role: Option<TeamRole>,
    pub(crate) policy_epoch: u64,
    pub(crate) membership_epoch: u64,
    pub(crate) global_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeamMembershipSnapshot {
    pub(crate) organization_id: String,
    pub(crate) team_id: String,
    pub(crate) principal_id: String,
    pub(crate) role: TeamRole,
    pub(crate) status: String,
    pub(crate) membership_epoch: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct CreateTeamInvitationInput {
    actor: VerifiedIdentity,
    invited_principal_id: String,
    team_id: String,
    role: TeamRole,
    opaque_token: [u8; 32],
    ttl_seconds: i64,
}

impl CreateTeamInvitationInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        team_id: impl Into<String>,
        invited_principal_id: impl Into<String>,
        role: TeamRole,
        opaque_token: [u8; 32],
        ttl_seconds: i64,
    ) -> AccessStoreResult<Self> {
        let team_id = team_id.into();
        let invited_principal_id = invited_principal_id.into();
        if !valid_id(&team_id)
            || !valid_id(&invited_principal_id)
            || !(1..=MAX_INVITATION_TTL_SECONDS).contains(&ttl_seconds)
            || opaque_token.iter().all(|byte| *byte == 0)
        {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            invited_principal_id,
            team_id,
            role,
            opaque_token,
            ttl_seconds,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AcceptTeamInvitationInput {
    identity: VerifiedIdentity,
    opaque_token: [u8; 32],
}

impl AcceptTeamInvitationInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        opaque_token: [u8; 32],
    ) -> AccessStoreResult<Self> {
        if opaque_token.iter().all(|byte| *byte == 0) {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            identity,
            opaque_token,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeamInvitationSnapshot {
    pub(crate) organization_id: String,
    pub(crate) team_id: String,
    pub(crate) role: TeamRole,
    pub(crate) status: String,
    pub(crate) team_membership_epoch: u64,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct AssignTeamProjectInput {
    actor: VerifiedIdentity,
    team_id: String,
    project_id: String,
    role: ProjectRole,
}

impl AssignTeamProjectInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        team_id: impl Into<String>,
        project_id: impl Into<String>,
        role: ProjectRole,
    ) -> AccessStoreResult<Self> {
        let team_id = team_id.into();
        let project_id = project_id.into();
        if !valid_id(&team_id) || !valid_id(&project_id) {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            team_id,
            project_id,
            role,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeamProjectAssignmentSnapshot {
    pub(crate) organization_id: String,
    pub(crate) team_id: String,
    pub(crate) project_id: String,
    pub(crate) role: ProjectRole,
    pub(crate) assignment_epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct ManagedProjectSnapshot {
    pub(crate) project_id: String,
    pub(crate) team_id: String,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) role: String,
    pub(crate) policy_epoch: u64,
    pub(crate) can_manage: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ManageTeamProjectInput {
    actor: VerifiedIdentity,
    team_id: String,
    project_id: String,
    name: Option<String>,
}
impl ManageTeamProjectInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        team_id: impl Into<String>,
        project_id: impl Into<String>,
        name: Option<String>,
    ) -> AccessStoreResult<Self> {
        let team_id = team_id.into();
        let project_id = project_id.into();
        if !valid_id(&team_id)
            || !valid_id(&project_id)
            || name.as_deref().is_some_and(|v| !valid_name(v))
        {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            team_id,
            project_id,
            name,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EffectiveProjectRoleSnapshot {
    pub(crate) organization_id: String,
    pub(crate) project_id: String,
    pub(crate) role: ProjectRole,
    pub(crate) direct: bool,
    pub(crate) team_derived: bool,
    pub(crate) global_revision: u64,
}

pub(super) fn create_team(
    connection: &mut Connection,
    input: &CreateTeamInput,
) -> AccessStoreResult<TeamSnapshot> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_platform_admin(&tx, &actor.id)?;
    let now = unix_now()?;
    tx.execute(
        "INSERT INTO groups(group_id,organization_id,kind,name,status,policy_epoch,membership_epoch,created_by,created_at,updated_at,deleted_at) VALUES(?1,?2,'team',?3,'active',1,1,?4,?5,?5,NULL)",
        params![input.team_id, actor.organization_id, input.name, actor.id, now],
    ).map_err(map_sqlite_error)?;
    tx.execute(
        "INSERT INTO team_memberships(membership_id,organization_id,team_id,principal_id,role,status,membership_epoch,created_by,created_at,updated_at,revoked_at) VALUES(?1,?2,?3,?4,'owner','active',1,?4,?5,?5,NULL)",
        params![format!("team-owner-{}-{}", input.team_id, actor.id), actor.organization_id, input.team_id, actor.id, now],
    ).map_err(map_sqlite_error)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        "access.team.create",
        "team",
        &input.team_id,
        1,
        "platform_admin",
    )?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        "access.team_member.add",
        "team_membership",
        &format!("{}\0{}", input.team_id, actor.id),
        1,
        "implicit_team_owner",
    )?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(TeamSnapshot {
        organization_id: actor.organization_id,
        team_id: input.team_id.clone(),
        name: input.name.clone(),
        status: "active".into(),
        role: Some(TeamRole::Owner),
        policy_epoch: 1,
        membership_epoch: 1,
        global_revision: revision,
    })
}

pub(super) fn list_teams(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
) -> AccessStoreResult<Vec<TeamSnapshot>> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let actor = resolve_principal(&tx, identity)?;
    let platform = is_platform_admin(&tx, &actor.id)?;
    let revision = global_revision(&tx)?;
    let sql = if platform {
        "SELECT g.organization_id,g.group_id,g.name,g.status,NULL,g.policy_epoch,g.membership_epoch FROM groups g WHERE g.kind='team' AND g.status!='deleted' ORDER BY g.organization_id COLLATE BINARY,g.group_id COLLATE BINARY"
    } else {
        "SELECT g.organization_id,g.group_id,g.name,g.status,m.role,g.policy_epoch,g.membership_epoch FROM team_memberships m JOIN groups g ON g.organization_id=m.organization_id AND g.group_id=m.team_id WHERE m.organization_id=?1 AND m.principal_id=?2 AND m.status='active' AND g.kind='team' AND g.status!='deleted' ORDER BY g.group_id COLLATE BINARY"
    };
    let mut statement = tx.prepare(sql).map_err(map_sqlite_error)?;
    let mut snapshots = Vec::new();
    if platform {
        let rows = statement
            .query_map([], team_row)
            .map_err(map_sqlite_error)?;
        for row in rows {
            snapshots.push(snapshot(row.map_err(map_sqlite_error)?, revision)?);
        }
    } else {
        let rows = statement
            .query_map(params![actor.organization_id, actor.id], team_row)
            .map_err(map_sqlite_error)?;
        for row in rows {
            snapshots.push(snapshot(row.map_err(map_sqlite_error)?, revision)?);
        }
    }
    drop(statement);
    tx.commit().map_err(map_sqlite_error)?;
    Ok(snapshots)
}

fn team_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, String, Option<String>, i64, i64)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn snapshot(
    row: (String, String, String, String, Option<String>, i64, i64),
    revision: u64,
) -> AccessStoreResult<TeamSnapshot> {
    let role = match row.4.as_deref() {
        Some(value) => {
            Some(TeamRole::from_persisted(value).ok_or(AccessStoreError::MalformedVocabulary)?)
        }
        None => None,
    };
    Ok(TeamSnapshot {
        organization_id: row.0,
        team_id: row.1,
        name: row.2,
        status: row.3,
        role,
        policy_epoch: epoch(row.5)?,
        membership_epoch: epoch(row.6)?,
        global_revision: revision,
    })
}

pub(super) fn add_member(
    connection: &mut Connection,
    input: &AddTeamMemberInput,
) -> AccessStoreResult<TeamMembershipSnapshot> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_team_manager(&tx, &actor.id, &actor.organization_id, &input.team_id)?;
    require_principal_in_organization(&tx, &input.principal_id, &actor.organization_id)?;
    let now = unix_now()?;
    let existing: Option<String> = tx.query_row("SELECT status FROM team_memberships WHERE organization_id=?1 AND team_id=?2 AND principal_id=?3", params![actor.organization_id,input.team_id,input.principal_id], |row| row.get(0)).optional().map_err(map_sqlite_error)?;
    if existing.is_some() {
        return Err(AccessStoreError::TeamUnavailable);
    }
    tx.execute("INSERT INTO team_memberships(membership_id,organization_id,team_id,principal_id,role,status,membership_epoch,created_by,created_at,updated_at,revoked_at) VALUES(?1,?2,?3,?4,?5,'active',1,?6,?7,?7,NULL)", params![format!("team-member-{}-{}",input.team_id,input.principal_id),actor.organization_id,input.team_id,input.principal_id,input.role.as_persisted(),actor.id,now]).map_err(map_sqlite_error)?;
    let membership_epoch =
        advance_team_membership_epoch(&tx, &actor.organization_id, &input.team_id, now)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        "access.team_member.add",
        "team_membership",
        &format!("{}\0{}", input.team_id, input.principal_id),
        membership_epoch,
        "team_manage",
    )?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(TeamMembershipSnapshot {
        organization_id: actor.organization_id,
        team_id: input.team_id.clone(),
        principal_id: input.principal_id.clone(),
        role: input.role,
        status: "active".into(),
        membership_epoch: 1,
    })
}

pub(super) fn suspend_team(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    team_id: &str,
) -> AccessStoreResult<()> {
    mutate_team_status(connection, identity, team_id, "suspended")
}

pub(super) fn activate_team(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    team_id: &str,
) -> AccessStoreResult<()> {
    mutate_team_status(connection, identity, team_id, "active")
}

fn mutate_team_status(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    team_id: &str,
    status: &str,
) -> AccessStoreResult<()> {
    if !valid_id(team_id) {
        return Err(AccessStoreError::InvalidTeamInput);
    }
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, identity)?;
    require_team_manager(&tx, &actor.id, &actor.organization_id, team_id)?;
    let now = unix_now()?;
    let changed = tx.execute("UPDATE groups SET status=?1,policy_epoch=policy_epoch+1,updated_at=?2 WHERE organization_id=?3 AND group_id=?4 AND kind='team' AND status!=?1 AND status!='deleted'", params![status,now,actor.organization_id,team_id]).map_err(map_sqlite_error)?;
    if changed != 1 {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let policy_epoch: i64 = tx
        .query_row(
            "SELECT policy_epoch FROM groups WHERE organization_id=?1 AND group_id=?2",
            params![actor.organization_id, team_id],
            |r| r.get(0),
        )
        .map_err(map_sqlite_error)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        "access.team.status",
        "team",
        team_id,
        epoch(policy_epoch)?,
        status,
    )?;
    tx.commit().map_err(map_sqlite_error)
}

pub(super) fn set_member_role(
    connection: &mut Connection,
    input: &AddTeamMemberInput,
) -> AccessStoreResult<()> {
    mutate_member(
        connection,
        &input.actor,
        &input.team_id,
        &input.principal_id,
        Some(input.role),
        None,
        "access.team_member.role",
    )
}

pub(super) fn suspend_member(
    connection: &mut Connection,
    input: &TeamMembershipInput,
) -> AccessStoreResult<()> {
    mutate_member(
        connection,
        &input.actor,
        &input.team_id,
        &input.principal_id,
        None,
        Some("suspended"),
        "access.team_member.suspend",
    )
}

pub(super) fn remove_member(
    connection: &mut Connection,
    input: &TeamMembershipInput,
) -> AccessStoreResult<()> {
    mutate_member(
        connection,
        &input.actor,
        &input.team_id,
        &input.principal_id,
        None,
        Some("revoked"),
        "access.team_member.remove",
    )
}

fn mutate_member(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    team_id: &str,
    principal_id: &str,
    role: Option<TeamRole>,
    status: Option<&str>,
    action: &str,
) -> AccessStoreResult<()> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, identity)?;
    require_team_manager(&tx, &actor.id, &actor.organization_id, team_id)?;
    protect_last_owner(
        &tx,
        &actor.organization_id,
        team_id,
        principal_id,
        role,
        status,
    )?;
    let now = unix_now()?;
    let changed=match (role,status) {
        (Some(role),None)=>tx.execute("UPDATE team_memberships SET role=?1,membership_epoch=membership_epoch+1,updated_at=?2 WHERE organization_id=?3 AND team_id=?4 AND principal_id=?5 AND status!='revoked' AND role!=?1",params![role.as_persisted(),now,actor.organization_id,team_id,principal_id]),
        (None,Some("revoked"))=>tx.execute("UPDATE team_memberships SET status='revoked',membership_epoch=membership_epoch+1,updated_at=?1,revoked_at=?1 WHERE organization_id=?2 AND team_id=?3 AND principal_id=?4 AND status!='revoked'",params![now,actor.organization_id,team_id,principal_id]),
        (None,Some(value))=>tx.execute("UPDATE team_memberships SET status=?1,membership_epoch=membership_epoch+1,updated_at=?2,revoked_at=NULL WHERE organization_id=?3 AND team_id=?4 AND principal_id=?5 AND status!=?1 AND status!='revoked'",params![value,now,actor.organization_id,team_id,principal_id]),
        _=>return Err(AccessStoreError::InvalidTeamInput),
    }.map_err(map_sqlite_error)?;
    if changed != 1 {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let team_epoch = advance_team_membership_epoch(&tx, &actor.organization_id, team_id, now)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        action,
        "team_membership",
        principal_id,
        team_epoch,
        "team_manage",
    )?;
    tx.commit().map_err(map_sqlite_error)
}

pub(super) fn grant_platform_admin(
    connection: &mut Connection,
    input: &PlatformAdministratorInput,
) -> AccessStoreResult<()> {
    mutate_platform_admin(connection, input, "active", "access.platform_admin.grant")
}
pub(super) fn revoke_platform_admin(
    connection: &mut Connection,
    input: &PlatformAdministratorInput,
) -> AccessStoreResult<()> {
    mutate_platform_admin(connection, input, "revoked", "access.platform_admin.revoke")
}

pub(super) fn create_invitation(
    connection: &mut Connection,
    input: &CreateTeamInvitationInput,
) -> AccessStoreResult<TeamInvitationSnapshot> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_team_manager(&tx, &actor.id, &actor.organization_id, &input.team_id)?;
    if input.role == TeamRole::Owner {
        require_team_owner(&tx, &actor.id, &actor.organization_id, &input.team_id)?;
    }
    require_principal_in_organization(&tx, &input.invited_principal_id, &actor.organization_id)?;
    let now = unix_now()?;
    let expires_at = now
        .checked_add(input.ttl_seconds)
        .ok_or(AccessStoreError::InvalidTeamInput)?;
    let team_epoch: i64 = tx
        .query_row(
            "SELECT membership_epoch FROM groups
             WHERE organization_id=?1 AND group_id=?2 AND kind='team' AND status='active'",
            params![actor.organization_id, input.team_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or(AccessStoreError::TeamUnavailable)?;
    let digest = Sha256::digest(input.opaque_token);
    tx.execute(
        "INSERT INTO team_invitations(
           invitation_digest,organization_id,team_id,role,invited_principal_id,
           inviter_principal_id,team_membership_epoch,status,accepted_principal_id,
           created_at,expires_at,accepted_at,revoked_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,'pending',NULL,?8,?9,NULL,NULL,?8)",
        params![
            digest.as_slice(),
            actor.organization_id,
            input.team_id,
            input.role.as_persisted(),
            input.invited_principal_id,
            actor.id,
            team_epoch,
            now,
            expires_at
        ],
    )
    .map_err(map_sqlite_error)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        "access.team_invitation.create",
        "team_invitation",
        &input.team_id,
        epoch(team_epoch)?,
        "team_manage",
    )?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(TeamInvitationSnapshot {
        organization_id: actor.organization_id,
        team_id: input.team_id.clone(),
        role: input.role,
        status: "pending".into(),
        team_membership_epoch: epoch(team_epoch)?,
        expires_at,
    })
}

pub(super) fn accept_invitation(
    connection: &mut Connection,
    input: &AcceptTeamInvitationInput,
) -> AccessStoreResult<TeamMembershipSnapshot> {
    accept_invitation_at(connection, input, unix_now()?)
}

fn accept_invitation_at(
    connection: &mut Connection,
    input: &AcceptTeamInvitationInput,
    now: i64,
) -> AccessStoreResult<TeamMembershipSnapshot> {
    let tx = immediate(connection)?;
    let principal = resolve_principal(&tx, &input.identity)?;
    let digest = Sha256::digest(input.opaque_token);
    let invitation: Option<(String, String, String, String, i64, i64)> = tx
        .query_row(
            "SELECT organization_id,team_id,role,invited_principal_id,
                    team_membership_epoch,expires_at
             FROM team_invitations WHERE invitation_digest=?1 AND status='pending'",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let Some((organization_id, team_id, role, invited_principal_id, issued_epoch, expires_at)) =
        invitation
    else {
        return Err(AccessStoreError::TeamUnavailable);
    };
    if expires_at <= now {
        tx.execute(
            "UPDATE team_invitations SET status='expired',updated_at=?1
             WHERE invitation_digest=?2 AND status='pending'",
            params![now, digest.as_slice()],
        )
        .map_err(map_sqlite_error)?;
        tx.commit().map_err(map_sqlite_error)?;
        return Err(AccessStoreError::TeamUnavailable);
    }
    if organization_id != principal.organization_id || invited_principal_id != principal.id {
        return Err(AccessStoreError::NotAuthorized);
    }
    let current_epoch: i64 = tx
        .query_row(
            "SELECT membership_epoch FROM groups
             WHERE organization_id=?1 AND group_id=?2 AND kind='team' AND status='active'",
            params![organization_id, team_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or(AccessStoreError::TeamUnavailable)?;
    if current_epoch != issued_epoch {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let role = TeamRole::from_persisted(&role).ok_or(AccessStoreError::MalformedVocabulary)?;
    tx.execute(
        "INSERT INTO team_memberships(
           membership_id,organization_id,team_id,principal_id,role,status,
           membership_epoch,created_by,created_at,updated_at,revoked_at)
         SELECT ?1,organization_id,team_id,?2,role,'active',1,
                inviter_principal_id,?3,?3,NULL
         FROM team_invitations WHERE invitation_digest=?4 AND status='pending'",
        params![
            format!("team-member-{team_id}-{}", principal.id),
            principal.id,
            now,
            digest.as_slice()
        ],
    )
    .map_err(map_sqlite_error)?;
    let changed = tx
        .execute(
            "UPDATE team_invitations
             SET status='accepted',accepted_principal_id=?1,accepted_at=?2,updated_at=?2
             WHERE invitation_digest=?3 AND status='pending'",
            params![principal.id, now, digest.as_slice()],
        )
        .map_err(map_sqlite_error)?;
    if changed != 1 {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let team_epoch = advance_team_membership_epoch(&tx, &organization_id, &team_id, now)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &principal.id,
        &organization_id,
        "access.team_invitation.accept",
        "team_membership",
        &principal.id,
        team_epoch,
        "verified_identity_invitation",
    )?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(TeamMembershipSnapshot {
        organization_id,
        team_id,
        principal_id: principal.id,
        role,
        status: "active".into(),
        membership_epoch: 1,
    })
}

pub(super) fn assign_team_project(
    connection: &mut Connection,
    input: &AssignTeamProjectInput,
) -> AccessStoreResult<TeamProjectAssignmentSnapshot> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_team_manager(&tx, &actor.id, &actor.organization_id, &input.team_id)?;
    let project_exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects
             WHERE organization_id=?1 AND project_id=?2 AND status='active')",
            params![actor.organization_id, input.project_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if !project_exists {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let now = unix_now()?;
    tx.execute(
        "INSERT INTO team_project_assignments(
           assignment_id,organization_id,team_id,project_id,role,status,
           assignment_epoch,created_by,created_at,updated_at,revoked_at)
         VALUES(?1,?2,?3,?4,?5,'active',1,?6,?7,?7,NULL)
         ON CONFLICT(organization_id,team_id,project_id) DO UPDATE SET
           role=excluded.role,status='active',
           assignment_epoch=team_project_assignments.assignment_epoch+1,
           updated_at=excluded.updated_at,revoked_at=NULL",
        params![
            format!("team-project-{}-{}", input.team_id, input.project_id),
            actor.organization_id,
            input.team_id,
            input.project_id,
            input.role.as_persisted(),
            actor.id,
            now
        ],
    )
    .map_err(map_sqlite_error)?;
    tx.execute(
        "UPDATE projects SET project_policy_epoch=project_policy_epoch+1,updated_at=?1
         WHERE organization_id=?2 AND project_id=?3",
        params![now, actor.organization_id, input.project_id],
    )
    .map_err(map_sqlite_error)?;
    let assignment_epoch: i64 = tx
        .query_row(
            "SELECT assignment_epoch FROM team_project_assignments
             WHERE organization_id=?1 AND team_id=?2 AND project_id=?3",
            params![actor.organization_id, input.team_id, input.project_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        "access.team_project.assign",
        "team_project",
        &format!("{}\0{}", input.team_id, input.project_id),
        epoch(assignment_epoch)?,
        "team_manage",
    )?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(TeamProjectAssignmentSnapshot {
        organization_id: actor.organization_id,
        team_id: input.team_id.clone(),
        project_id: input.project_id.clone(),
        role: input.role,
        assignment_epoch: epoch(assignment_epoch)?,
    })
}

pub(super) fn create_managed_project(
    connection: &mut Connection,
    input: &ManageTeamProjectInput,
) -> AccessStoreResult<ManagedProjectSnapshot> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_team_manager(&tx, &actor.id, &actor.organization_id, &input.team_id)?;
    let name = input
        .name
        .as_deref()
        .ok_or(AccessStoreError::InvalidTeamInput)?;
    let now = unix_now()?;
    tx.execute("INSERT INTO projects(project_id,organization_id,name,status,project_policy_epoch,created_at,updated_at) VALUES(?1,?2,?3,'active',1,?4,?4)",params![input.project_id,actor.organization_id,name,now]).map_err(map_sqlite_error)?;
    tx.execute("INSERT INTO team_project_assignments(assignment_id,organization_id,team_id,project_id,role,status,assignment_epoch,created_by,created_at,updated_at,revoked_at) VALUES(?1,?2,?3,?4,'admin','active',1,?5,?6,?6,NULL)",params![format!("team-project-{}-{}",input.team_id,input.project_id),actor.organization_id,input.team_id,input.project_id,actor.id,now]).map_err(map_sqlite_error)?;
    advance_global_revision(&tx, now)?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(ManagedProjectSnapshot {
        project_id: input.project_id.clone(),
        team_id: input.team_id.clone(),
        name: name.into(),
        status: "active".into(),
        role: "admin".into(),
        policy_epoch: 1,
        can_manage: true,
    })
}

pub(super) fn get_managed_project(
    connection: &mut Connection,
    input: &ManageTeamProjectInput,
) -> AccessStoreResult<ManagedProjectSnapshot> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_team_member(&tx, &actor.id, &actor.organization_id, &input.team_id)?;
    let can_manage =
        require_team_manager(&tx, &actor.id, &actor.organization_id, &input.team_id).is_ok();
    let result=tx.query_row("SELECT p.name,p.status,a.role,p.project_policy_epoch FROM projects p JOIN team_project_assignments a ON a.organization_id=p.organization_id AND a.project_id=p.project_id WHERE p.organization_id=?1 AND p.project_id=?2 AND a.team_id=?3 AND a.status='active'",params![actor.organization_id,input.project_id,input.team_id],|r|Ok(ManagedProjectSnapshot{project_id:input.project_id.clone(),team_id:input.team_id.clone(),name:r.get(0)?,status:r.get(1)?,role:r.get(2)?,policy_epoch:u64::try_from(r.get::<_,i64>(3)?).map_err(|_|rusqlite::Error::InvalidQuery)?,can_manage})).optional().map_err(map_sqlite_error)?.ok_or(AccessStoreError::TeamUnavailable)?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(result)
}

pub(super) fn list_managed_projects(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
) -> AccessStoreResult<Vec<ManagedProjectSnapshot>> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let actor = resolve_principal(&tx, identity)?;
    let platform = is_platform_admin(&tx, &actor.id)?;
    let values = if platform {
        let mut statement=tx.prepare("SELECT p.project_id,a.team_id,p.name,p.status,a.role,p.project_policy_epoch FROM projects p JOIN team_project_assignments a ON a.organization_id=p.organization_id AND a.project_id=p.project_id WHERE p.organization_id=?1 AND a.status='active' AND p.status!='disabled' ORDER BY a.team_id,p.project_id").map_err(map_sqlite_error)?;
        statement
            .query_map([actor.organization_id], |r| {
                Ok(ManagedProjectSnapshot {
                    project_id: r.get(0)?,
                    team_id: r.get(1)?,
                    name: r.get(2)?,
                    status: r.get(3)?,
                    role: r.get(4)?,
                    policy_epoch: u64::try_from(r.get::<_, i64>(5)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    can_manage: true,
                })
            })
            .map_err(map_sqlite_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_sqlite_error)?
    } else {
        let mut statement=tx.prepare("SELECT p.project_id,a.team_id,p.name,p.status,a.role,p.project_policy_epoch,m.role FROM projects p JOIN team_project_assignments a ON a.organization_id=p.organization_id AND a.project_id=p.project_id JOIN team_memberships m ON m.organization_id=a.organization_id AND m.team_id=a.team_id WHERE p.organization_id=?1 AND m.principal_id=?2 AND m.status='active' AND a.status='active' AND p.status!='disabled' ORDER BY a.team_id,p.project_id").map_err(map_sqlite_error)?;
        statement
            .query_map(params![actor.organization_id, actor.id], |r| {
                Ok(ManagedProjectSnapshot {
                    project_id: r.get(0)?,
                    team_id: r.get(1)?,
                    name: r.get(2)?,
                    status: r.get(3)?,
                    role: r.get(4)?,
                    policy_epoch: u64::try_from(r.get::<_, i64>(5)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    can_manage: matches!(r.get::<_, String>(6)?.as_str(), "owner" | "admin"),
                })
            })
            .map_err(map_sqlite_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_sqlite_error)?
    };
    tx.commit().map_err(map_sqlite_error)?;
    Ok(values)
}

pub(super) fn update_managed_project(
    connection: &mut Connection,
    input: &ManageTeamProjectInput,
    archive: bool,
) -> AccessStoreResult<ManagedProjectSnapshot> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_team_manager(&tx, &actor.id, &actor.organization_id, &input.team_id)?;
    let assigned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM team_project_assignments WHERE organization_id=?1 AND team_id=?2 AND project_id=?3 AND status='active')",params![actor.organization_id,input.team_id,input.project_id],|r|r.get(0)).map_err(map_sqlite_error)?;
    if !assigned {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let now = unix_now()?;
    let changed = if archive {
        tx.execute("UPDATE projects SET status='disabled',project_policy_epoch=project_policy_epoch+1,updated_at=?1 WHERE organization_id=?2 AND project_id=?3 AND status='active'",params![now,actor.organization_id,input.project_id]).map_err(map_sqlite_error)?
    } else {
        tx.execute("UPDATE projects SET name=?1,project_policy_epoch=project_policy_epoch+1,updated_at=?2 WHERE organization_id=?3 AND project_id=?4 AND status='active'",params![input.name.as_deref().ok_or(AccessStoreError::InvalidTeamInput)?,now,actor.organization_id,input.project_id]).map_err(map_sqlite_error)?
    };
    if changed != 1 {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let snapshot=tx.query_row("SELECT p.name,p.status,a.role,p.project_policy_epoch FROM projects p JOIN team_project_assignments a ON a.organization_id=p.organization_id AND a.project_id=p.project_id WHERE p.organization_id=?1 AND p.project_id=?2 AND a.team_id=?3",params![actor.organization_id,input.project_id,input.team_id],|r|Ok(ManagedProjectSnapshot{project_id:input.project_id.clone(),team_id:input.team_id.clone(),name:r.get(0)?,status:r.get(1)?,role:r.get(2)?,policy_epoch:u64::try_from(r.get::<_,i64>(3)?).map_err(|_|rusqlite::Error::InvalidQuery)?,can_manage:true})).map_err(map_sqlite_error)?;
    advance_global_revision(&tx, now)?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(snapshot)
}

pub(super) fn list_effective_projects(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
) -> AccessStoreResult<Vec<EffectiveProjectRoleSnapshot>> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let actor = resolve_principal(&tx, identity)?;
    let revision = global_revision(&tx)?;
    let mut statement = tx
        .prepare(
            "WITH candidates(project_id,role,source) AS (
               SELECT pm.project_id,pm.role,'direct'
               FROM project_memberships pm JOIN projects p
                 ON p.organization_id=pm.organization_id AND p.project_id=pm.project_id
               WHERE pm.organization_id=?1 AND pm.principal_id=?2
                 AND pm.status='active' AND p.status='active'
               UNION ALL
               SELECT a.project_id,a.role,'team'
               FROM team_memberships tm
               JOIN groups g ON g.organization_id=tm.organization_id AND g.group_id=tm.team_id
               JOIN team_project_assignments a
                 ON a.organization_id=tm.organization_id AND a.team_id=tm.team_id
               JOIN projects p
                 ON p.organization_id=a.organization_id AND p.project_id=a.project_id
               WHERE tm.organization_id=?1 AND tm.principal_id=?2 AND tm.status='active'
                 AND g.status='active' AND a.status='active' AND p.status='active'
             ), ranked AS (
               SELECT project_id,source,
                 CASE role WHEN 'owner' THEN 4 WHEN 'admin' THEN 3
                           WHEN 'member' THEN 2 WHEN 'viewer' THEN 1 ELSE 0 END rank
               FROM candidates
             )
             SELECT project_id,MAX(rank),
                    MAX(CASE WHEN source='direct' THEN 1 ELSE 0 END),
                    MAX(CASE WHEN source='team' THEN 1 ELSE 0 END)
             FROM ranked GROUP BY project_id ORDER BY project_id COLLATE BINARY",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(params![actor.organization_id, actor.id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })
        .map_err(map_sqlite_error)?;
    let mut result = Vec::new();
    for row in rows {
        let (project_id, rank, direct, team_derived) = row.map_err(map_sqlite_error)?;
        let role = match rank {
            4 => ProjectRole::Owner,
            3 => ProjectRole::Admin,
            2 => ProjectRole::Member,
            1 => ProjectRole::Viewer,
            _ => return Err(AccessStoreError::MalformedVocabulary),
        };
        result.push(EffectiveProjectRoleSnapshot {
            organization_id: actor.organization_id.clone(),
            project_id,
            role,
            direct,
            team_derived,
            global_revision: revision,
        });
    }
    drop(statement);
    tx.commit().map_err(map_sqlite_error)?;
    Ok(result)
}

fn mutate_platform_admin(
    connection: &mut Connection,
    input: &PlatformAdministratorInput,
    status: &str,
    action: &str,
) -> AccessStoreResult<()> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_platform_admin(&tx, &actor.id)?;
    require_principal_in_organization(&tx, &input.principal_id, &actor.organization_id)?;
    if status == "revoked" && input.principal_id == actor.id {
        return Err(AccessStoreError::NotAuthorized);
    }
    let now = unix_now()?;
    tx.execute("INSERT INTO platform_administrators(principal_id,status,authority_epoch,granted_by,created_at,updated_at,revoked_at) VALUES(?1,?2,1,?3,?4,?4,CASE WHEN ?2='revoked' THEN ?4 ELSE NULL END) ON CONFLICT(principal_id) DO UPDATE SET status=excluded.status,authority_epoch=platform_administrators.authority_epoch+1,updated_at=excluded.updated_at,revoked_at=excluded.revoked_at",params![input.principal_id,status,actor.id,now]).map_err(map_sqlite_error)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        action,
        "principal",
        &input.principal_id,
        revision,
        status,
    )?;
    tx.commit().map_err(map_sqlite_error)
}

fn protect_last_owner(
    tx: &Transaction<'_>,
    organization_id: &str,
    team_id: &str,
    principal_id: &str,
    new_role: Option<TeamRole>,
    new_status: Option<&str>,
) -> AccessStoreResult<()> {
    let current:Option<(String,String)>=tx.query_row("SELECT role,status FROM team_memberships WHERE organization_id=?1 AND team_id=?2 AND principal_id=?3",params![organization_id,team_id,principal_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(map_sqlite_error)?;
    if current
        .as_ref()
        .is_some_and(|(role, status)| role == "owner" && status == "active")
        && (new_role.is_some_and(|role| role != TeamRole::Owner)
            || new_status.is_some_and(|status| status != "active"))
    {
        let owners:i64=tx.query_row("SELECT count(*) FROM team_memberships WHERE organization_id=?1 AND team_id=?2 AND role='owner' AND status='active'",params![organization_id,team_id],|r|r.get(0)).map_err(map_sqlite_error)?;
        if owners <= 1 {
            return Err(AccessStoreError::LastActiveTeamOwner);
        }
    }
    Ok(())
}

fn require_platform_admin(tx: &Transaction<'_>, principal_id: &str) -> AccessStoreResult<()> {
    if is_platform_admin(tx, principal_id)? {
        Ok(())
    } else {
        Err(AccessStoreError::NotAuthorized)
    }
}
fn is_platform_admin(tx: &Transaction<'_>, principal_id: &str) -> AccessStoreResult<bool> {
    tx.query_row("SELECT EXISTS(SELECT 1 FROM platform_administrators WHERE principal_id=?1 AND status='active')",[principal_id],|r|r.get(0)).map_err(map_sqlite_error)
}
fn require_team_manager(
    tx: &Transaction<'_>,
    principal_id: &str,
    organization_id: &str,
    team_id: &str,
) -> AccessStoreResult<()> {
    if is_platform_admin(tx, principal_id)? {
        return Ok(());
    }
    let allowed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM team_memberships m JOIN groups g ON g.organization_id=m.organization_id AND g.group_id=m.team_id WHERE m.organization_id=?1 AND m.team_id=?2 AND m.principal_id=?3 AND m.status='active' AND m.role IN ('owner','admin') AND g.status='active')",params![organization_id,team_id,principal_id],|r|r.get(0)).map_err(map_sqlite_error)?;
    if allowed {
        Ok(())
    } else {
        Err(AccessStoreError::NotAuthorized)
    }
}
fn require_team_member(
    tx: &Transaction<'_>,
    principal_id: &str,
    organization_id: &str,
    team_id: &str,
) -> AccessStoreResult<()> {
    if is_platform_admin(tx, principal_id)? {
        return Ok(());
    }
    let allowed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM team_memberships m JOIN groups g ON g.organization_id=m.organization_id AND g.group_id=m.team_id WHERE m.organization_id=?1 AND m.team_id=?2 AND m.principal_id=?3 AND m.status='active' AND g.status='active')",params![organization_id,team_id,principal_id],|r|r.get(0)).map_err(map_sqlite_error)?;
    if allowed {
        Ok(())
    } else {
        Err(AccessStoreError::NotAuthorized)
    }
}
fn require_team_owner(
    tx: &Transaction<'_>,
    principal_id: &str,
    organization_id: &str,
    team_id: &str,
) -> AccessStoreResult<()> {
    if is_platform_admin(tx, principal_id)? {
        return Ok(());
    }
    let allowed: bool = tx
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM team_memberships m
               JOIN groups g ON g.organization_id=m.organization_id AND g.group_id=m.team_id
               WHERE m.organization_id=?1 AND m.team_id=?2 AND m.principal_id=?3
                 AND m.status='active' AND m.role='owner' AND g.status='active')",
            params![organization_id, team_id, principal_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if allowed {
        Ok(())
    } else {
        Err(AccessStoreError::NotAuthorized)
    }
}
fn require_principal_in_organization(
    tx: &Transaction<'_>,
    principal_id: &str,
    organization_id: &str,
) -> AccessStoreResult<()> {
    let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM principals WHERE principal_id=?1 AND organization_id=?2 AND status='active')",params![principal_id,organization_id],|r|r.get(0)).map_err(map_sqlite_error)?;
    if exists {
        Ok(())
    } else {
        Err(AccessStoreError::TeamUnavailable)
    }
}
fn advance_team_membership_epoch(
    tx: &Transaction<'_>,
    organization_id: &str,
    team_id: &str,
    now: i64,
) -> AccessStoreResult<u64> {
    let changed=tx.execute("UPDATE groups SET membership_epoch=membership_epoch+1,updated_at=?1 WHERE organization_id=?2 AND group_id=?3 AND status!='deleted'",params![now,organization_id,team_id]).map_err(map_sqlite_error)?;
    if changed != 1 {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let value: i64 = tx
        .query_row(
            "SELECT membership_epoch FROM groups WHERE organization_id=?1 AND group_id=?2",
            params![organization_id, team_id],
            |r| r.get(0),
        )
        .map_err(map_sqlite_error)?;
    epoch(value)
}
fn advance_global_revision(tx: &Transaction<'_>, now: i64) -> AccessStoreResult<u64> {
    tx.execute("UPDATE access_metadata SET global_revision=global_revision+1,updated_at=?1 WHERE singleton=1",[now]).map_err(map_sqlite_error)?;
    global_revision(tx)
}
fn global_revision(tx: &Transaction<'_>) -> AccessStoreResult<u64> {
    let value: i64 = tx
        .query_row(
            "SELECT global_revision FROM access_metadata WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(map_sqlite_error)?;
    epoch(value)
}
fn epoch(value: i64) -> AccessStoreResult<u64> {
    u64::try_from(value).map_err(|_| AccessStoreError::MalformedVocabulary)
}
fn audit(
    tx: &Transaction<'_>,
    revision: u64,
    now: i64,
    actor: &str,
    organization: &str,
    action: &str,
    target_kind: &str,
    target: &str,
    policy_epoch: u64,
    reason: &str,
) -> AccessStoreResult<()> {
    let identity = hex::encode(Sha256::digest(format!("{target_kind}\0{target}")));
    tx.execute("INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json) VALUES(?1,?2,NULL,?3,?4,NULL,?5,?6,?7,'allow',?8,?9,'{}')",params![format!("team-authority-{revision}-{}",&identity[..16]),now,actor,organization,action,target_kind,target,reason,i64::try_from(policy_epoch).map_err(|_|AccessStoreError::MalformedVocabulary)?]).map_err(map_sqlite_error)?;
    Ok(())
}
fn immediate(connection: &mut Connection) -> AccessStoreResult<Transaction<'_>> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)
}
fn unix_now() -> AccessStoreResult<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .map_err(|e| AccessStoreError::Unavailable(e.to_string()))
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NAME
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use super::*;
    use crate::access::{BootstrapOwnerInput, store::AccessStore};

    fn identity(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    async fn store() -> (tempfile::TempDir, AccessStore, VerifiedIdentity) {
        let directory = super::super::test_support::secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = identity("owner");
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        (directory, store, owner)
    }

    async fn seed_principal(store: &AccessStore, principal_id: &str, subject: &str) {
        let principal_id = principal_id.to_owned();
        let subject = subject.to_owned();
        store
            .with_connection(move |connection| {
                connection
                    .execute(
                        "INSERT INTO principals(principal_id,organization_id,kind,status,display_name,created_at,updated_at) VALUES(?1,'bootstrap-local','user','active',NULL,2,2)",
                        [&principal_id],
                    )
                    .map_err(map_sqlite_error)?;
                connection
                    .execute(
                        "INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'external','https://accounts.google.com',?3,NULL,'active',1,1,2,2)",
                        params![format!("link-{principal_id}"), principal_id, subject],
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn verified_bootstrap_binds_explicit_platform_admin_and_initial_team() {
        let (_directory, store, owner) = store().await;
        let teams = store.list_teams(owner).await.unwrap();
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].team_id, "bootstrap-initial-team");
        assert_eq!(teams[0].status, "active");
        assert_eq!(teams[0].policy_epoch, 1);
        assert_eq!(teams[0].membership_epoch, 1);

        let counts = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT (SELECT count(*) FROM platform_administrators WHERE status='active'),(SELECT count(*) FROM access_audit WHERE action IN ('access.platform_admin.bootstrap','access.team.bootstrap'))",
                        [],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(counts, (1, 2));
    }

    #[tokio::test]
    async fn platform_admin_creates_team_and_member_lists_only_its_teams() {
        let (_directory, store, owner) = store().await;
        let member = identity("member");
        seed_principal(&store, "member-principal", "member").await;

        let created = store
            .create_team(CreateTeamInput::new(owner.clone(), "team-a", "Team A").unwrap())
            .await
            .unwrap();
        assert_eq!(created.role, Some(TeamRole::Owner));

        store
            .add_team_member(
                AddTeamMemberInput::new(owner, "team-a", "member-principal", TeamRole::Member)
                    .unwrap(),
            )
            .await
            .unwrap();

        let visible = store.list_teams(member).await.unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].team_id, "team-a");
        assert_eq!(visible[0].role, Some(TeamRole::Member));
    }

    #[tokio::test]
    async fn regular_member_cannot_manage_team_but_team_admin_can() {
        let (_directory, store, owner) = store().await;
        let admin = identity("admin");
        let member = identity("member");
        seed_principal(&store, "admin-principal", "admin").await;
        seed_principal(&store, "member-principal", "member").await;
        store
            .create_team(CreateTeamInput::new(owner.clone(), "team-a", "Team A").unwrap())
            .await
            .unwrap();
        store
            .add_team_member(
                AddTeamMemberInput::new(
                    owner.clone(),
                    "team-a",
                    "admin-principal",
                    TeamRole::Admin,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .add_team_member(
                AddTeamMemberInput::new(owner, "team-a", "member-principal", TeamRole::Member)
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(matches!(
            store.suspend_team(member, "team-a".into()).await,
            Err(AccessStoreError::NotAuthorized)
        ));
        store.suspend_team(admin, "team-a".into()).await.unwrap();
    }

    #[tokio::test]
    async fn team_project_lifecycle_is_manager_owned_and_non_members_cannot_enumerate() {
        let (_directory, store, owner) = store().await;
        let member = identity("member-projects");
        let outsider = identity("outsider-projects");
        seed_principal(&store, "member-projects-principal", "member-projects").await;
        seed_principal(&store, "outsider-projects-principal", "outsider-projects").await;
        store
            .create_team(
                CreateTeamInput::new(owner.clone(), "project-team", "Project Team").unwrap(),
            )
            .await
            .unwrap();
        store
            .add_team_member(
                AddTeamMemberInput::new(
                    owner.clone(),
                    "project-team",
                    "member-projects-principal",
                    TeamRole::Member,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let created = store
            .create_managed_project(
                ManageTeamProjectInput::new(
                    owner.clone(),
                    "project-team",
                    "project-a",
                    Some("Project A".into()),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.team_id, "project-team");
        assert_eq!(
            store
                .list_managed_projects(member.clone())
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .list_managed_projects(outsider)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            store
                .update_managed_project(
                    ManageTeamProjectInput::new(
                        member.clone(),
                        "project-team",
                        "project-a",
                        Some("Nope".into())
                    )
                    .unwrap(),
                    false
                )
                .await,
            Err(AccessStoreError::NotAuthorized)
        ));
        let updated = store
            .update_managed_project(
                ManageTeamProjectInput::new(
                    owner.clone(),
                    "project-team",
                    "project-a",
                    Some("Renamed".into()),
                )
                .unwrap(),
                false,
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        store
            .update_managed_project(
                ManageTeamProjectInput::new(owner, "project-team", "project-a", None).unwrap(),
                true,
            )
            .await
            .unwrap();
        assert!(
            store
                .list_managed_projects(member)
                .await
                .unwrap()
                .is_empty(),
            "archive invalidates future selection/listing"
        );
    }

    #[tokio::test]
    async fn platform_admin_lists_organization_projects_without_team_membership() {
        let (_directory, store, owner) = store().await;
        let platform = identity("project-platform");
        seed_principal(&store, "project-platform-principal", "project-platform").await;
        store
            .grant_platform_administrator(
                PlatformAdministratorInput::new(owner.clone(), "project-platform-principal")
                    .unwrap(),
            )
            .await
            .unwrap();
        store
            .create_team(
                CreateTeamInput::new(owner.clone(), "platform-project-team", "Project Team")
                    .unwrap(),
            )
            .await
            .unwrap();
        store
            .create_managed_project(
                ManageTeamProjectInput::new(
                    owner,
                    "platform-project-team",
                    "platform-visible-project",
                    Some("Visible to Platform".into()),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let visible = store.list_managed_projects(platform).await.unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].project_id, "platform-visible-project");
        assert!(visible[0].can_manage);
    }

    #[tokio::test]
    async fn serialized_transaction_protects_last_active_owner() {
        let (_directory, store, owner) = store().await;
        let demote = AddTeamMemberInput::new(
            owner,
            "bootstrap-initial-team",
            "bootstrap-owner",
            TeamRole::Admin,
        )
        .unwrap();
        assert!(matches!(
            store.set_team_member_role(demote).await,
            Err(AccessStoreError::LastActiveTeamOwner)
        ));

        let state = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT role,status,membership_epoch FROM team_memberships WHERE membership_id='bootstrap-initial-team-owner'",
                        [],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(state, ("owner".into(), "active".into(), 1));
    }

    #[tokio::test]
    async fn audit_failure_rolls_back_team_and_epoch_mutation() {
        let (_directory, store, owner) = store().await;
        store
            .with_connection(|connection| {
                connection.execute_batch("CREATE TEMP TRIGGER fail_team_audit BEFORE INSERT ON access_audit WHEN NEW.action='access.team.create' BEGIN SELECT RAISE(ABORT,'forced'); END;").map_err(map_sqlite_error)
            })
            .await
            .unwrap();

        assert!(
            store
                .create_team(CreateTeamInput::new(owner, "rolled-back", "Rolled Back").unwrap())
                .await
                .is_err()
        );
        let state = store
            .with_connection(|connection| {
                connection.query_row("SELECT (SELECT count(*) FROM groups WHERE group_id='rolled-back'),(SELECT global_revision FROM access_metadata WHERE singleton=1)",[],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?))).map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(state, (0, 1));
    }

    #[tokio::test]
    async fn invitation_is_single_use_identity_bound_and_team_epoch_bound() {
        let (_directory, store, owner) = store().await;
        let member = identity("invited-member");
        seed_principal(&store, "invited-principal", "invited-member").await;
        let token = [7_u8; 32];

        let invitation = store
            .with_connection({
                let owner = owner.clone();
                move |connection| {
                    create_invitation(
                        connection,
                        &CreateTeamInvitationInput::new(
                            owner,
                            "bootstrap-initial-team",
                            "invited-principal",
                            TeamRole::Member,
                            token,
                            600,
                        )?,
                    )
                }
            })
            .await
            .unwrap();
        assert_eq!(invitation.status, "pending");
        assert_eq!(invitation.team_membership_epoch, 1);

        let accepted = store
            .with_connection({
                let member = member.clone();
                move |connection| {
                    accept_invitation(connection, &AcceptTeamInvitationInput::new(member, token)?)
                }
            })
            .await
            .unwrap();
        assert_eq!(accepted.principal_id, "invited-principal");
        assert!(matches!(
            store
                .with_connection(move |connection| {
                    accept_invitation(connection, &AcceptTeamInvitationInput::new(member, token)?)
                })
                .await,
            Err(AccessStoreError::TeamUnavailable)
        ));
    }

    #[tokio::test]
    async fn invitation_rejects_wrong_identity_expiry_and_stale_team_epoch() {
        let (_directory, store, owner) = store().await;
        let invited = identity("invite-target");
        let other = identity("other-target");
        seed_principal(&store, "invite-principal", "invite-target").await;
        seed_principal(&store, "other-principal", "other-target").await;

        let expiring = [8_u8; 32];
        let invitation = store
            .with_connection({
                let owner = owner.clone();
                move |connection| {
                    create_invitation(
                        connection,
                        &CreateTeamInvitationInput::new(
                            owner,
                            "bootstrap-initial-team",
                            "invite-principal",
                            TeamRole::Member,
                            expiring,
                            60,
                        )?,
                    )
                }
            })
            .await
            .unwrap();
        assert!(matches!(
            store
                .with_connection({
                    let other = other.clone();
                    move |connection| {
                        accept_invitation(
                            connection,
                            &AcceptTeamInvitationInput::new(other, expiring)?,
                        )
                    }
                })
                .await,
            Err(AccessStoreError::NotAuthorized)
        ));
        assert!(matches!(
            store
                .with_connection(move |connection| {
                    accept_invitation_at(
                        connection,
                        &AcceptTeamInvitationInput::new(invited, expiring)?,
                        invitation.expires_at,
                    )
                })
                .await,
            Err(AccessStoreError::TeamUnavailable)
        ));

        let stale = [9_u8; 32];
        store
            .with_connection({
                let owner = owner.clone();
                move |connection| {
                    create_invitation(
                        connection,
                        &CreateTeamInvitationInput::new(
                            owner,
                            "bootstrap-initial-team",
                            "invite-principal",
                            TeamRole::Member,
                            stale,
                            600,
                        )?,
                    )
                }
            })
            .await
            .unwrap();
        store
            .add_team_member(
                AddTeamMemberInput::new(
                    owner,
                    "bootstrap-initial-team",
                    "other-principal",
                    TeamRole::Member,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .with_connection(move |connection| {
                    accept_invitation(
                        connection,
                        &AcceptTeamInvitationInput::new(identity("invite-target"), stale)?,
                    )
                })
                .await,
            Err(AccessStoreError::TeamUnavailable)
        ));
    }

    #[tokio::test]
    async fn effective_project_role_is_max_of_direct_and_team_derived() {
        let (_directory, store, owner) = store().await;
        let member = identity("project-member");
        seed_principal(&store, "project-principal", "project-member").await;
        store
            .add_team_member(
                AddTeamMemberInput::new(
                    owner.clone(),
                    "bootstrap-initial-team",
                    "project-principal",
                    TeamRole::Member,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .with_connection({
                let owner = owner.clone();
                move |connection| {
                    assign_team_project(
                        connection,
                        &AssignTeamProjectInput::new(
                            owner,
                            "bootstrap-initial-team",
                            "bootstrap-default",
                            ProjectRole::Admin,
                        )?,
                    )
                }
            })
            .await
            .unwrap();
        store
            .with_connection(|connection| {
                connection.execute("INSERT INTO project_memberships VALUES('direct-viewer','bootstrap-local','bootstrap-default','project-principal','viewer','active','bootstrap-owner',3,3)",[]).map_err(map_sqlite_error)?;
                Ok(())
            })
            .await
            .unwrap();

        let projects = store
            .with_connection(move |connection| list_effective_projects(connection, &member))
            .await
            .unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].project_id, "bootstrap-default");
        assert_eq!(projects[0].role, ProjectRole::Admin);
        assert!(projects[0].direct);
        assert!(projects[0].team_derived);
    }
}
