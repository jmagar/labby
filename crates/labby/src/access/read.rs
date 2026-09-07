use labby_auth::{PrincipalLink, VerifiedIdentity};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::domain::{ProjectRole, validate_loadout_name};
use super::error::{AccessStoreError, AccessStoreResult};
use super::store::map_sqlite_error;

/// Redacted access facts suitable for project selection UI and policy assembly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccessibleProjectSnapshot {
    pub(crate) principal_id: String,
    pub(crate) organization_id: String,
    pub(crate) project_id: String,
    pub(crate) role: ProjectRole,
    pub(crate) loadout_name: Option<String>,
    pub(crate) global_revision: u64,
}

/// A selected project is usable only when its exact persisted loadout exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectAccessSnapshot {
    pub(crate) principal_id: String,
    pub(crate) organization_id: String,
    pub(crate) project_id: String,
    pub(crate) role: ProjectRole,
    pub(crate) loadout_name: String,
    pub(crate) global_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectMembershipSnapshot {
    pub(crate) principal_id: String,
    pub(crate) organization_id: String,
    pub(crate) project_id: String,
    pub(crate) role: ProjectRole,
    pub(crate) global_revision: u64,
}

pub(super) struct ResolvedPrincipal {
    pub(super) id: String,
    pub(super) organization_id: String,
}

pub(super) fn resolve_principal_id(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
) -> AccessStoreResult<super::AccessPrincipalId> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let principal = resolve_principal(&transaction, identity)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(super::AccessPrincipalId(principal.id))
}

pub(super) fn list_accessible_projects(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
) -> AccessStoreResult<Vec<AccessibleProjectSnapshot>> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let revision = global_revision(&transaction)?;
    let principal = resolve_principal(&transaction, identity)?;
    let mut statement = transaction
        .prepare(
            "SELECT p.project_id, m.role, l.loadout_name, m.status, p.status, o.status
             FROM project_memberships m
             JOIN projects p
               ON p.organization_id=m.organization_id AND p.project_id=m.project_id
             JOIN organizations o ON o.organization_id=m.organization_id
             LEFT JOIN project_loadouts l
               ON l.organization_id=p.organization_id AND l.project_id=p.project_id
             WHERE m.organization_id=?1 AND m.principal_id=?2
             ORDER BY p.project_id COLLATE BINARY",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(params![principal.organization_id, principal.id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(map_sqlite_error)?;
    let mut projects = Vec::new();
    for row in rows {
        let (project_id, role, loadout_name, membership_status, project_status, org_status) =
            row.map_err(map_sqlite_error)?;
        if !known_status(&membership_status)
            || !known_status(&project_status)
            || !known_status(&org_status)
        {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        if membership_status != "active" || project_status != "active" || org_status != "active" {
            continue;
        }
        if loadout_name
            .as_deref()
            .is_some_and(|name| validate_loadout_name(name).is_err())
        {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        projects.push(AccessibleProjectSnapshot {
            principal_id: principal.id.clone(),
            organization_id: principal.organization_id.clone(),
            project_id,
            role: ProjectRole::from_persisted(&role)
                .ok_or(AccessStoreError::MalformedVocabulary)?,
            loadout_name,
            global_revision: revision,
        });
    }
    drop(statement);
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(projects)
}

pub(super) fn select_project(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    project_id: &str,
) -> AccessStoreResult<ProjectAccessSnapshot> {
    if project_id.is_empty() {
        return Err(AccessStoreError::ProjectAccessUnavailable);
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let snapshot = select_project_in_transaction(&transaction, identity, project_id)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(snapshot)
}

pub(super) fn select_project_in_transaction(
    transaction: &Transaction<'_>,
    identity: &VerifiedIdentity,
    project_id: &str,
) -> AccessStoreResult<ProjectAccessSnapshot> {
    let revision = global_revision(&transaction)?;
    let principal = resolve_principal(&transaction, identity)?;
    let row = transaction
        .query_row(
            "SELECT m.role, l.loadout_name, m.status, p.status, o.status
             FROM project_memberships m
             JOIN projects p
               ON p.organization_id=m.organization_id AND p.project_id=m.project_id
             JOIN organizations o ON o.organization_id=m.organization_id
             LEFT JOIN project_loadouts l
               ON l.organization_id=p.organization_id AND l.project_id=p.project_id
             WHERE m.organization_id=?1 AND m.principal_id=?2 AND m.project_id=?3
             ",
            params![principal.organization_id, principal.id, project_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or(AccessStoreError::ProjectAccessUnavailable)?;
    if !known_status(&row.2) || !known_status(&row.3) || !known_status(&row.4) {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    if row.2 != "active" || row.3 != "active" || row.4 != "active" {
        return Err(AccessStoreError::ProjectAccessUnavailable);
    }
    let loadout_name = row.1.ok_or(AccessStoreError::ProjectAccessUnavailable)?;
    validate_loadout_name(&loadout_name).map_err(|_| AccessStoreError::MalformedVocabulary)?;
    let snapshot = ProjectAccessSnapshot {
        principal_id: principal.id,
        organization_id: principal.organization_id,
        project_id: project_id.to_owned(),
        role: ProjectRole::from_persisted(&row.0).ok_or(AccessStoreError::MalformedVocabulary)?,
        loadout_name,
        global_revision: revision,
    };
    Ok(snapshot)
}

pub(super) fn select_project_membership_in_transaction(
    transaction: &Transaction<'_>,
    identity: &VerifiedIdentity,
    project_id: &str,
) -> AccessStoreResult<ProjectMembershipSnapshot> {
    if project_id.is_empty() {
        return Err(AccessStoreError::ProjectAccessUnavailable);
    }
    let revision = global_revision(transaction)?;
    let principal = resolve_principal(transaction, identity)?;
    let row = transaction
        .query_row(
            "SELECT m.role, m.status, p.status, o.status
             FROM project_memberships m
             JOIN projects p
               ON p.organization_id=m.organization_id AND p.project_id=m.project_id
             JOIN organizations o ON o.organization_id=m.organization_id
             WHERE m.organization_id=?1 AND m.principal_id=?2 AND m.project_id=?3",
            params![principal.organization_id, principal.id, project_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or(AccessStoreError::ProjectAccessUnavailable)?;
    if !known_status(&row.1) || !known_status(&row.2) || !known_status(&row.3) {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    if row.1 != "active" || row.2 != "active" || row.3 != "active" {
        return Err(AccessStoreError::ProjectAccessUnavailable);
    }
    Ok(ProjectMembershipSnapshot {
        principal_id: principal.id,
        organization_id: principal.organization_id,
        project_id: project_id.to_owned(),
        role: ProjectRole::from_persisted(&row.0).ok_or(AccessStoreError::MalformedVocabulary)?,
        global_revision: revision,
    })
}

fn global_revision(transaction: &Transaction<'_>) -> AccessStoreResult<u64> {
    let revision = transaction
        .query_row(
            "SELECT global_revision FROM access_metadata WHERE singleton=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_sqlite_error)?;
    u64::try_from(revision).map_err(|_| AccessStoreError::MalformedVocabulary)
}

pub(super) fn resolve_principal(
    transaction: &Transaction<'_>,
    identity: &VerifiedIdentity,
) -> AccessStoreResult<ResolvedPrincipal> {
    let (predicate, first, second) = match identity.principal_link() {
        PrincipalLink::External { issuer, subject } => (
            "l.link_kind='external' AND l.issuer=?1 AND l.subject=?2 AND l.credential_id IS NULL",
            issuer.as_str(),
            subject.as_str(),
        ),
        PrincipalLink::LocalCredential { credential_id } => (
            "l.link_kind='local_credential' AND l.credential_id=?1 AND ?2=?2 AND l.issuer IS NULL AND l.subject IS NULL",
            credential_id.as_str(),
            "local",
        ),
    };
    let sql = format!(
        "SELECT p.principal_id, p.organization_id, l.status, p.kind, p.status, o.status,
                l.verification_generation, l.link_generation
         FROM principal_links l
         JOIN principals p ON p.principal_id=l.principal_id
         JOIN organizations o ON o.organization_id=p.organization_id
         WHERE {predicate}"
    );
    let mut statement = transaction.prepare(&sql).map_err(map_sqlite_error)?;
    let mut rows = statement
        .query(params![first, second])
        .map_err(map_sqlite_error)?;
    let Some(row) = rows.next().map_err(map_sqlite_error)? else {
        return Err(AccessStoreError::IdentityUnavailable);
    };
    let resolved = ResolvedPrincipal {
        id: row.get(0).map_err(map_sqlite_error)?,
        organization_id: row.get(1).map_err(map_sqlite_error)?,
    };
    let link_status: String = row.get(2).map_err(map_sqlite_error)?;
    let principal_kind: String = row.get(3).map_err(map_sqlite_error)?;
    let principal_status: String = row.get(4).map_err(map_sqlite_error)?;
    let organization_status: String = row.get(5).map_err(map_sqlite_error)?;
    let verification_generation: i64 = row.get(6).map_err(map_sqlite_error)?;
    let link_generation: i64 = row.get(7).map_err(map_sqlite_error)?;
    if rows.next().map_err(map_sqlite_error)?.is_some() {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    if !matches!(link_status.as_str(), "active" | "revoked")
        || !matches!(principal_kind.as_str(), "user" | "service_account")
        || !known_status(&principal_status)
        || !known_status(&organization_status)
        || verification_generation
            != i64::try_from(VerifiedIdentity::VERIFICATION_SCHEMA_VERSION)
                .map_err(|_| AccessStoreError::MalformedVocabulary)?
        || link_generation
            != i64::try_from(VerifiedIdentity::LINK_SCHEMA_VERSION)
                .map_err(|_| AccessStoreError::MalformedVocabulary)?
    {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    if link_status != "active" || principal_status != "active" || organization_status != "active" {
        return Err(AccessStoreError::IdentityUnavailable);
    }
    Ok(resolved)
}

fn known_status(value: &str) -> bool {
    matches!(value, "active" | "suspended" | "disabled")
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use super::*;
    use crate::access::{BootstrapOwnerInput, store::AccessStore};

    fn secure_tempdir() -> tempfile::TempDir {
        super::super::test_support::secure_tempdir()
    }

    fn external(authenticator: Authenticator) -> VerifiedIdentity {
        VerifiedIdentity::external(authenticator, "https://accounts.google.com", "subject-1")
            .unwrap()
    }

    async fn bootstrapped(identity: VerifiedIdentity) -> (tempfile::TempDir, AccessStore) {
        let directory = secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        store
            .bootstrap_owner(BootstrapOwnerInput::new(identity, "Local", "Default").unwrap())
            .await
            .unwrap();
        (directory, store)
    }

    #[tokio::test]
    async fn provider_transports_converge_and_email_is_not_an_input() {
        let (_directory, store) = bootstrapped(external(Authenticator::BrowserSession)).await;
        store
            .execute_test_statement(
                "INSERT INTO projects VALUES('z-project','bootstrap-local','Z','active',0,2,2),('a-project','bootstrap-local','A','active',0,2,2);
                 INSERT INTO project_memberships VALUES('z-membership','bootstrap-local','z-project','bootstrap-owner','viewer','active','bootstrap-owner',2,2),('a-membership','bootstrap-local','a-project','bootstrap-owner','member','active','bootstrap-owner',2,2);",
            )
            .await
            .unwrap();
        let browser = store
            .list_accessible_projects(external(Authenticator::BrowserSession))
            .await
            .unwrap();
        let bearer = store
            .list_accessible_projects(external(Authenticator::OauthBearer))
            .await
            .unwrap();
        assert_eq!(browser, bearer);
        assert_eq!(
            browser
                .iter()
                .map(|project| project.project_id.as_str())
                .collect::<Vec<_>>(),
            ["a-project", "bootstrap-default", "z-project"]
        );
    }

    #[tokio::test]
    async fn exact_local_credential_id_resolves_without_prefix_matching() {
        let identity = VerifiedIdentity::local_credential(
            Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .unwrap();
        let (_directory, store) = bootstrapped(identity).await;
        let near = VerifiedIdentity::local_credential(
            Authenticator::StaticBearer,
            "static-bearer:primary-extra",
        )
        .unwrap();
        assert!(matches!(
            store.list_accessible_projects(near).await,
            Err(AccessStoreError::IdentityUnavailable)
        ));
    }

    #[tokio::test]
    async fn revoked_link_and_disabled_principal_fail_closed() {
        for sql in [
            "UPDATE principal_links SET status='revoked'",
            "UPDATE principals SET status='disabled'",
        ] {
            let (_directory, store) = bootstrapped(external(Authenticator::BrowserSession)).await;
            store.execute_test_statement(sql).await.unwrap();
            assert!(matches!(
                store
                    .list_accessible_projects(external(Authenticator::BrowserSession))
                    .await,
                Err(AccessStoreError::IdentityUnavailable)
            ));
        }
    }

    #[tokio::test]
    async fn stale_persisted_identity_vocabulary_fails_as_malformed() {
        let (_directory, store) = bootstrapped(external(Authenticator::BrowserSession)).await;
        store
            .execute_test_statement("UPDATE principal_links SET verification_generation=2")
            .await
            .unwrap();
        assert!(matches!(
            store
                .list_accessible_projects(external(Authenticator::BrowserSession))
                .await,
            Err(AccessStoreError::MalformedVocabulary)
        ));
    }

    #[tokio::test]
    async fn inactive_membership_project_and_organization_are_not_accessible() {
        for sql in [
            "UPDATE project_memberships SET status='suspended'",
            "UPDATE projects SET status='disabled'",
        ] {
            let (_directory, store) = bootstrapped(external(Authenticator::BrowserSession)).await;
            store.execute_test_statement(sql).await.unwrap();
            let result = store
                .select_project(
                    external(Authenticator::BrowserSession),
                    "bootstrap-default".into(),
                )
                .await;
            assert!(matches!(
                result,
                Err(AccessStoreError::ProjectAccessUnavailable)
            ));
        }

        let (_directory, store) = bootstrapped(external(Authenticator::BrowserSession)).await;
        store
            .execute_test_statement("UPDATE organizations SET status='suspended'")
            .await
            .unwrap();
        assert!(matches!(
            store
                .select_project(
                    external(Authenticator::BrowserSession),
                    "bootstrap-default".into(),
                )
                .await,
            Err(AccessStoreError::IdentityUnavailable)
        ));
    }

    #[tokio::test]
    async fn other_organization_projects_never_contribute_authority() {
        let (_directory, store) = bootstrapped(external(Authenticator::BrowserSession)).await;
        store
            .execute_test_statement(
                "INSERT INTO organizations VALUES('other-org','Other','active',0,3,3);
                 INSERT INTO principals VALUES('other-principal','other-org','user','active',NULL,3,3);
                 INSERT INTO principal_links VALUES('other-link','other-principal','external','https://accounts.google.com','subject-2',NULL,'active',1,1,3,3);
                 INSERT INTO projects VALUES('other-project','other-org','Other Project','active',0,3,3);
                 INSERT INTO project_memberships VALUES('other-membership','other-org','other-project','other-principal','owner','active','other-principal',3,3);
                 INSERT INTO project_loadouts VALUES('other-org','other-project','Other Loadout','other-principal',3,3);",
            )
            .await
            .unwrap();

        let listed = store
            .list_accessible_projects(external(Authenticator::BrowserSession))
            .await
            .unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|project| project.project_id.as_str())
                .collect::<Vec<_>>(),
            vec!["bootstrap-default"]
        );
        assert!(matches!(
            store
                .select_project(
                    external(Authenticator::BrowserSession),
                    "other-project".into(),
                )
                .await,
            Err(AccessStoreError::ProjectAccessUnavailable)
        ));
    }

    #[tokio::test]
    async fn list_exposes_missing_loadout_but_selection_denies_it_non_enumerating() {
        let (_directory, store) = bootstrapped(external(Authenticator::BrowserSession)).await;
        let listed = store
            .list_accessible_projects(external(Authenticator::BrowserSession))
            .await
            .unwrap();
        assert_eq!(listed[0].loadout_name, None);
        for project_id in ["bootstrap-default", "does-not-exist"] {
            assert!(matches!(
                store
                    .select_project(
                        external(Authenticator::BrowserSession),
                        project_id.to_owned(),
                    )
                    .await,
                Err(AccessStoreError::ProjectAccessUnavailable)
            ));
        }
    }

    #[tokio::test]
    async fn selection_returns_exact_loadout_and_revision_across_restart() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .bootstrap_owner(
                BootstrapOwnerInput::new(
                    external(Authenticator::BrowserSession),
                    "Local",
                    "Default",
                )
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .execute_test_statement(
                "INSERT INTO project_loadouts VALUES('bootstrap-local','bootstrap-default','Exact Loadout','bootstrap-owner',2,2); UPDATE access_metadata SET global_revision=9 WHERE singleton=1;",
            )
            .await
            .unwrap();
        drop(store);
        let reopened = AccessStore::open(path).await.unwrap();
        let selected = reopened
            .select_project(
                external(Authenticator::OauthBearer),
                "bootstrap-default".into(),
            )
            .await
            .unwrap();
        assert_eq!(selected.loadout_name, "Exact Loadout");
        assert_eq!(selected.global_revision, 9);
        assert_eq!(selected.role, ProjectRole::Owner);
    }
}
