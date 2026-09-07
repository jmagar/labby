use labby_auth::VerifiedIdentity;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};

use super::domain::{Permission, ProjectRole, validate_loadout_name};
use super::error::{AccessStoreError, AccessStoreResult};
use super::store::map_sqlite_error;

#[derive(Clone)]
pub(crate) struct AssignProjectLoadoutInput {
    identity: VerifiedIdentity,
    project_id: String,
    loadout_name: String,
}

impl AssignProjectLoadoutInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        project_id: impl Into<String>,
        loadout_name: impl Into<String>,
    ) -> AccessStoreResult<Self> {
        let project_id = project_id.into();
        let loadout_name = loadout_name.into();
        if !labby_runtime::gateway_config::is_canonical_project_id(&project_id)
            || validate_loadout_name(&loadout_name).is_err()
        {
            return Err(AccessStoreError::InvalidProjectLoadoutInput);
        }
        Ok(Self {
            identity,
            project_id,
            loadout_name,
        })
    }

    pub(super) fn identity(&self) -> &VerifiedIdentity {
        &self.identity
    }

    pub(super) fn project_id(&self) -> &str {
        &self.project_id
    }

    pub(super) fn loadout_name(&self) -> &str {
        &self.loadout_name
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AssignProjectLoadoutOutcome {
    Assigned,
    AlreadyApplied,
}

pub(super) struct ProjectManager {
    pub(super) principal: super::read::ResolvedPrincipal,
    project_policy_epoch: i64,
    organization_policy_epoch: i64,
}

pub(super) fn resolve_project_manager(
    transaction: &Transaction<'_>,
    identity: &VerifiedIdentity,
    project_id: &str,
) -> AccessStoreResult<ProjectManager> {
    let principal = super::read::resolve_principal(transaction, identity)?;
    let row = transaction
        .query_row(
            "SELECT m.role, m.status, p.status, o.status, p.project_policy_epoch, o.policy_epoch
             FROM project_memberships m
             JOIN projects p ON p.organization_id=m.organization_id AND p.project_id=m.project_id
             JOIN organizations o ON o.organization_id=m.organization_id
             WHERE m.organization_id=?1 AND m.principal_id=?2 AND m.project_id=?3",
            params![principal.organization_id, principal.id, project_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or(AccessStoreError::ProjectAccessUnavailable)?;
    let role = ProjectRole::from_persisted(&row.0).ok_or(AccessStoreError::MalformedVocabulary)?;
    if !matches!(row.1.as_str(), "active" | "suspended" | "disabled")
        || !matches!(row.2.as_str(), "active" | "suspended" | "disabled")
        || !matches!(row.3.as_str(), "active" | "suspended" | "disabled")
    {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    if row.1 != "active"
        || row.2 != "active"
        || row.3 != "active"
        || !role.permissions().contains(&Permission::ProjectManage)
    {
        return Err(AccessStoreError::ProjectAccessUnavailable);
    }
    Ok(ProjectManager {
        principal,
        project_policy_epoch: row.4,
        organization_policy_epoch: row.5,
    })
}

pub(super) fn assign(
    connection: &mut Connection,
    input: &AssignProjectLoadoutInput,
) -> AccessStoreResult<AssignProjectLoadoutOutcome> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    let prior_global_revision: i64 = transaction
        .query_row(
            "SELECT global_revision FROM access_metadata WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let manager = resolve_project_manager(&transaction, &input.identity, &input.project_id)?;
    let principal = &manager.principal;
    let existing = transaction
        .query_row(
            "SELECT loadout_name FROM project_loadouts WHERE organization_id=?1 AND project_id=?2",
            params![principal.organization_id, input.project_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    if existing
        .as_deref()
        .is_some_and(|name| validate_loadout_name(name).is_err())
    {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    match existing.as_deref() {
        Some(name) if name == input.loadout_name => {
            transaction.commit().map_err(map_sqlite_error)?;
            return Ok(AssignProjectLoadoutOutcome::AlreadyApplied);
        }
        Some(_) => return Err(AccessStoreError::ProjectLoadoutConflict),
        None => {}
    }
    let now = unix_now()?;
    transaction.execute(
        "INSERT INTO project_loadouts(organization_id,project_id,loadout_name,created_by,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5)",
        params![principal.organization_id, input.project_id, input.loadout_name, principal.id, now],
    ).map_err(map_sqlite_error)?;
    let global_revision: i64 = transaction.query_row(
        "UPDATE access_metadata SET global_revision=global_revision+1, updated_at=?1 WHERE singleton=1 RETURNING global_revision",
        [now], |r| r.get(0)).map_err(map_sqlite_error)?;
    let organization_epoch: i64 = transaction.query_row(
        "UPDATE organizations SET policy_epoch=policy_epoch+1, updated_at=?1 WHERE organization_id=?2 RETURNING policy_epoch",
        params![now,principal.organization_id], |r| r.get(0)).map_err(map_sqlite_error)?;
    let project_epoch: i64 = transaction.query_row(
        "UPDATE projects SET project_policy_epoch=project_policy_epoch+1, updated_at=?1 WHERE organization_id=?2 AND project_id=?3 RETURNING project_policy_epoch",
        params![now,principal.organization_id,input.project_id], |r| r.get(0)).map_err(map_sqlite_error)?;
    if global_revision
        != prior_global_revision
            .checked_add(1)
            .ok_or(AccessStoreError::MalformedVocabulary)?
        || organization_epoch
            != manager
                .organization_policy_epoch
                .checked_add(1)
                .ok_or(AccessStoreError::MalformedVocabulary)?
        || project_epoch
            != manager
                .project_policy_epoch
                .checked_add(1)
                .ok_or(AccessStoreError::MalformedVocabulary)?
    {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    let fingerprint = target_fingerprint(&principal.organization_id, &input.project_id);
    let event_id = format!("project-loadout-{global_revision}");
    transaction.execute(
        "INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json) VALUES(?1,?2,NULL,?3,?4,?5,'access.project_loadout.assign','project_loadout',?6,'allow','project_manage',?7,'{}')",
        params![event_id,now,principal.id,principal.organization_id,input.project_id,fingerprint,organization_epoch],
    ).map_err(map_sqlite_error)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(AssignProjectLoadoutOutcome::Assigned)
}

fn target_fingerprint(organization_id: &str, project_id: &str) -> String {
    let mut digest = Sha256::new();
    for field in [organization_id.as_bytes(), project_id.as_bytes()] {
        digest.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
        digest.update(field);
    }
    let bytes = digest.finalize();
    let mut fingerprint = String::with_capacity(7 + bytes.len() * 2);
    fingerprint.push_str("sha256:");
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut fingerprint, "{byte:02x}").expect("writing to String cannot fail");
    }
    fingerprint
}

fn unix_now() -> AccessStoreResult<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use crate::access::{BootstrapOwnerInput, store::AccessStore};

    use super::{AccessStoreError, AssignProjectLoadoutInput, AssignProjectLoadoutOutcome};

    fn secure_tempdir() -> tempfile::TempDir {
        super::super::test_support::secure_tempdir()
    }

    fn identity(credential: &str) -> VerifiedIdentity {
        VerifiedIdentity::local_credential(Authenticator::StaticBearer, credential).unwrap()
    }

    async fn bootstrapped() -> (tempfile::TempDir, AccessStore, VerifiedIdentity) {
        let directory = secure_tempdir();
        let owner = identity("static-bearer:owner");
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        (directory, store, owner)
    }

    #[test]
    fn loadout_assignment_requires_a_canonical_validated_name() {
        let owner = identity("static-bearer:owner");
        assert!(AssignProjectLoadoutInput::new(owner.clone(), "project", "production").is_ok());
        assert!(AssignProjectLoadoutInput::new(owner.clone(), "project", " production").is_err());
        assert!(AssignProjectLoadoutInput::new(owner.clone(), "project", "bad\nname").is_err());
        assert!(AssignProjectLoadoutInput::new(owner, "", "production").is_err());
        assert!(
            AssignProjectLoadoutInput::new(
                identity("static-bearer:owner"),
                "x".repeat(129),
                "production"
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn owner_and_admin_can_assign_but_member_and_viewer_cannot() {
        let (_directory, store, _owner) = bootstrapped().await;
        store.seed_loadout_roles_for_test().await.unwrap();

        for (credential, project, allowed) in [
            ("static-bearer:owner", "bootstrap-default", true),
            ("static-bearer:admin", "admin-project", true),
            ("static-bearer:member", "member-project", false),
            ("static-bearer:viewer", "viewer-project", false),
        ] {
            let result = store
                .assign_project_loadout(
                    AssignProjectLoadoutInput::new(identity(credential), project, "production")
                        .unwrap(),
                )
                .await;
            if allowed {
                assert_eq!(result.unwrap(), AssignProjectLoadoutOutcome::Assigned);
            } else {
                assert!(matches!(
                    result,
                    Err(AccessStoreError::ProjectAccessUnavailable)
                ));
            }
        }
    }

    #[tokio::test]
    async fn missing_and_cross_organization_projects_fail_without_writes() {
        let (_directory, store, owner) = bootstrapped().await;
        store.seed_loadout_roles_for_test().await.unwrap();
        let before = store.loadout_state_for_test().await.unwrap();

        for project in ["missing-project", "other-project"] {
            assert!(matches!(
                store
                    .assign_project_loadout(
                        AssignProjectLoadoutInput::new(owner.clone(), project, "production")
                            .unwrap()
                    )
                    .await,
                Err(AccessStoreError::ProjectAccessUnavailable)
            ));
        }
        assert_eq!(store.loadout_state_for_test().await.unwrap(), before);
    }

    #[tokio::test]
    async fn same_assignment_is_no_write_idempotent_and_a_different_name_conflicts() {
        let (_directory, store, owner) = bootstrapped().await;
        let input =
            AssignProjectLoadoutInput::new(owner.clone(), "bootstrap-default", "production")
                .unwrap();
        assert_eq!(
            store.assign_project_loadout(input.clone()).await.unwrap(),
            AssignProjectLoadoutOutcome::Assigned
        );
        let after_first = store.loadout_state_for_test().await.unwrap();
        assert_eq!(
            store.assign_project_loadout(input).await.unwrap(),
            AssignProjectLoadoutOutcome::AlreadyApplied
        );
        assert_eq!(store.loadout_state_for_test().await.unwrap(), after_first);
        assert!(matches!(
            store
                .assign_project_loadout(
                    AssignProjectLoadoutInput::new(owner, "bootstrap-default", "staging").unwrap()
                )
                .await,
            Err(AccessStoreError::ProjectLoadoutConflict)
        ));
        assert_eq!(store.loadout_state_for_test().await.unwrap(), after_first);
    }

    #[tokio::test]
    async fn malformed_persisted_loadout_name_fails_as_corrupt_vocabulary() {
        let (_directory, store, owner) = bootstrapped().await;
        store
            .execute_test_statement(
                "INSERT INTO project_loadouts VALUES('bootstrap-local','bootstrap-default','bad\nname','bootstrap-owner',2,2)",
            )
            .await
            .unwrap();

        assert!(matches!(
            store
                .assign_project_loadout(
                    AssignProjectLoadoutInput::new(owner, "bootstrap-default", "production",)
                        .unwrap(),
                )
                .await,
            Err(AccessStoreError::MalformedVocabulary)
        ));
    }

    #[tokio::test]
    async fn assignment_increments_all_epochs_and_writes_one_redacted_audit_record() {
        let (_directory, store, owner) = bootstrapped().await;
        store
            .assign_project_loadout(
                AssignProjectLoadoutInput::new(owner, "bootstrap-default", "production").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            store.loadout_state_for_test().await.unwrap(),
            (1, 2, 1, 1, 4)
        );
        let audit = store.loadout_audit_for_test().await.unwrap();
        assert_eq!(audit.0, "access.project_loadout.assign");
        assert_eq!(audit.1, "allow");
        assert_eq!(audit.2, "project_manage");
        assert_eq!(audit.3, 1);
        assert!(!audit.4.contains("production"));
    }

    #[tokio::test]
    async fn assigned_mapping_is_immediately_visible_to_project_selection() {
        let (_directory, store, owner) = bootstrapped().await;
        store
            .assign_project_loadout(
                AssignProjectLoadoutInput::new(owner.clone(), "bootstrap-default", "production")
                    .unwrap(),
            )
            .await
            .unwrap();

        let selected = store
            .select_project(owner, "bootstrap-default".to_owned())
            .await
            .unwrap();
        assert_eq!(selected.loadout_name, "production");
        assert_eq!(selected.global_revision, 2);
    }

    #[tokio::test]
    async fn audit_failure_rolls_back_mapping_and_revisions() {
        let (_directory, store, owner) = bootstrapped().await;
        store
            .install_loadout_audit_failure_for_test()
            .await
            .unwrap();
        let before = store.loadout_state_for_test().await.unwrap();
        assert!(
            store
                .assign_project_loadout(
                    AssignProjectLoadoutInput::new(owner, "bootstrap-default", "production")
                        .unwrap(),
                )
                .await
                .is_err()
        );
        assert_eq!(store.loadout_state_for_test().await.unwrap(), before);
    }

    #[tokio::test]
    async fn concurrent_identical_assignments_create_exactly_one_change() {
        let (directory, first, owner) = bootstrapped().await;
        let second = AccessStore::open_existing_current(directory.path().join("access.db"))
            .await
            .unwrap();
        let input =
            AssignProjectLoadoutInput::new(owner, "bootstrap-default", "production").unwrap();
        let (left, right) = tokio::join!(
            first.assign_project_loadout(input.clone()),
            second.assign_project_loadout(input),
        );
        let mut outcomes = [left.unwrap(), right.unwrap()];
        outcomes.sort_by_key(|outcome| match outcome {
            AssignProjectLoadoutOutcome::Assigned => 0,
            AssignProjectLoadoutOutcome::AlreadyApplied => 1,
        });
        assert_eq!(
            outcomes,
            [
                AssignProjectLoadoutOutcome::Assigned,
                AssignProjectLoadoutOutcome::AlreadyApplied
            ]
        );
        assert_eq!(
            first.loadout_state_for_test().await.unwrap(),
            (1, 2, 1, 1, 4)
        );
    }
}
