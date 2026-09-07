use labby_auth::VerifiedIdentity;
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};

use super::domain::{Permission, ProjectRole};
use super::error::{AccessStoreError, AccessStoreResult};
use super::read::{
    resolve_principal, select_project_in_transaction, select_project_membership_in_transaction,
};
use super::store::map_sqlite_error;

/// Exact request facts for one Project-scoped authorization decision.
///
/// This deliberately does not implement `Debug`: verified identity material must not leak into
/// diagnostics.
pub(crate) struct AuthorizeProjectInput {
    identity: VerifiedIdentity,
    project_id: String,
    permission: Permission,
}

impl AuthorizeProjectInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        project_id: impl Into<String>,
        permission: Permission,
    ) -> Self {
        Self {
            identity,
            project_id: project_id.into(),
            permission,
        }
    }
}

/// Redacted facts from one project-level permission snapshot.
///
/// This is not a reusable dispatch grant: it binds no concrete gateway action, target, or catalog
/// generation. Final dispatch must reauthorize the exact operation at its in-process boundary.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ProjectPermissionSnapshot {
    pub(crate) principal_id: String,
    pub(crate) organization_id: String,
    pub(crate) project_id: String,
    pub(crate) role: ProjectRole,
    pub(crate) loadout_name: String,
    pub(crate) permission: Permission,
    pub(crate) global_revision: u64,
}

/// One exact current membership snapshot for Labby-owned library policy.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct LibraryAccessSnapshot {
    pub(crate) principal_id: String,
    pub(crate) organization_id: String,
    pub(crate) project_id: String,
    pub(crate) role: ProjectRole,
    pub(crate) global_revision: u64,
    pub(crate) team_ids: Vec<String>,
    /// Teams where the principal holds the management capability bundle (Owner/Admin).
    pub(crate) team_management_ids: Vec<String>,
    pub(crate) is_platform_admin: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DepotDelegationAuthoritySnapshot {
    pub(crate) principal_id: String,
    pub(crate) organization_id: String,
    pub(crate) team_id: Option<String>,
    pub(crate) project_id: String,
    pub(crate) platform_administrator: bool,
    pub(crate) authority_schema: u64,
    pub(crate) organization_policy: u64,
    pub(crate) team_membership: Option<u64>,
    pub(crate) team_policy: Option<u64>,
    pub(crate) project_membership: Option<u64>,
    pub(crate) project_policy: u64,
    pub(crate) global_revision: u64,
}

pub(super) fn depot_delegation_authority(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    project_id: &str,
    selected_team_id: Option<&str>,
) -> AccessStoreResult<DepotDelegationAuthoritySnapshot> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let principal = resolve_principal(&transaction, identity).map_err(collapse_denial)?;
    let platform_administrator: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM platform_administrators WHERE principal_id=?1 AND status='active')",
            [&principal.id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let selected = if platform_administrator {
        let (organization_id, global_revision) = transaction
            .query_row(
                "SELECT p.organization_id,m.global_revision FROM projects p
                 JOIN organizations o ON o.organization_id=p.organization_id
                 JOIN access_metadata m ON m.singleton=1
                 WHERE p.project_id=?1 AND p.status='active' AND o.status='active'",
                [project_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(map_sqlite_error)?
            .ok_or(AccessStoreError::ProjectAccessUnavailable)?;
        super::read::ProjectMembershipSnapshot {
            principal_id: principal.id,
            organization_id,
            project_id: project_id.to_owned(),
            role: ProjectRole::Owner,
            global_revision: epoch_u64(global_revision)?,
        }
    } else {
        select_project_membership_in_transaction(&transaction, identity, project_id)
            .map_err(collapse_denial)?
    };
    let (authority_schema, global_revision, organization_policy, project_policy, project_membership) = transaction
        .query_row(
            "SELECT m.schema_version,m.global_revision,o.policy_epoch,p.project_policy_epoch,pm.updated_at
             FROM access_metadata m JOIN organizations o ON o.organization_id=?1
             JOIN projects p ON p.organization_id=o.organization_id AND p.project_id=?2
             LEFT JOIN project_memberships pm ON pm.organization_id=o.organization_id
               AND pm.project_id=p.project_id AND pm.principal_id=?3 AND pm.status='active'
             WHERE m.singleton=1 AND o.status='active' AND p.status='active'",
            params![selected.organization_id, selected.project_id, selected.principal_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?, row.get::<_, Option<i64>>(4)?)),
        )
        .map_err(map_sqlite_error)?;
    let team = if let Some(team_id) = selected_team_id {
        if platform_administrator {
            let policy_epoch = transaction
                .query_row(
                    "SELECT g.policy_epoch FROM groups g
                     JOIN team_project_assignments a ON a.organization_id=g.organization_id
                       AND a.team_id=g.group_id AND a.project_id=?2
                     WHERE g.group_id=?1 AND g.organization_id=?3
                       AND g.status='active' AND a.status='active'",
                    params![team_id, selected.project_id, selected.organization_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| collapse_denial(map_sqlite_error(error)))?;
            Some((team_id.to_owned(), None, policy_epoch))
        } else {
            let epochs = transaction
                .query_row(
                    "SELECT g.membership_epoch,g.policy_epoch FROM team_memberships tm
             JOIN groups g ON g.organization_id=tm.organization_id AND g.group_id=tm.team_id
             JOIN team_project_assignments a ON a.organization_id=tm.organization_id
               AND a.team_id=tm.team_id AND a.project_id=?3
             WHERE tm.organization_id=?1 AND tm.principal_id=?2 AND tm.team_id=?4
               AND tm.status='active' AND g.status='active' AND a.status='active'",
                    params![
                        selected.organization_id,
                        selected.principal_id,
                        selected.project_id,
                        team_id
                    ],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(|error| collapse_denial(map_sqlite_error(error)))?;
            Some((team_id.to_owned(), Some(epochs.0), epochs.1))
        }
    } else {
        None
    };
    let snapshot = DepotDelegationAuthoritySnapshot {
        principal_id: selected.principal_id,
        organization_id: selected.organization_id,
        team_id: team.as_ref().map(|value| value.0.clone()),
        project_id: selected.project_id,
        platform_administrator,
        authority_schema: epoch_u64(authority_schema)?,
        organization_policy: epoch_u64(organization_policy)?,
        team_membership: team
            .as_ref()
            .and_then(|value| value.1)
            .map(epoch_u64)
            .transpose()?,
        team_policy: team
            .as_ref()
            .map(|value| value.2)
            .map(epoch_u64)
            .transpose()?,
        project_membership: project_membership.map(epoch_u64).transpose()?,
        project_policy: epoch_u64(project_policy)?,
        global_revision: epoch_u64(global_revision)?,
    };
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(snapshot)
}

fn epoch_u64(value: i64) -> AccessStoreResult<u64> {
    u64::try_from(value).map_err(|_| AccessStoreError::MalformedVocabulary)
}

pub(super) fn authorize(
    connection: &mut Connection,
    input: &AuthorizeProjectInput,
) -> AccessStoreResult<ProjectPermissionSnapshot> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let selected = select_project_in_transaction(&transaction, &input.identity, &input.project_id)
        .map_err(collapse_denial)?;
    if !selected.role.permissions().contains(&input.permission) {
        return Err(AccessStoreError::NotAuthorized);
    }

    let snapshot = ProjectPermissionSnapshot {
        principal_id: selected.principal_id,
        organization_id: selected.organization_id,
        project_id: selected.project_id,
        role: selected.role,
        loadout_name: selected.loadout_name,
        permission: input.permission,
        global_revision: selected.global_revision,
    };
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(snapshot)
}

pub(super) fn authorize_library(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    project_id: &str,
    permission: Permission,
) -> AccessStoreResult<LibraryAccessSnapshot> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let snapshot =
        authorize_library_in_transaction(&transaction, identity, project_id, permission)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(snapshot)
}

pub(super) fn authorize_library_in_transaction(
    transaction: &Transaction<'_>,
    identity: &VerifiedIdentity,
    project_id: &str,
    permission: Permission,
) -> AccessStoreResult<LibraryAccessSnapshot> {
    let principal = resolve_principal(transaction, identity).map_err(collapse_denial)?;
    let is_platform_admin = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM platform_administrators
             WHERE principal_id=?1 AND status='active')",
            params![principal.id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let selected = if is_platform_admin {
        let (organization_id, global_revision) = transaction
            .query_row(
                "SELECT p.organization_id,m.global_revision FROM projects p
                 JOIN organizations o ON o.organization_id=p.organization_id
                 JOIN access_metadata m ON m.singleton=1
                 WHERE p.project_id=?1 AND p.status='active' AND o.status='active'",
                [project_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(map_sqlite_error)?
            .ok_or(AccessStoreError::ProjectAccessUnavailable)?;
        super::read::ProjectMembershipSnapshot {
            principal_id: principal.id,
            organization_id,
            project_id: project_id.to_owned(),
            role: ProjectRole::Owner,
            global_revision: epoch_u64(global_revision)?,
        }
    } else {
        let selected = select_project_membership_in_transaction(transaction, identity, project_id)
            .map_err(collapse_denial)?;
        if !selected.role.permissions().contains(&permission) {
            return Err(AccessStoreError::NotAuthorized);
        }
        selected
    };
    let (team_ids, team_management_ids) = {
        let mut statement = transaction
            .prepare(if is_platform_admin {
                "SELECT assignment.team_id, 'owner'
                 FROM team_project_assignments assignment
                 JOIN groups g ON g.organization_id=assignment.organization_id
                   AND g.group_id=assignment.team_id
                 WHERE assignment.organization_id=?1 AND assignment.status='active'
                   AND assignment.project_id=?3 AND g.status='active'
                 ORDER BY assignment.team_id"
            } else {
                "SELECT tm.team_id, tm.role
                 FROM team_memberships tm
                 JOIN team_project_assignments assignment
                   ON assignment.organization_id=tm.organization_id
                  AND assignment.team_id=tm.team_id
                 WHERE tm.organization_id=?1 AND tm.principal_id=?2
                   AND tm.status='active' AND assignment.status='active'
                   AND assignment.project_id=?3
                 ORDER BY tm.team_id"
            })
            .map_err(map_sqlite_error)?;
        statement
            .query_map(
                params![
                    selected.organization_id,
                    selected.principal_id,
                    selected.project_id
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(map_sqlite_error)?
            .collect::<Result<Vec<(String, String)>, _>>()
            .map_err(map_sqlite_error)?
            .into_iter()
            .fold(
                (Vec::new(), Vec::new()),
                |(mut all, mut management), (id, role)| {
                    all.push(id.clone());
                    if matches!(role.as_str(), "owner" | "admin") {
                        management.push(id);
                    }
                    (all, management)
                },
            )
    };
    let snapshot = LibraryAccessSnapshot {
        principal_id: selected.principal_id,
        organization_id: selected.organization_id,
        project_id: selected.project_id,
        role: selected.role,
        global_revision: selected.global_revision,
        team_ids,
        team_management_ids,
        is_platform_admin,
    };
    Ok(snapshot)
}

/// Authorizes management of a Project without consulting its Loadout mapping.
///
/// This narrow preflight exists for the operation that creates that mapping. It deliberately
/// returns no reusable grant; the mutation reauthorizes in its own immediate transaction.
pub(super) fn authorize_management_without_loadout(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    project_id: &str,
) -> AccessStoreResult<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    super::loadout::resolve_project_manager(&transaction, identity, project_id)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(())
}

fn collapse_denial(error: AccessStoreError) -> AccessStoreError {
    match error {
        AccessStoreError::IdentityUnavailable | AccessStoreError::ProjectAccessUnavailable => {
            AccessStoreError::NotAuthorized
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use super::*;
    use crate::access::{AccessStore, BootstrapOwnerInput};

    fn secure_tempdir() -> tempfile::TempDir {
        super::super::test_support::secure_tempdir()
    }

    fn identity(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    async fn fixture() -> (tempfile::TempDir, AccessStore, VerifiedIdentity) {
        let directory = secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = identity("owner-subject");
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        store
            .execute_test_statement(
                "INSERT INTO projects VALUES
                   ('admin-project','bootstrap-local','Admin','active',0,2,2),
                   ('member-project','bootstrap-local','Member','active',0,2,2),
                   ('viewer-project','bootstrap-local','Viewer','active',0,2,2),
                   ('unmapped-project','bootstrap-local','Unmapped','active',0,2,2);
                 INSERT INTO project_memberships VALUES
                   ('admin-membership','bootstrap-local','admin-project','bootstrap-owner','admin','active','bootstrap-owner',2,2),
                   ('member-membership','bootstrap-local','member-project','bootstrap-owner','member','active','bootstrap-owner',2,2),
                   ('viewer-membership','bootstrap-local','viewer-project','bootstrap-owner','viewer','active','bootstrap-owner',2,2),
                   ('unmapped-membership','bootstrap-local','unmapped-project','bootstrap-owner','owner','active','bootstrap-owner',2,2);
                 INSERT INTO project_loadouts VALUES
                   ('bootstrap-local','bootstrap-default','production','bootstrap-owner',2,2),
                   ('bootstrap-local','admin-project','production','bootstrap-owner',2,2),
                   ('bootstrap-local','member-project','production','bootstrap-owner',2,2),
                   ('bootstrap-local','viewer-project','production','bootstrap-owner',2,2);
                 INSERT INTO organizations VALUES('other-org','Other','active',0,2,2);
                 INSERT INTO projects VALUES('other-project','other-org','Other','active',0,2,2);",
            )
            .await
            .unwrap();
        (directory, store, owner)
    }

    async fn decision(
        store: &AccessStore,
        identity: VerifiedIdentity,
        project_id: &str,
        permission: Permission,
    ) -> AccessStoreResult<ProjectPermissionSnapshot> {
        store
            .authorize_project(AuthorizeProjectInput::new(identity, project_id, permission))
            .await
    }

    #[tokio::test]
    async fn role_permission_matrix_uses_the_canonical_role_permissions() {
        let (_directory, store, owner) = fixture().await;
        let cases = [
            ("bootstrap-default", ProjectRole::Owner),
            ("admin-project", ProjectRole::Admin),
            ("member-project", ProjectRole::Member),
            ("viewer-project", ProjectRole::Viewer),
        ];
        let permissions = [
            Permission::ProjectRead,
            Permission::ProjectManage,
            Permission::AssetDiscover,
            Permission::AssetUse,
        ];

        for (project_id, role) in cases {
            for permission in permissions {
                let result = decision(&store, owner.clone(), project_id, permission).await;
                assert_eq!(
                    result.is_ok(),
                    role.permissions().contains(&permission),
                    "{role:?} {permission:?}"
                );
                if let Err(error) = result {
                    assert!(matches!(error, AccessStoreError::NotAuthorized));
                }
            }
        }
    }

    #[tokio::test]
    async fn allowed_snapshot_contains_only_exact_redacted_facts_and_revision() {
        let (_directory, store, owner) = fixture().await;
        let snapshot = decision(&store, owner, "member-project", Permission::AssetUse)
            .await
            .unwrap();

        assert_eq!(
            snapshot,
            ProjectPermissionSnapshot {
                principal_id: "bootstrap-owner".into(),
                organization_id: "bootstrap-local".into(),
                project_id: "member-project".into(),
                role: ProjectRole::Member,
                loadout_name: "production".into(),
                permission: Permission::AssetUse,
                global_revision: 1,
            }
        );
    }

    #[tokio::test]
    async fn ordinary_denials_are_indistinguishable_and_cross_org_is_denied() {
        let (_directory, store, owner) = fixture().await;
        let denials = [
            decision(
                &store,
                identity("unknown"),
                "member-project",
                Permission::ProjectRead,
            )
            .await,
            decision(
                &store,
                owner.clone(),
                "missing-project",
                Permission::ProjectRead,
            )
            .await,
            decision(
                &store,
                owner.clone(),
                "unmapped-project",
                Permission::ProjectRead,
            )
            .await,
            decision(
                &store,
                owner.clone(),
                "viewer-project",
                Permission::AssetUse,
            )
            .await,
            decision(&store, owner, "other-project", Permission::ProjectRead).await,
        ];
        for denial in denials {
            assert!(matches!(denial, Err(AccessStoreError::NotAuthorized)));
        }
    }

    #[tokio::test]
    async fn platform_administrator_can_authorize_an_unjoined_project() {
        let (_directory, store, owner) = fixture().await;

        let library = store
            .authorize_skill_library(
                owner.clone(),
                "other-project".to_owned(),
                Permission::AssetUse,
            )
            .await
            .unwrap();
        assert!(library.is_platform_admin);
        assert_eq!(library.organization_id, "other-org");
        assert_eq!(library.project_id, "other-project");

        let delegated = store
            .depot_delegation_authority(owner, "other-project".to_owned(), None)
            .await
            .unwrap();
        assert!(delegated.platform_administrator);
        assert_eq!(delegated.organization_id, "other-org");
        assert_eq!(delegated.project_id, "other-project");
        assert_eq!(delegated.team_id, None);
    }

    #[tokio::test]
    async fn each_call_observes_revocation_and_authorization_never_writes_or_audits() {
        let (_directory, store, owner) = fixture().await;
        let before = store.loadout_state_for_test().await.unwrap();
        decision(
            &store,
            owner.clone(),
            "member-project",
            Permission::AssetUse,
        )
        .await
        .unwrap();
        let after_allow = store.loadout_state_for_test().await.unwrap();
        assert_eq!(after_allow, before);

        store
            .execute_test_statement(
                "UPDATE project_memberships SET status='disabled'
                 WHERE membership_id='member-membership';",
            )
            .await
            .unwrap();
        let denial = decision(&store, owner, "member-project", Permission::AssetUse).await;
        assert!(matches!(denial, Err(AccessStoreError::NotAuthorized)));
        assert_eq!(store.loadout_state_for_test().await.unwrap(), before);
    }

    #[tokio::test]
    async fn malformed_persisted_vocabulary_remains_typed() {
        let (_directory, store, owner) = fixture().await;
        store
            .execute_test_statement(
                "UPDATE project_loadouts SET loadout_name='bad
name'
                 WHERE project_id='member-project';",
            )
            .await
            .unwrap();
        let result = decision(&store, owner, "member-project", Permission::ProjectRead).await;
        assert!(matches!(result, Err(AccessStoreError::MalformedVocabulary)));
    }
}
