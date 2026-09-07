use labby_auth::{PrincipalLink, VerifiedIdentity};
use rusqlite::{Connection, TransactionBehavior, params};

use super::error::{AccessStoreError, AccessStoreResult};

pub(super) const ORGANIZATION_ID: &str = "bootstrap-local";
pub(super) const PRINCIPAL_ID: &str = "bootstrap-owner";
pub(super) const PROJECT_ID: &str = "bootstrap-default";
pub(super) const LINK_ID: &str = "bootstrap-owner-link";
pub(super) const MEMBERSHIP_ID: &str = "bootstrap-owner-membership";
pub(super) const AUDIT_ID: &str = "bootstrap-owner-audit";
const MAX_DISPLAY_NAME_LENGTH: usize = 128;

#[derive(Clone, Debug)]
pub(crate) struct BootstrapOwnerInput {
    identity: VerifiedIdentity,
    organization_name: String,
    project_name: String,
}

impl BootstrapOwnerInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        organization_name: impl Into<String>,
        project_name: impl Into<String>,
    ) -> AccessStoreResult<Self> {
        let organization_name = organization_name.into().trim().to_owned();
        let project_name = project_name.into().trim().to_owned();
        if !valid_display_name(&organization_name) || !valid_display_name(&project_name) {
            return Err(AccessStoreError::InvalidBootstrapInput);
        }
        Ok(Self {
            identity,
            organization_name,
            project_name,
        })
    }
}

fn valid_display_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_DISPLAY_NAME_LENGTH
        && !value.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BootstrapOutcome {
    Created,
    AlreadyApplied,
}

pub(super) fn bootstrap_owner(
    connection: &mut Connection,
    input: &BootstrapOwnerInput,
) -> AccessStoreResult<BootstrapOutcome> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(super::store::map_sqlite_error)?;
    let (generation, stored_fingerprint, global_revision): (i64, Option<String>, i64) = transaction.query_row(
        "SELECT bootstrap_generation, bootstrap_identity_fingerprint, global_revision FROM access_metadata WHERE singleton=1", [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(super::store::map_sqlite_error)?;
    let fingerprint = input.identity.safe_fingerprint();
    if generation == 1 {
        super::integrity::validate_bootstrap_state(&transaction, generation)?;
        if stored_fingerprint.as_deref() == Some(&fingerprint)
            && existing_state_matches(&transaction, input)?
        {
            transaction
                .commit()
                .map_err(super::store::map_sqlite_error)?;
            return Ok(BootstrapOutcome::AlreadyApplied);
        }
        return Err(AccessStoreError::BootstrapConflict);
    }
    if generation != 0
        || stored_fingerprint.is_some()
        || global_revision != 0
        || any_business_state(&transaction)?
    {
        return Err(AccessStoreError::BootstrapConflict);
    }
    let now = unix_now()?;
    transaction.execute("INSERT INTO organizations(organization_id,name,status,policy_epoch,created_at,updated_at) VALUES(?1,?2,'active',0,?3,?3)", params![ORGANIZATION_ID,input.organization_name,now]).map_err(super::store::map_sqlite_error)?;
    let owner_label = format!("{} owner", input.organization_name);
    transaction.execute("INSERT INTO principals(principal_id,organization_id,kind,status,display_name,created_at,updated_at) VALUES(?1,?2,'user','active',?3,?4,?4)", params![PRINCIPAL_ID,ORGANIZATION_ID,owner_label,now]).map_err(super::store::map_sqlite_error)?;
    match input.identity.principal_link() {
        PrincipalLink::External { issuer, subject } => transaction.execute("INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES('bootstrap-owner-link',?1,'external',?2,?3,NULL,'active',?4,?5,?6,?6)", params![PRINCIPAL_ID,issuer,subject,i64::try_from(VerifiedIdentity::VERIFICATION_SCHEMA_VERSION).map_err(|_| AccessStoreError::InvalidBootstrapInput)?,i64::try_from(VerifiedIdentity::LINK_SCHEMA_VERSION).map_err(|_| AccessStoreError::InvalidBootstrapInput)?,now]),
        PrincipalLink::LocalCredential { credential_id } => transaction.execute("INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES('bootstrap-owner-link',?1,'local_credential',NULL,NULL,?2,'active',?3,?4,?5,?5)", params![PRINCIPAL_ID,credential_id,i64::try_from(VerifiedIdentity::VERIFICATION_SCHEMA_VERSION).map_err(|_| AccessStoreError::InvalidBootstrapInput)?,i64::try_from(VerifiedIdentity::LINK_SCHEMA_VERSION).map_err(|_| AccessStoreError::InvalidBootstrapInput)?,now]),
    }.map_err(super::store::map_sqlite_error)?;
    transaction.execute("INSERT INTO projects(project_id,organization_id,name,status,project_policy_epoch,created_at,updated_at) VALUES(?1,?2,?3,'active',0,?4,?4)", params![PROJECT_ID,ORGANIZATION_ID,input.project_name,now]).map_err(super::store::map_sqlite_error)?;
    transaction.execute("INSERT INTO project_memberships(membership_id,organization_id,project_id,principal_id,role,status,created_by,created_at,updated_at) VALUES('bootstrap-owner-membership',?1,?2,?3,'owner','active',?3,?4,?4)", params![ORGANIZATION_ID,PROJECT_ID,PRINCIPAL_ID,now]).map_err(super::store::map_sqlite_error)?;
    transaction.execute("INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json) VALUES('bootstrap-owner-audit',?1,NULL,?2,?3,?4,'access.bootstrap_owner','project',?5,'allow','explicit_owner_bootstrap',0,'{}')", params![now,PRINCIPAL_ID,ORGANIZATION_ID,PROJECT_ID,fingerprint]).map_err(super::store::map_sqlite_error)?;
    let changed = transaction.execute("UPDATE access_metadata SET global_revision=global_revision+1, bootstrap_generation=1, bootstrap_identity_fingerprint=?1, updated_at=?2 WHERE singleton=1 AND bootstrap_generation=0 AND bootstrap_identity_fingerprint IS NULL AND global_revision=0", params![fingerprint,now]).map_err(super::store::map_sqlite_error)?;
    if changed != 1 {
        return Err(AccessStoreError::BootstrapConflict);
    }
    transaction
        .commit()
        .map_err(super::store::map_sqlite_error)?;
    Ok(BootstrapOutcome::Created)
}

fn any_business_state(connection: &Connection) -> AccessStoreResult<bool> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM organizations) OR EXISTS(SELECT 1 FROM principals) OR EXISTS(SELECT 1 FROM principal_links) OR EXISTS(SELECT 1 FROM projects) OR EXISTS(SELECT 1 FROM project_memberships) OR EXISTS(SELECT 1 FROM project_loadouts) OR EXISTS(SELECT 1 FROM access_audit)", [], |r| r.get(0)).map_err(super::store::map_sqlite_error)
}

fn existing_state_matches(
    connection: &Connection,
    input: &BootstrapOwnerInput,
) -> AccessStoreResult<bool> {
    let identity_matches = match input.identity.principal_link() {
        PrincipalLink::External { issuer, subject } => connection.query_row("SELECT count(*)=1 FROM principal_links WHERE link_id='bootstrap-owner-link' AND principal_id=?1 AND link_kind='external' AND issuer=?2 AND subject=?3 AND credential_id IS NULL AND status='active'", params![PRINCIPAL_ID,issuer,subject], |r| r.get(0)),
        PrincipalLink::LocalCredential { credential_id } => connection.query_row("SELECT count(*)=1 FROM principal_links WHERE link_id='bootstrap-owner-link' AND principal_id=?1 AND link_kind='local_credential' AND credential_id=?2 AND issuer IS NULL AND subject IS NULL AND status='active'", params![PRINCIPAL_ID,credential_id], |r| r.get(0)),
    }.map_err(super::store::map_sqlite_error)?;
    let shape: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM organizations WHERE organization_id=?1 AND name=?2 AND status='active') AND EXISTS(SELECT 1 FROM principals WHERE principal_id=?3 AND organization_id=?1 AND kind='user' AND status='active') AND EXISTS(SELECT 1 FROM projects WHERE project_id=?4 AND organization_id=?1 AND name=?5 AND status='active') AND EXISTS(SELECT 1 FROM project_memberships WHERE membership_id='bootstrap-owner-membership' AND organization_id=?1 AND project_id=?4 AND principal_id=?3 AND role='owner' AND status='active') AND EXISTS(SELECT 1 FROM access_audit WHERE event_id='bootstrap-owner-audit' AND actor_principal_id=?3 AND organization_id=?1 AND project_id=?4 AND action='access.bootstrap_owner' AND decision='allow' AND reason_code='explicit_owner_bootstrap')", params![ORGANIZATION_ID,input.organization_name,PRINCIPAL_ID,PROJECT_ID,input.project_name], |r| r.get(0)).map_err(super::store::map_sqlite_error)?;
    Ok(identity_matches && shape)
}

fn unix_now() -> AccessStoreResult<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .map_err(|e| AccessStoreError::Unavailable(e.to_string()))
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, PrincipalLink, VerifiedIdentity};

    use super::*;
    use crate::access::store::AccessStore;

    fn secure_tempdir() -> tempfile::TempDir {
        super::super::test_support::secure_tempdir()
    }

    fn input(identity: VerifiedIdentity) -> BootstrapOwnerInput {
        BootstrapOwnerInput::new(identity, "Local", "Default").unwrap()
    }

    #[test]
    fn owner_names_are_normalized_and_validated_at_the_domain_boundary() {
        let identity = VerifiedIdentity::local_credential(
            Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .unwrap();
        let normalized = BootstrapOwnerInput::new(identity.clone(), " Local ", " Default ")
            .expect("valid names");
        assert_eq!(normalized.organization_name, "Local");
        assert_eq!(normalized.project_name, "Default");
        assert!(matches!(
            BootstrapOwnerInput::new(identity.clone(), "", "Default"),
            Err(AccessStoreError::InvalidBootstrapInput)
        ));
        assert!(matches!(
            BootstrapOwnerInput::new(identity.clone(), "Local", "bad\nname"),
            Err(AccessStoreError::InvalidBootstrapInput)
        ));
        assert!(matches!(
            BootstrapOwnerInput::new(identity, "Local", "x".repeat(129)),
            Err(AccessStoreError::InvalidBootstrapInput)
        ));
    }

    #[tokio::test]
    async fn bootstrap_is_atomic_and_idempotent() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        let owner = input(
            VerifiedIdentity::local_credential(
                Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .unwrap(),
        );

        assert_eq!(
            store.bootstrap_owner(owner.clone()).await.unwrap(),
            BootstrapOutcome::Created
        );
        assert_eq!(
            store.bootstrap_owner(owner).await.unwrap(),
            BootstrapOutcome::AlreadyApplied
        );
        drop(store);

        let reopened = AccessStore::open(path).await.unwrap();
        assert_eq!(
            reopened.bootstrap_counts_for_test().await.unwrap(),
            (1, 1, 1, 1, 1, 1)
        );
        assert_eq!(reopened.bootstrap_metadata_for_test().await.unwrap().0, 1);
    }

    #[tokio::test]
    async fn identity_or_configuration_drift_fails_closed() {
        let directory = secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        store
            .bootstrap_owner(input(
                VerifiedIdentity::local_credential(
                    Authenticator::StaticBearer,
                    "static-bearer:primary",
                )
                .unwrap(),
            ))
            .await
            .unwrap();

        let other = input(
            VerifiedIdentity::local_credential(
                Authenticator::UnixPeer,
                "unix-peer:uid=1000:gid=1000",
            )
            .unwrap(),
        );
        assert!(matches!(
            store.bootstrap_owner(other).await,
            Err(AccessStoreError::BootstrapConflict)
        ));
        let renamed = BootstrapOwnerInput::new(
            VerifiedIdentity::local_credential(
                Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .unwrap(),
            "Renamed",
            "Default",
        )
        .unwrap();
        assert!(matches!(
            store.bootstrap_owner(renamed).await,
            Err(AccessStoreError::BootstrapConflict)
        ));
        assert_eq!(
            store.bootstrap_counts_for_test().await.unwrap(),
            (1, 1, 1, 1, 1, 1)
        );
    }

    #[tokio::test]
    async fn external_identity_is_linked_by_canonical_issuer_and_subject() {
        let directory = secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "stable-provider-subject",
        )
        .unwrap();
        store.bootstrap_owner(input(identity)).await.unwrap();
        let linked: bool = store.with_connection(|connection| connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM principal_links WHERE link_kind='external' AND issuer='https://accounts.google.com' AND subject='stable-provider-subject' AND credential_id IS NULL)", [], |row| row.get(0)).map_err(crate::access::store::map_sqlite_error)).await.unwrap();
        assert!(linked);
    }

    #[tokio::test]
    async fn persisted_enterprise_issuer_bootstrap_state_reopens() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .bootstrap_owner(input(
                VerifiedIdentity::local_credential(
                    Authenticator::StaticBearer,
                    "static-bearer:primary",
                )
                .unwrap(),
            ))
            .await
            .unwrap();
        let link = PrincipalLink::External {
            issuer: "https://login.enterprise.example/oidc".to_string(),
            subject: "enterprise-subject".to_string(),
        };
        let fingerprint = link.safe_fingerprint();
        store
            .with_connection(move |connection| {
                connection
                    .execute(
                        "UPDATE principal_links SET link_kind='external', issuer='https://login.enterprise.example/oidc', subject='enterprise-subject', credential_id=NULL WHERE link_id='bootstrap-owner-link'",
                        [],
                    )
                    .map_err(crate::access::store::map_sqlite_error)?;
                connection
                    .execute(
                        "UPDATE access_metadata SET bootstrap_identity_fingerprint=?1 WHERE singleton=1",
                        [&fingerprint],
                    )
                    .map_err(crate::access::store::map_sqlite_error)?;
                connection
                    .execute(
                        "UPDATE access_audit SET target_fingerprint=?1 WHERE event_id='bootstrap-owner-audit'",
                        [&fingerprint],
                    )
                    .map_err(crate::access::store::map_sqlite_error)?;
                Ok(())
            })
            .await
            .unwrap();
        drop(store);

        AccessStore::open(path).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn separate_stores_create_exactly_once_concurrently() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        AccessStore::open(path.clone()).await.unwrap();
        let owner = input(
            VerifiedIdentity::local_credential(
                Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .unwrap(),
        );
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let path = path.clone();
            let owner = owner.clone();
            tasks.push(tokio::spawn(async move {
                AccessStore::open(path)
                    .await
                    .unwrap()
                    .bootstrap_owner(owner)
                    .await
                    .unwrap()
            }));
        }
        let results = futures::future::join_all(tasks).await;
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Ok(BootstrapOutcome::Created)))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Ok(BootstrapOutcome::AlreadyApplied)))
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn partial_state_rolls_back_and_reopen_can_retry() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        store.execute_test_statement("CREATE TRIGGER fail_bootstrap BEFORE INSERT ON access_audit BEGIN SELECT RAISE(ABORT, 'forced'); END;").await.unwrap();
        assert!(
            store
                .bootstrap_owner(input(
                    VerifiedIdentity::local_credential(
                        Authenticator::StaticBearer,
                        "static-bearer:primary"
                    )
                    .unwrap()
                ))
                .await
                .is_err()
        );
        drop(store);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("DROP TRIGGER fail_bootstrap")
            .unwrap();
        drop(connection);
        let reopened = AccessStore::open(path).await.unwrap();
        assert_eq!(
            reopened.bootstrap_counts_for_test().await.unwrap(),
            (0, 0, 0, 0, 0, 0)
        );
        assert_eq!(
            reopened
                .bootstrap_owner(input(
                    VerifiedIdentity::local_credential(
                        Authenticator::StaticBearer,
                        "static-bearer:primary"
                    )
                    .unwrap()
                ))
                .await
                .unwrap(),
            BootstrapOutcome::Created
        );
    }

    #[tokio::test]
    async fn reopen_rejects_partial_bootstrap_state() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .bootstrap_owner(input(
                VerifiedIdentity::local_credential(
                    Authenticator::StaticBearer,
                    "static-bearer:primary",
                )
                .unwrap(),
            ))
            .await
            .unwrap();
        drop(store);
        let connection = Connection::open(&path).unwrap();
        connection.execute("DELETE FROM access_audit", []).unwrap();
        drop(connection);
        assert!(matches!(
            AccessStore::open(path).await,
            Err(AccessStoreError::IntegrityViolation {
                check: "bootstrap_state"
            })
        ));
    }

    #[tokio::test]
    async fn reopen_rejects_reserved_link_substitution_even_when_fingerprints_are_unchanged() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .bootstrap_owner(input(
                VerifiedIdentity::local_credential(
                    Authenticator::StaticBearer,
                    "static-bearer:primary",
                )
                .unwrap(),
            ))
            .await
            .unwrap();
        drop(store);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE principal_links SET credential_id='static-bearer:substituted' WHERE link_id='bootstrap-owner-link'",
                [],
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            AccessStore::open(path).await,
            Err(AccessStoreError::IntegrityViolation {
                check: "bootstrap_state"
            })
        ));
    }

    #[tokio::test]
    async fn unrelated_post_bootstrap_growth_reopens_and_remains_idempotent() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        let owner = input(
            VerifiedIdentity::local_credential(
                Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .unwrap(),
        );
        store.bootstrap_owner(owner.clone()).await.unwrap();
        store.execute_test_statement("INSERT INTO principals VALUES('extra-principal','bootstrap-local','user','active',NULL,1,1); INSERT INTO projects VALUES('extra-project','bootstrap-local','Extra','active',0,1,1); INSERT INTO project_memberships VALUES('extra-membership','bootstrap-local','extra-project','extra-principal','viewer','active','bootstrap-owner',1,1); INSERT INTO project_loadouts VALUES('bootstrap-local','extra-project','extra-loadout','bootstrap-owner',1,1); INSERT INTO access_audit VALUES('extra-audit',1,NULL,'bootstrap-owner','bootstrap-local','extra-project','extra.action','project','extra','allow','test',0,'{}');").await.unwrap();
        drop(store);
        let reopened = AccessStore::open(path).await.unwrap();
        assert_eq!(
            reopened.bootstrap_owner(owner).await.unwrap(),
            BootstrapOutcome::AlreadyApplied
        );
    }

    #[tokio::test]
    async fn generation_zero_allows_unrelated_data_but_bootstrap_requires_pristine_store() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .execute_test_statement(
                "INSERT INTO organizations VALUES('existing-org','Existing','active',0,1,1);",
            )
            .await
            .unwrap();
        drop(store);
        let reopened = AccessStore::open(path).await.unwrap();
        let owner = input(
            VerifiedIdentity::local_credential(
                Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .unwrap(),
        );
        assert!(matches!(
            reopened.bootstrap_owner(owner).await,
            Err(AccessStoreError::BootstrapConflict)
        ));
    }
}
