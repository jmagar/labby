use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rusqlite::TransactionBehavior;
#[cfg(test)]
use rusqlite::types::Value;
use rusqlite::{Connection, ErrorCode, OpenFlags};

use super::authorization::{
    AuthorizeProjectInput, LibraryAccessSnapshot, ProjectPermissionSnapshot,
};
use super::bootstrap::{BootstrapOutcome, BootstrapOwnerInput, bootstrap_owner};
use super::error::{AccessStoreError, AccessStoreResult};
use super::loadout::{AssignProjectLoadoutInput, AssignProjectLoadoutOutcome};
use super::read::{AccessibleProjectSnapshot, ProjectAccessSnapshot};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Clone)]
pub(crate) struct AccessStore {
    connection: Arc<Mutex<Connection>>,
    connection_admission: Arc<tokio::sync::Semaphore>,
    file_stash_principal_gates: Arc<Mutex<HashMap<String, Weak<tokio::sync::RwLock<()>>>>>,
    path: Arc<PathBuf>,
    #[cfg(test)]
    skill_library_authorizations: Arc<AtomicUsize>,
}

impl std::fmt::Debug for AccessStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccessStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl AccessStore {
    fn file_stash_principal_gate(&self, principal: &str) -> Arc<tokio::sync::RwLock<()>> {
        let mut gates = self
            .file_stash_principal_gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(principal).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(tokio::sync::RwLock::new(()));
        gates.insert(principal.to_owned(), Arc::downgrade(&gate));
        gate
    }

    pub(crate) async fn open(path: PathBuf) -> AccessStoreResult<Self> {
        let path = validated_access_path(&path)
            .map_err(|()| AccessStoreError::InsecurePath { path: path.clone() })?;
        let open_path = path.clone();
        let connection = tokio::task::spawn_blocking(move || open_connection(&open_path))
            .await
            .map_err(|error| AccessStoreError::Unavailable(error.to_string()))??;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            connection_admission: Arc::new(tokio::sync::Semaphore::new(1)),
            file_stash_principal_gates: Arc::new(Mutex::new(HashMap::new())),
            path: Arc::new(path),
            #[cfg(test)]
            skill_library_authorizations: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Opens an already-bootstrapped store at the exact current schema without creating or
    /// migrating any persistent state.
    pub(crate) async fn open_existing_current(path: PathBuf) -> AccessStoreResult<Self> {
        let path = validated_access_path(&path)
            .map_err(|()| AccessStoreError::InsecurePath { path: path.clone() })?;
        let open_path = path.clone();
        let connection =
            tokio::task::spawn_blocking(move || open_existing_current_connection(&open_path))
                .await
                .map_err(|error| AccessStoreError::Unavailable(error.to_string()))??;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            connection_admission: Arc::new(tokio::sync::Semaphore::new(1)),
            file_stash_principal_gates: Arc::new(Mutex::new(HashMap::new())),
            path: Arc::new(path),
            #[cfg(test)]
            skill_library_authorizations: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub(super) async fn with_connection<T, F>(&self, operation: F) -> AccessStoreResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> AccessStoreResult<T> + Send + 'static,
    {
        // Admit only work that can immediately own the single SQLite connection.
        // Otherwise every concurrent caller occupies a blocking-pool worker while
        // waiting on the std mutex, which can starve unrelated blocking work.
        let permit = Arc::clone(&self.connection_admission)
            .acquire_owned()
            .await
            .map_err(|_| AccessStoreError::Unavailable("connection admission closed".into()))?;
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            // Recover from poisoning rather than wedging the store for the
            // process lifetime. Callers now run arbitrary executor closures under
            // this lock (`authorize_skill_library_and_execute`), so one panic must
            // not make every subsequent authorization permanently unavailable. An
            // unwind drops any open `Transaction`, which rolls back, so the
            // connection is left consistent.
            let mut connection = connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            operation(&mut connection)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn bootstrap_owner(
        &self,
        input: BootstrapOwnerInput,
    ) -> AccessStoreResult<BootstrapOutcome> {
        self.with_connection(move |connection| bootstrap_owner(connection, &input))
            .await
    }

    pub(crate) async fn provision_file_stash_recipient_fixture(
        &self,
        owner_credential_id: String,
        principal_id: String,
        display_name: String,
        recipient_credential_id: String,
    ) -> AccessStoreResult<()> {
        if principal_id.is_empty()
            || principal_id.len() > 255
            || principal_id.chars().any(char::is_control)
            || display_name.trim().is_empty()
            || display_name.len() > 255
            || display_name.chars().any(char::is_control)
            || recipient_credential_id.is_empty()
            || recipient_credential_id.len() > 255
            || recipient_credential_id.chars().any(char::is_control)
        {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let organization_id = transaction
                .query_row(
                    "SELECT p.organization_id FROM principals p JOIN principal_links l ON l.principal_id=p.principal_id JOIN organizations o ON o.organization_id=p.organization_id WHERE l.credential_id=?1 AND l.status='active' AND p.status='active' AND o.status='active'",
                    [&owner_credential_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_error)?;
            let _revision: i64 = transaction
                .query_row(
                    "UPDATE access_metadata SET global_revision=global_revision+1,updated_at=unixepoch() WHERE singleton=1 RETURNING global_revision",
                    [],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO principals(principal_id,organization_id,kind,status,display_name,created_at,updated_at) VALUES(?1,?2,'user','active',?3,unixepoch(),unixepoch())",
                    rusqlite::params![principal_id, organization_id, display_name],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'local_credential',NULL,NULL,?3,'active',1,1,unixepoch(),unixepoch())",
                    rusqlite::params![format!("credential-link:{recipient_credential_id}"), principal_id, recipient_credential_id],
                )
                .map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)
        })
        .await
    }

    pub(crate) async fn list_accessible_projects(
        &self,
        identity: labby_auth::VerifiedIdentity,
    ) -> AccessStoreResult<Vec<AccessibleProjectSnapshot>> {
        self.with_connection(move |connection| {
            super::read::list_accessible_projects(connection, &identity)
        })
        .await
    }

    pub(crate) async fn resolve_file_stash_principal(
        &self,
        identity: labby_auth::VerifiedIdentity,
    ) -> AccessStoreResult<super::AccessPrincipalId> {
        self.with_connection(move |connection| {
            super::read::resolve_principal_id(connection, &identity)
        })
        .await
    }

    /// Resolve an authenticated identity while retaining exclusive mutation
    /// admission. Callers keep the returned lease through the Stash
    /// authorization/commit linearization point, so deactivation cannot race
    /// between identity resolution and the dependent Stash operation.
    pub(crate) async fn resolve_and_lease_file_stash_principal(
        &self,
        identity: labby_auth::VerifiedIdentity,
    ) -> AccessStoreResult<(
        super::AccessPrincipalId,
        super::ActiveFileStashPrincipalLease,
    )> {
        let principal = self.resolve_file_stash_principal(identity.clone()).await?;
        let guard = self
            .file_stash_principal_gate(principal.as_str())
            .read_owned()
            .await;
        let confirmed = self.resolve_file_stash_principal(identity).await?;
        if confirmed != principal {
            return Err(AccessStoreError::IdentityUnavailable);
        }
        Ok((
            principal,
            super::ActiveFileStashPrincipalLease {
                _guards: vec![guard],
            },
        ))
    }

    pub(crate) async fn resolve_and_lease_file_stash_participants(
        &self,
        identity: labby_auth::VerifiedIdentity,
        recipient: String,
    ) -> AccessStoreResult<(
        super::AccessPrincipalId,
        super::AccessPrincipalId,
        super::ActiveFileStashPrincipalLease,
    )> {
        let recipient = recipient.trim().to_owned();
        if recipient.is_empty() || recipient.len() > 255 || recipient.chars().any(char::is_control)
        {
            return Err(AccessStoreError::IdentityUnavailable);
        }
        let owner = self.resolve_file_stash_principal(identity.clone()).await?;
        let mut ids = [owner.as_str().to_owned(), recipient.clone()];
        ids.sort();
        let first = self.file_stash_principal_gate(&ids[0]).read_owned().await;
        let second = self.file_stash_principal_gate(&ids[1]).read_owned().await;
        let owner_for_query = owner.clone();
        let recipient_for_query = recipient.clone();
        let confirmed = self
            .with_connection(move |connection| {
            let resolved = super::read::resolve_principal_id(connection, &identity)?;
            if resolved != owner_for_query {
                return Err(AccessStoreError::IdentityUnavailable);
            }
            let active = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM principals owner JOIN principals recipient ON recipient.organization_id=owner.organization_id JOIN organizations o ON o.organization_id=owner.organization_id WHERE owner.principal_id=?1 AND owner.status='active' AND recipient.principal_id=?2 AND recipient.status='active' AND o.status='active')",
                    rusqlite::params![resolved.as_str(), recipient_for_query],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(map_sqlite_error)?;
            if !active {
                return Err(AccessStoreError::IdentityUnavailable);
            }
            Ok(())
        })
        .await;
        confirmed?;
        Ok((
            owner,
            super::AccessPrincipalId(recipient),
            super::ActiveFileStashPrincipalLease {
                _guards: vec![first, second],
            },
        ))
    }

    pub(crate) async fn search_file_stash_recipients(
        &self,
        owner: super::AccessPrincipalId,
        query: String,
        limit: usize,
        deadline: Duration,
    ) -> AccessStoreResult<Vec<super::FileStashRecipient>> {
        let deadline_at = tokio::time::Instant::now() + deadline;
        let permit = tokio::time::timeout_at(
            deadline_at,
            Arc::clone(&self.connection_admission).acquire_owned(),
        )
        .await
        .map_err(|_| AccessStoreError::Unavailable("recipient search deadline exceeded".into()))?
        .map_err(|_| AccessStoreError::Unavailable("connection admission closed".into()))?;
        let connection = Arc::clone(&self.connection);
        // The interrupt handle is safe to invoke from another thread and is
        // the only way an async deadline can actually stop SQLite work already
        // running on the blocking pool.
        let interrupt = connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_interrupt_handle();
        let mut operation = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let connection = connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
            let mut statement = connection.prepare(
                "SELECT candidate.principal_id, candidate.display_name FROM principals owner JOIN principals candidate ON candidate.organization_id=owner.organization_id WHERE owner.principal_id=?1 AND owner.status='active' AND candidate.status='active' AND candidate.principal_id<>owner.principal_id AND candidate.display_name IS NOT NULL AND candidate.display_name LIKE ?2 ESCAPE '\\' COLLATE NOCASE ORDER BY candidate.display_name,candidate.principal_id LIMIT ?3"
            ).map_err(map_sqlite_error)?;
            let rows = statement
                .query_map(
                    rusqlite::params![owner.as_str(), pattern, i64::try_from(limit).unwrap_or(20)],
                    |row| {
                        Ok(super::FileStashRecipient {
                            principal_id: row.get(0)?,
                            display_name: row.get(1)?,
                        })
                    },
                )
                .map_err(map_sqlite_error)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_error)
        });
        if deadline.is_zero() || tokio::time::Instant::now() >= deadline_at {
            interrupt.interrupt();
            drop(operation.await);
            return Err(AccessStoreError::Unavailable(
                "recipient search deadline exceeded".into(),
            ));
        }
        let deadline = tokio::time::sleep_until(deadline_at);
        tokio::pin!(deadline);
        tokio::select! {
            biased;
            () = &mut deadline => {
                interrupt.interrupt();
                // Do not return capacity until SQLite has acknowledged the
                // interrupt and the blocking worker has dropped its lease.
                drop(operation.await);
                Err(AccessStoreError::Unavailable("recipient search deadline exceeded".into()))
            },
            result = &mut operation => result
                .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?,
        }
    }

    pub(crate) async fn lease_active_file_stash_principal(
        &self,
        principal: super::AccessPrincipalId,
    ) -> AccessStoreResult<super::ActiveFileStashPrincipalLease> {
        let guard = self
            .file_stash_principal_gate(principal.as_str())
            .read_owned()
            .await;
        let principal_for_query = principal.clone();
        let active = self
            .with_connection(move |connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM principals p JOIN organizations o ON o.organization_id=p.organization_id WHERE p.principal_id=?1 AND p.status='active' AND o.status='active')",
                    [principal_for_query.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(map_sqlite_error)
        })
        .await?;
        if !active {
            return Err(AccessStoreError::IdentityUnavailable);
        }
        Ok(super::ActiveFileStashPrincipalLease {
            _guards: vec![guard],
        })
    }

    pub(crate) async fn lease_file_stash_participants(
        &self,
        owner: super::AccessPrincipalId,
        recipient: String,
    ) -> AccessStoreResult<(
        super::AccessPrincipalId,
        super::ActiveFileStashPrincipalLease,
    )> {
        let recipient = recipient.trim().to_owned();
        if recipient.is_empty() || recipient.len() > 255 || recipient.chars().any(char::is_control)
        {
            return Err(AccessStoreError::IdentityUnavailable);
        }
        let mut ids = [owner.as_str().to_owned(), recipient.clone()];
        ids.sort();
        let first = self.file_stash_principal_gate(&ids[0]).read_owned().await;
        let second = self.file_stash_principal_gate(&ids[1]).read_owned().await;
        let owner_for_query = owner.clone();
        let recipient_for_query = recipient.clone();
        let active = self
            .with_connection(move |connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM principals owner JOIN principals recipient ON recipient.organization_id=owner.organization_id JOIN organizations o ON o.organization_id=owner.organization_id WHERE owner.principal_id=?1 AND owner.status='active' AND recipient.principal_id=?2 AND recipient.status='active' AND o.status='active')",
                    rusqlite::params![owner_for_query.as_str(), recipient_for_query],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(map_sqlite_error)
        })
        .await?;
        if !active {
            return Err(AccessStoreError::IdentityUnavailable);
        }
        Ok((
            super::AccessPrincipalId(recipient),
            super::ActiveFileStashPrincipalLease {
                _guards: vec![first, second],
            },
        ))
    }

    pub(crate) async fn select_project(
        &self,
        identity: labby_auth::VerifiedIdentity,
        project_id: String,
    ) -> AccessStoreResult<ProjectAccessSnapshot> {
        self.with_connection(move |connection| {
            super::read::select_project(connection, &identity, &project_id)
        })
        .await
    }

    pub(crate) async fn assign_project_loadout(
        &self,
        input: AssignProjectLoadoutInput,
    ) -> AccessStoreResult<AssignProjectLoadoutOutcome> {
        self.with_connection(move |connection| super::loadout::assign(connection, &input))
            .await
    }

    pub(crate) async fn authorize_project(
        &self,
        input: AuthorizeProjectInput,
    ) -> AccessStoreResult<ProjectPermissionSnapshot> {
        self.with_connection(move |connection| super::authorization::authorize(connection, &input))
            .await
    }

    pub(crate) async fn authorize_skill_library(
        &self,
        identity: labby_auth::VerifiedIdentity,
        project_id: String,
        permission: super::domain::Permission,
    ) -> AccessStoreResult<LibraryAccessSnapshot> {
        #[cfg(test)]
        self.skill_library_authorizations
            .fetch_add(1, Ordering::Relaxed);
        self.with_connection(move |connection| {
            super::authorization::authorize_library(connection, &identity, &project_id, permission)
        })
        .await
    }

    /// Holds SQLite's writer reservation from the current membership read through one
    /// synchronous library commit, linearizing that commit with membership revocation.
    ///
    /// `executor` runs while this task owns the single connection admission permit
    /// *and* the connection mutex, so for its whole duration every access-store
    /// operation process-wide is blocked. It must stay short and synchronous, and it
    /// must never call back into `AccessStore` — doing so deadlocks against the
    /// permit it is already holding.
    pub(crate) async fn authorize_skill_library_and_execute<T, E>(
        &self,
        identity: labby_auth::VerifiedIdentity,
        project_id: String,
        permission: super::domain::Permission,
        executor: impl FnOnce(LibraryAccessSnapshot) -> Result<T, E> + Send + 'static,
    ) -> AccessStoreResult<Result<T, E>>
    where
        T: Send + 'static,
        E: Send + 'static,
    {
        #[cfg(test)]
        self.skill_library_authorizations
            .fetch_add(1, Ordering::Relaxed);
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let snapshot = super::authorization::authorize_library_in_transaction(
                &transaction,
                &identity,
                &project_id,
                permission,
            )?;
            let result = executor(snapshot);
            if let Err(error) = transaction.rollback() {
                // The executor has already run and, on success, already committed
                // its own durable state. Reporting the lease rollback as the
                // operation's result would tell the caller a completed commit
                // failed, and the retry would duplicate the work. Record the
                // rollback failure as observability and return the real outcome.
                tracing::error!(
                    project_id,
                    executor_failed = result.is_err(),
                    error = %error,
                    "failed to roll back the Skill Library authorization lease; \
                     the executor outcome is still authoritative"
                );
            }
            Ok(result)
        })
        .await
    }

    #[cfg(test)]
    pub(crate) fn skill_library_authorization_count_for_test(&self) -> usize {
        self.skill_library_authorizations.load(Ordering::Relaxed)
    }

    pub(crate) async fn authorize_project_management_without_loadout(
        &self,
        identity: labby_auth::VerifiedIdentity,
        project_id: String,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            super::authorization::authorize_management_without_loadout(
                connection,
                &identity,
                &project_id,
            )
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn seed_loadout_roles_for_test(&self) -> AccessStoreResult<()> {
        self.execute_test_statement(
            "INSERT INTO principals VALUES
               ('admin-principal','bootstrap-local','user','active',NULL,2,2),
               ('member-principal','bootstrap-local','user','active',NULL,2,2),
               ('viewer-principal','bootstrap-local','user','active',NULL,2,2);
             INSERT INTO principal_links VALUES
               ('admin-link','admin-principal','local_credential',NULL,NULL,'static-bearer:admin','active',1,1,2,2),
               ('member-link','member-principal','local_credential',NULL,NULL,'static-bearer:member','active',1,1,2,2),
               ('viewer-link','viewer-principal','local_credential',NULL,NULL,'static-bearer:viewer','active',1,1,2,2);
             INSERT INTO projects VALUES
               ('admin-project','bootstrap-local','Admin','active',0,2,2),
               ('member-project','bootstrap-local','Member','active',0,2,2),
               ('viewer-project','bootstrap-local','Viewer','active',0,2,2);
             INSERT INTO project_memberships VALUES
               ('admin-membership','bootstrap-local','admin-project','admin-principal','admin','active','bootstrap-owner',2,2),
               ('member-membership','bootstrap-local','member-project','member-principal','member','active','bootstrap-owner',2,2),
               ('viewer-membership','bootstrap-local','viewer-project','viewer-principal','viewer','active','bootstrap-owner',2,2);
             INSERT INTO organizations VALUES('other-org','Other','active',0,2,2);
             INSERT INTO principals VALUES('other-principal','other-org','user','active',NULL,2,2);
             INSERT INTO projects VALUES('other-project','other-org','Other','active',0,2,2);",
        ).await
    }

    #[cfg(test)]
    pub(super) async fn loadout_state_for_test(
        &self,
    ) -> AccessStoreResult<(i64, i64, i64, i64, i64)> {
        self.with_connection(|c| c.query_row("SELECT (SELECT count(*) FROM project_loadouts), (SELECT global_revision FROM access_metadata WHERE singleton=1), (SELECT policy_epoch FROM organizations WHERE organization_id='bootstrap-local'), (SELECT project_policy_epoch FROM projects WHERE project_id='bootstrap-default'), (SELECT count(*) FROM access_audit)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(map_sqlite_error)).await
    }

    #[cfg(test)]
    pub(super) async fn loadout_audit_for_test(
        &self,
    ) -> AccessStoreResult<(String, String, String, i64, String)> {
        self.with_connection(|c| c.query_row("SELECT action,decision,reason_code,policy_epoch,target_fingerprint FROM access_audit WHERE action='access.project_loadout.assign'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(map_sqlite_error)).await
    }

    #[cfg(test)]
    pub(super) async fn install_loadout_audit_failure_for_test(&self) -> AccessStoreResult<()> {
        self.execute_test_statement("CREATE TEMP TRIGGER fail_loadout_audit BEFORE INSERT ON access_audit WHEN NEW.action='access.project_loadout.assign' BEGIN SELECT RAISE(ABORT, 'test audit failure'); END;").await
    }

    #[cfg(test)]
    pub(super) async fn bootstrap_counts_for_test(
        &self,
    ) -> AccessStoreResult<(i64, i64, i64, i64, i64, i64)> {
        self.with_connection(|c| c.query_row("SELECT (SELECT count(*) FROM organizations), (SELECT count(*) FROM principals), (SELECT count(*) FROM principal_links), (SELECT count(*) FROM projects), (SELECT count(*) FROM project_memberships), (SELECT count(*) FROM access_audit)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).map_err(map_sqlite_error)).await
    }

    #[cfg(test)]
    pub(super) async fn bootstrap_metadata_for_test(
        &self,
    ) -> AccessStoreResult<(i64, Option<String>)> {
        self.with_connection(|c| c.query_row("SELECT bootstrap_generation, bootstrap_identity_fingerprint FROM access_metadata WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?))).map_err(map_sqlite_error)).await
    }

    #[cfg(test)]
    async fn pragma_for_test(&self, name: &'static str) -> AccessStoreResult<String> {
        self.with_connection(move |connection| {
            let value = connection
                .query_row(&format!("PRAGMA {name}"), [], |row| row.get::<_, Value>(0))
                .map_err(map_sqlite_error)?;
            Ok(match value {
                Value::Text(value) => value,
                Value::Integer(value) => value.to_string(),
                other => format!("{other:?}"),
            })
        })
        .await
    }

    #[cfg(test)]
    async fn tables_for_test(&self) -> AccessStoreResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT name FROM sqlite_schema
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )
                .map_err(map_sqlite_error)?;
            statement
                .query_map([], |row| row.get(0))
                .map_err(map_sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_sqlite_error)
        })
        .await
    }

    #[cfg(test)]
    pub(super) async fn metadata_for_test(&self) -> AccessStoreResult<(i64, String, i64)> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT schema_version, schema_fingerprint, global_revision
                     FROM access_metadata WHERE singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(map_sqlite_error)
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn execute_test_statement(&self, sql: &'static str) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            connection.execute_batch(sql).map_err(map_sqlite_error)
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn deactivate_principal_for_test(
        &self,
        principal: &'static str,
    ) -> AccessStoreResult<()> {
        let _barrier = self
            .file_stash_principal_gate(principal)
            .write_owned()
            .await;
        self.with_connection(move |connection| {
            connection
                .execute(
                    "UPDATE principals SET status='disabled' WHERE principal_id=?1",
                    [principal],
                )
                .map(|_| ())
                .map_err(map_sqlite_error)
        })
        .await
    }

    #[cfg(test)]
    async fn seed_tenant_test_rows(&self) -> AccessStoreResult<()> {
        self.execute_test_statement(
            "INSERT INTO organizations VALUES
               ('org_a', 'A', 'active', 0, 1, 1),
               ('org_b', 'B', 'active', 0, 1, 1);
             INSERT INTO principals VALUES
               ('principal_a', 'org_a', 'user', 'active', NULL, 1, 1),
               ('principal_b', 'org_b', 'user', 'active', NULL, 1, 1);
             INSERT INTO projects VALUES
               ('project_a', 'org_a', 'A', 'active', 0, 1, 1),
               ('project_b', 'org_b', 'B', 'active', 0, 1, 1);",
        )
        .await
    }
}

fn open_connection(path: &Path) -> AccessStoreResult<Connection> {
    if !path.is_absolute() || path.file_name().is_none_or(|name| name != "access.db") {
        return Err(AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        });
    }
    validated_access_path(path).map_err(|_| AccessStoreError::InsecurePath {
        path: path.to_path_buf(),
    })?;
    let parent = path
        .parent()
        .ok_or_else(|| AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        })?;
    prepare_parent(parent)?;
    validate_existing_store_files(path)?;

    let existed = path.exists();
    if !existed {
        create_restricted_database(path)?;
    }
    validate_store_file(path)?;

    let mut connection = configure_connection(open_nofollow(path)?)?;
    validate_store_file(path)?;
    validate_sidecars(path)?;
    super::migrations::migrate(&mut connection)?;
    backfill_owner_display_labels(&mut connection)?;
    validate_store_file(path)?;
    validate_sidecars(path)?;
    let validation = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    super::integrity::validate(&validation)?;
    validation.commit().map_err(map_sqlite_error)?;
    Ok(connection)
}

fn open_existing_current_connection(path: &Path) -> AccessStoreResult<Connection> {
    validate_path_shape(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        })?;
    validate_secure_parent(parent)?;
    validate_existing_store_files(path)?;
    reject_rollback_journal(path)?;
    validate_store_file(path)?;

    let mut connection = open_nofollow(path)?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(map_sqlite_error)?;
    let journal_mode = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
        .map_err(map_sqlite_error)?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(AccessStoreError::IntegrityViolation {
            check: "journal_mode",
        });
    }
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(map_sqlite_error)?;
    let synchronous = connection
        .query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?;
    if synchronous != 2 {
        return Err(AccessStoreError::IntegrityViolation {
            check: "synchronous",
        });
    }
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(map_sqlite_error)?;
    let foreign_keys = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?;
    if foreign_keys != 1 {
        return Err(AccessStoreError::IntegrityViolation {
            check: "foreign_keys",
        });
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let version = transaction
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?;
    if version > super::migrations::SCHEMA_VERSION {
        return Err(AccessStoreError::UnsupportedSchema {
            found: version,
            supported: super::migrations::SCHEMA_VERSION,
        });
    }
    if version != super::migrations::SCHEMA_VERSION {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_version",
        });
    }
    validate_store_file(path)?;
    validate_sidecars(path)?;
    reject_rollback_journal(path)?;
    super::integrity::validate(&transaction)?;
    let bootstrap_generation = transaction
        .query_row(
            "SELECT bootstrap_generation FROM access_metadata WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_sqlite_error)?;
    if bootstrap_generation != 1 {
        return Err(AccessStoreError::IntegrityViolation {
            check: "bootstrap_required",
        });
    }
    transaction.commit().map_err(map_sqlite_error)?;
    validate_store_file(path)?;
    validate_sidecars(path)?;
    reject_rollback_journal(path)?;
    Ok(connection)
}

fn backfill_owner_display_labels(connection: &mut Connection) -> AccessStoreResult<()> {
    connection.execute(
        "UPDATE principals AS p SET display_name=(SELECT substr(o.name,1,122)||' owner' FROM organizations o WHERE o.organization_id=p.organization_id AND o.status='active'),updated_at=unixepoch() WHERE p.kind='user' AND p.status='active' AND (p.display_name IS NULL OR trim(p.display_name)='') AND EXISTS(SELECT 1 FROM project_memberships m JOIN projects project ON project.organization_id=m.organization_id AND project.project_id=m.project_id WHERE m.organization_id=p.organization_id AND m.principal_id=p.principal_id AND m.role='owner' AND m.status='active' AND project.status='active') AND EXISTS(SELECT 1 FROM organizations o WHERE o.organization_id=p.organization_id AND o.status='active')",
        [],
    ).map_err(map_sqlite_error)?;
    Ok(())
}

fn reject_rollback_journal(path: &Path) -> AccessStoreResult<()> {
    let journal = sidecar_path(path, "-journal");
    match std::fs::symlink_metadata(journal) {
        Ok(_) => Err(AccessStoreError::Locked),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AccessStoreError::Unavailable(error.to_string())),
    }
}

fn validate_path_shape(path: &Path) -> AccessStoreResult<()> {
    if !path.is_absolute() || path.file_name().is_none_or(|name| name != "access.db") {
        return Err(AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        });
    }
    validated_access_path(path).map_err(|_| AccessStoreError::InsecurePath {
        path: path.to_path_buf(),
    })?;
    Ok(())
}

pub(super) fn validated_access_path(path: &Path) -> Result<PathBuf, ()> {
    #[cfg(target_os = "macos")]
    let checked_path = {
        let system_var = Path::new("/var");
        if let Ok(relative) = path.strip_prefix(system_var) {
            let trusted_target = Path::new("/private/var");
            let resolved = std::fs::canonicalize(system_var).map_err(|_| ())?;
            if resolved != trusted_target {
                return Err(());
            }
            trusted_target.join(relative)
        } else {
            path.to_path_buf()
        }
    };
    #[cfg(not(target_os = "macos"))]
    let checked_path = path.to_path_buf();

    labby_runtime::path_safety::reject_existing_symlinks_in_path(&checked_path).map_err(|_| ())?;
    Ok(checked_path)
}

fn open_nofollow(path: &Path) -> AccessStoreResult<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(map_sqlite_error)
}

fn validate_existing_store_files(path: &Path) -> AccessStoreResult<()> {
    if path.exists() {
        validate_store_file(path)?;
    }
    validate_sidecars(path)
}

fn validate_sidecars(path: &Path) -> AccessStoreResult<()> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = sidecar_path(path, suffix);
        match std::fs::symlink_metadata(&sidecar) {
            Ok(_) => validate_store_file(&sidecar)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AccessStoreError::Unavailable(error.to_string())),
        }
    }
    Ok(())
}

pub(super) fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn prepare_parent(path: &Path) -> AccessStoreResult<()> {
    if path.exists() {
        return validate_secure_parent(path);
    }
    let ancestor = path
        .parent()
        .ok_or_else(|| AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        })?;
    if !ancestor.exists() {
        return Err(AccessStoreError::MissingParent {
            path: ancestor.to_path_buf(),
        });
    }
    validated_access_path(ancestor).map_err(|_| AccessStoreError::InsecurePath {
        path: ancestor.to_path_buf(),
    })?;
    create_restricted_directory(path)?;
    validate_secure_parent(path)
}

#[cfg(unix)]
fn create_restricted_directory(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(path)
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

#[cfg(not(unix))]
fn create_restricted_directory(path: &Path) -> AccessStoreResult<()> {
    std::fs::create_dir(path).map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

#[cfg(unix)]
fn create_restricted_database(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

#[cfg(not(unix))]
fn create_restricted_database(path: &Path) -> AccessStoreResult<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

fn configure_connection(connection: Connection) -> AccessStoreResult<Connection> {
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(map_sqlite_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(map_sqlite_error)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(map_sqlite_error)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(map_sqlite_error)?;
    let foreign_keys = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?;
    if foreign_keys != 1 {
        return Err(AccessStoreError::IntegrityViolation {
            check: "foreign_keys",
        });
    }
    Ok(connection)
}

#[cfg(unix)]
fn ensure_restrictive_permissions(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::symlink_metadata(path)
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        return Err(AccessStoreError::InsecurePermissions {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(unix)]
fn validate_secure_parent(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?;
    let mode = metadata.permissions().mode() & 0o777;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || mode & 0o077 != 0
    {
        return Err(AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_secure_parent(_path: &Path) -> AccessStoreResult<()> {
    Ok(())
}

#[cfg(unix)]
fn validate_store_file(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.nlink() != 1
    {
        return Err(AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        });
    }
    ensure_restrictive_permissions(path)
}

#[cfg(not(unix))]
fn validate_store_file(path: &Path) -> AccessStoreResult<()> {
    ensure_restrictive_permissions(path)
}

#[cfg(not(unix))]
fn ensure_restrictive_permissions(_path: &Path) -> AccessStoreResult<()> {
    Ok(())
}

#[cfg(all(unix, test))]
fn restrict_permissions(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

#[cfg(all(not(unix), test))]
fn restrict_permissions(_path: &Path) -> AccessStoreResult<()> {
    Ok(())
}

pub(super) fn map_sqlite_error(error: rusqlite::Error) -> AccessStoreError {
    let Some(failure) = error.sqlite_error() else {
        return AccessStoreError::Unavailable(error.to_string());
    };
    if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY {
        return AccessStoreError::ForeignKeyViolation;
    }
    match failure.code {
        ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => AccessStoreError::Locked,
        ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => AccessStoreError::Corrupt,
        ErrorCode::DiskFull => AccessStoreError::DiskFull,
        ErrorCode::ReadOnly => AccessStoreError::ReadOnly,
        _ => AccessStoreError::Unavailable(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secure_test_path(directory: &tempfile::TempDir) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        directory.path().join("access.db")
    }

    #[tokio::test]
    async fn fresh_store_has_exact_current_schema_and_security_pragmas() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path).await.unwrap();

        assert_eq!(
            store.pragma_for_test("user_version").await.unwrap(),
            super::super::migrations::SCHEMA_VERSION.to_string()
        );
        assert_eq!(store.pragma_for_test("journal_mode").await.unwrap(), "wal");
        assert_eq!(store.pragma_for_test("synchronous").await.unwrap(), "2");
        assert_eq!(store.pragma_for_test("foreign_keys").await.unwrap(), "1");
        assert_eq!(store.pragma_for_test("busy_timeout").await.unwrap(), "5000");
        assert_eq!(
            store.tables_for_test().await.unwrap(),
            vec![
                "access_admission_buckets",
                "access_audit",
                "access_installations",
                "access_metadata",
                "access_security_events",
                "access_tombstones",
                "bootstrap_proofs",
                "credential_idempotency",
                "organizations",
                "principal_links",
                "principals",
                "project_credentials",
                "project_loadouts",
                "project_memberships",
                "project_policy_publications",
                "projects",
            ]
        );
        assert_eq!(
            store.metadata_for_test().await.unwrap(),
            (
                super::super::migrations::SCHEMA_VERSION,
                super::super::migrations::SCHEMA_FINGERPRINT.to_string(),
                0,
            )
        );
    }

    #[tokio::test]
    async fn canonical_reopen_preserves_data_and_global_revision() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .execute_test_statement(
                "UPDATE access_metadata SET global_revision = 7 WHERE singleton = 1;",
            )
            .await
            .unwrap();
        drop(store);

        let reopened = AccessStore::open(path).await.unwrap();
        assert_eq!(reopened.metadata_for_test().await.unwrap().2, 7);
    }

    #[tokio::test]
    async fn newer_schema_fails_closed() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let connection = Connection::open(&path).unwrap();
        let unsupported = super::super::migrations::SCHEMA_VERSION + 1;
        connection
            .pragma_update(None, "user_version", unsupported)
            .unwrap();
        drop(connection);
        restrict_permissions(&path).unwrap();
        assert!(matches!(
            AccessStore::open(path.clone()).await,
            Err(AccessStoreError::UnsupportedSchema {
                found,
                supported
            }) if found == unsupported && supported == super::super::migrations::SCHEMA_VERSION
        ));
    }

    #[tokio::test]
    async fn stamped_v1_without_canonical_schema_identity_fails_closed() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(
                None,
                "user_version",
                super::super::migrations::V1_SCHEMA_VERSION,
            )
            .unwrap();
        drop(connection);
        restrict_permissions(&path).unwrap();

        assert!(matches!(
            AccessStore::open(path.clone()).await,
            Err(AccessStoreError::IntegrityViolation { .. })
        ));
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            super::super::migrations::V1_SCHEMA_VERSION
        );
    }

    #[tokio::test]
    async fn canonical_v1_migrates_to_current_and_preserves_revision() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(super::super::migrations::V1_METADATA_SCHEMA)
            .unwrap();
        connection
            .execute_batch(super::super::migrations::DOMAIN_SCHEMA)
            .unwrap();
        connection.execute("INSERT INTO access_metadata(singleton,schema_version,schema_fingerprint,global_revision,updated_at) VALUES(1,?1,?2,9,123)", rusqlite::params![super::super::migrations::V1_SCHEMA_VERSION, super::super::migrations::V1_SCHEMA_FINGERPRINT]).unwrap();
        connection
            .pragma_update(
                None,
                "application_id",
                super::super::migrations::APPLICATION_ID,
            )
            .unwrap();
        connection
            .pragma_update(
                None,
                "user_version",
                super::super::migrations::V1_SCHEMA_VERSION,
            )
            .unwrap();
        drop(connection);
        restrict_permissions(&path).unwrap();

        let store = AccessStore::open(path).await.unwrap();
        assert_eq!(
            store.pragma_for_test("user_version").await.unwrap(),
            super::super::migrations::SCHEMA_VERSION.to_string()
        );
        assert_eq!(store.metadata_for_test().await.unwrap().2, 9);
        assert_eq!(
            store.bootstrap_metadata_for_test().await.unwrap(),
            (0, None)
        );
        let input = BootstrapOwnerInput::new(
            labby_auth::VerifiedIdentity::local_credential(
                labby_auth::Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .unwrap(),
            "Local",
            "Default",
        )
        .unwrap();
        assert!(matches!(
            store.bootstrap_owner(input).await,
            Err(AccessStoreError::BootstrapConflict)
        ));
    }

    #[tokio::test]
    async fn populated_canonical_v1_migrates_healthy_but_is_not_bootstrap_pristine() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(super::super::migrations::V1_METADATA_SCHEMA)
            .unwrap();
        connection
            .execute_batch(super::super::migrations::DOMAIN_SCHEMA)
            .unwrap();
        connection.execute("INSERT INTO access_metadata(singleton,schema_version,schema_fingerprint,global_revision,updated_at) VALUES(1,?1,?2,4,123)", rusqlite::params![super::super::migrations::V1_SCHEMA_VERSION, super::super::migrations::V1_SCHEMA_FINGERPRINT]).unwrap();
        connection
            .execute(
                "INSERT INTO organizations VALUES('legacy-org','Legacy','active',0,1,1)",
                [],
            )
            .unwrap();
        connection
            .pragma_update(
                None,
                "application_id",
                super::super::migrations::APPLICATION_ID,
            )
            .unwrap();
        connection
            .pragma_update(
                None,
                "user_version",
                super::super::migrations::V1_SCHEMA_VERSION,
            )
            .unwrap();
        drop(connection);
        restrict_permissions(&path).unwrap();
        let store = AccessStore::open(path).await.unwrap();
        assert_eq!(
            store.bootstrap_metadata_for_test().await.unwrap(),
            (0, None)
        );
        let input = BootstrapOwnerInput::new(
            labby_auth::VerifiedIdentity::local_credential(
                labby_auth::Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .unwrap(),
            "Local",
            "Default",
        )
        .unwrap();
        assert!(matches!(
            store.bootstrap_owner(input).await,
            Err(AccessStoreError::BootstrapConflict)
        ));
    }

    #[tokio::test]
    async fn canonical_names_and_metadata_do_not_hide_altered_schema_definition() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path.clone()).await.unwrap();
        drop(store);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP INDEX principal_links_external_unique;
                 CREATE INDEX principal_links_external_unique
                   ON principal_links(issuer, subject) WHERE link_kind = 'external';",
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            AccessStore::open(path).await,
            Err(AccessStoreError::IntegrityViolation {
                check: "schema_manifest"
            })
        ));
    }

    #[tokio::test]
    async fn composite_foreign_keys_reject_cross_tenant_edges() {
        let directory = super::super::test_support::secure_tempdir();
        let store = AccessStore::open(secure_test_path(&directory))
            .await
            .unwrap();
        store.seed_tenant_test_rows().await.unwrap();
        let membership = store
            .execute_test_statement(
                "INSERT INTO project_memberships
             (membership_id, organization_id, project_id, principal_id, role, status,
              created_by, created_at, updated_at)
             VALUES ('mem_bad', 'org_b', 'project_a', 'principal_b', 'member', 'active',
                     'principal_b', 1, 1)",
            )
            .await;
        assert!(matches!(
            membership,
            Err(AccessStoreError::ForeignKeyViolation)
        ));
        let loadout = store
            .execute_test_statement(
                "INSERT INTO project_loadouts
             (organization_id, project_id, loadout_name, created_by, created_at, updated_at)
             VALUES ('org_b', 'project_a', 'default', 'principal_b', 1, 1)",
            )
            .await;
        assert!(matches!(
            loadout,
            Err(AccessStoreError::ForeignKeyViolation)
        ));

        let missing_same_tenant_parent = store
            .execute_test_statement(
                "INSERT INTO project_loadouts
                 (organization_id, project_id, loadout_name, created_by, created_at, updated_at)
                 VALUES ('org_a', 'missing', 'default', 'principal_a', 1, 1)",
            )
            .await;
        assert!(matches!(
            missing_same_tenant_parent,
            Err(AccessStoreError::ForeignKeyViolation)
        ));
    }

    #[tokio::test]
    async fn principal_link_shape_and_uniqueness_are_database_invariants() {
        let directory = super::super::test_support::secure_tempdir();
        let store = AccessStore::open(secure_test_path(&directory))
            .await
            .unwrap();
        store.seed_tenant_test_rows().await.unwrap();
        store
            .execute_test_statement(
                "INSERT INTO principal_links
             (link_id, principal_id, link_kind, issuer, subject, credential_id, status,
              verification_generation, link_generation, created_at, updated_at)
             VALUES ('link_a', 'principal_a', 'external', 'https://idp.example.com', 'alice',
                     NULL, 'active', 1, 1, 1, 1)",
            )
            .await
            .unwrap();
        assert!(
            store
                .execute_test_statement(
                    "INSERT INTO principal_links
             (link_id, principal_id, link_kind, issuer, subject, credential_id, status,
              verification_generation, link_generation, created_at, updated_at)
             VALUES ('link_dup', 'principal_b', 'external', 'https://idp.example.com', 'alice',
                     NULL, 'active', 1, 1, 1, 1)"
                )
                .await
                .is_err()
        );
        assert!(
            store
                .execute_test_statement(
                    "INSERT INTO principal_links
             (link_id, principal_id, link_kind, issuer, subject, credential_id, status,
              verification_generation, link_generation, created_at, updated_at)
             VALUES ('link_bad', 'principal_a', 'external', NULL, 'alice', 'credential',
                     'active', 1, 1, 1, 1)"
                )
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_weak_permissions_symlinks_hardlinks_and_corrupt_files() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let weak_directory = super::super::test_support::secure_tempdir();
        let weak_path = secure_test_path(&weak_directory);
        std::fs::write(&weak_path, []).unwrap();
        std::fs::set_permissions(&weak_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            AccessStore::open(weak_path).await,
            Err(AccessStoreError::InsecurePermissions { .. })
        ));

        let symlink_directory = super::super::test_support::secure_tempdir();
        let symlink_path = secure_test_path(&symlink_directory);
        let target = symlink_directory.path().join("target.db");
        std::fs::write(&target, []).unwrap();
        symlink(&target, &symlink_path).unwrap();
        assert!(matches!(
            AccessStore::open(symlink_path).await,
            Err(AccessStoreError::InsecurePath { .. })
        ));

        let hardlink_directory = super::super::test_support::secure_tempdir();
        let hardlink_path = secure_test_path(&hardlink_directory);
        let hardlink_target = hardlink_directory.path().join("other.db");
        let original = b"must remain byte-for-byte unchanged";
        std::fs::write(&hardlink_target, original).unwrap();
        std::fs::set_permissions(&hardlink_target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::hard_link(&hardlink_target, &hardlink_path).unwrap();
        assert!(matches!(
            AccessStore::open(hardlink_path).await,
            Err(AccessStoreError::InsecurePath { .. })
        ));
        assert_eq!(std::fs::read(&hardlink_target).unwrap(), original);

        let corrupt_directory = super::super::test_support::secure_tempdir();
        let corrupt_path = secure_test_path(&corrupt_directory);
        std::fs::write(&corrupt_path, b"not a sqlite database").unwrap();
        std::fs::set_permissions(&corrupt_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            AccessStore::open(corrupt_path).await,
            Err(AccessStoreError::Corrupt)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn creates_new_leaf_and_store_with_owner_only_permissions_without_fixing_weak_dirs() {
        use std::os::unix::fs::PermissionsExt as _;

        let base = super::super::test_support::secure_tempdir();
        std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let leaf = base.path().join("access-state");
        let path = leaf.join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        assert_eq!(
            std::fs::metadata(&leaf).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(store);

        let weak = base.path().join("weak");
        std::fs::create_dir(&weak).unwrap();
        std::fs::set_permissions(&weak, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            AccessStore::open(weak.join("access.db")).await,
            Err(AccessStoreError::InsecurePath { .. })
        ));
        assert_eq!(
            std::fs::metadata(&weak).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_world_writable_non_sticky_parent_without_creating_store() {
        use std::os::unix::fs::PermissionsExt as _;

        let base = super::super::test_support::secure_tempdir();
        let weak = base.path().join("world-writable");
        std::fs::create_dir(&weak).unwrap();
        std::fs::set_permissions(&weak, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = weak.join("access.db");

        assert!(matches!(
            AccessStore::open(path.clone()).await,
            Err(AccessStoreError::InsecurePath { .. })
        ));
        assert!(
            !path.exists(),
            "validation must happen before store creation"
        );
        assert_eq!(
            std::fs::metadata(&weak).unwrap().permissions().mode() & 0o1777,
            0o0777,
            "the rejected directory must not be silently repaired or made sticky"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn accepts_owner_only_store_below_trusted_macos_var_alias() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("access.db");
        AccessStore::open(path).await.unwrap();
    }

    #[tokio::test]
    async fn file_stash_identity_is_resolved_from_principal_links_across_transports() {
        let directory = super::super::test_support::secure_tempdir();
        let store = AccessStore::open(secure_test_path(&directory))
            .await
            .unwrap();
        store.execute_test_statement("INSERT INTO organizations VALUES('org','Org','active',0,1,1); INSERT INTO principals VALUES('external-principal','org','user','active',NULL,1,1),('local-principal','org','service_account','active',NULL,1,1); INSERT INTO principal_links VALUES('external-link','external-principal','external','https://accounts.google.com','stable-subject',NULL,'active',1,1,1,1),('local-link','local-principal','local_credential',NULL,NULL,'static-credential','active',1,1,1,1);").await.unwrap();
        let browser = labby_auth::VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            "https://accounts.google.com",
            "stable-subject",
        )
        .unwrap();
        let oauth = labby_auth::VerifiedIdentity::external(
            labby_auth::Authenticator::OauthBearer,
            "https://accounts.google.com",
            "stable-subject",
        )
        .unwrap();
        let local = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-credential",
        )
        .unwrap();
        assert_eq!(
            store
                .resolve_file_stash_principal(browser)
                .await
                .unwrap()
                .as_str(),
            "external-principal"
        );
        assert_eq!(
            store
                .resolve_file_stash_principal(oauth)
                .await
                .unwrap()
                .as_str(),
            "external-principal"
        );
        assert_eq!(
            store
                .resolve_file_stash_principal(local)
                .await
                .unwrap()
                .as_str(),
            "local-principal"
        );
        let missing = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::UnixPeer,
            "missing",
        )
        .unwrap();
        assert!(matches!(
            store.resolve_file_stash_principal(missing).await,
            Err(AccessStoreError::IdentityUnavailable)
        ));
    }

    #[tokio::test]
    async fn recipient_search_deadline_interrupts_sqlite_and_releases_admission() {
        let directory = super::super::test_support::secure_tempdir();
        let store = AccessStore::open(secure_test_path(&directory))
            .await
            .unwrap();
        store.execute_test_statement("INSERT INTO organizations VALUES('org','Org','active',0,1,1); INSERT INTO principals VALUES('owner','org','user','active','Owner',1,1),('recipient','org','user','active','Recipient',1,1);").await.unwrap();
        assert!(matches!(
            store
                .search_file_stash_recipients(
                    super::super::AccessPrincipalId::for_test("owner"),
                    "Rec".into(),
                    20,
                    Duration::ZERO,
                )
                .await,
            Err(AccessStoreError::Unavailable(message)) if message.contains("deadline")
        ));
        let recipients = store
            .search_file_stash_recipients(
                super::super::AccessPrincipalId::for_test("owner"),
                "Rec".into(),
                20,
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(recipients.len(), 1);
        assert_eq!(recipients[0].principal_id, "recipient");
    }

    #[tokio::test]
    async fn recipient_search_deadline_includes_admission_wait() {
        let directory = super::super::test_support::secure_tempdir();
        let store = AccessStore::open(secure_test_path(&directory))
            .await
            .unwrap();
        let held = Arc::clone(&store.connection_admission)
            .acquire_owned()
            .await
            .unwrap();
        let started = tokio::time::Instant::now();
        let result = store
            .search_file_stash_recipients(
                super::super::AccessPrincipalId::for_test("owner"),
                "Rec".into(),
                20,
                Duration::from_millis(10),
            )
            .await;
        assert!(matches!(
            result,
            Err(AccessStoreError::Unavailable(message)) if message.contains("deadline")
        ));
        assert!(started.elapsed() < Duration::from_millis(100));
        drop(held);
    }

    #[tokio::test]
    async fn file_stash_active_principal_lease_linearizes_deactivation() {
        let directory = super::super::test_support::secure_tempdir();
        let store = AccessStore::open(secure_test_path(&directory))
            .await
            .unwrap();
        store.execute_test_statement("INSERT INTO organizations VALUES('org','Org','active',0,1,1); INSERT INTO principals VALUES('recipient','org','user','active',NULL,1,1); INSERT INTO principal_links VALUES('link','recipient','local_credential',NULL,NULL,'credential','active',1,1,1,1);").await.unwrap();
        let identity = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "credential",
        )
        .unwrap();
        let lease = store
            .resolve_and_lease_file_stash_principal(identity)
            .await
            .unwrap()
            .1;
        tokio::time::timeout(Duration::from_millis(25), store.metadata_for_test())
            .await
            .expect("a Stash lease must not monopolize AccessStore admission")
            .unwrap();
        let mutation_store = store.clone();
        let mut mutation = tokio::spawn(async move {
            mutation_store
                .deactivate_principal_for_test("recipient")
                .await
                .unwrap();
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut mutation)
                .await
                .is_err()
        );
        drop(lease);
        mutation.await.unwrap();
        assert!(matches!(
            store
                .lease_active_file_stash_principal(super::super::AccessPrincipalId::for_test(
                    "recipient"
                ))
                .await,
            Err(AccessStoreError::IdentityUnavailable)
        ));
    }

    #[tokio::test]
    async fn open_existing_current_does_not_rewrite_principal_labels() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path.clone()).await.unwrap();
        let input = BootstrapOwnerInput::new(
            labby_auth::VerifiedIdentity::local_credential(
                labby_auth::Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .unwrap(),
            "Acme",
            "Default",
        )
        .unwrap();
        store.bootstrap_owner(input).await.unwrap();
        store
            .execute_test_statement(
                "UPDATE principals SET display_name=NULL WHERE principal_id='bootstrap-owner'",
            )
            .await
            .unwrap();
        drop(store);

        let reopened = AccessStore::open_existing_current(path).await.unwrap();
        let label = reopened
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT display_name FROM principals WHERE principal_id='bootstrap-owner'",
                        [],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(label, None);
    }

    #[tokio::test]
    async fn startup_backfill_labels_only_active_human_project_owners() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path).await.unwrap();
        store.execute_test_statement("INSERT INTO organizations VALUES('org','Acme','active',0,1,1); INSERT INTO principals VALUES('owner','org','user','active',NULL,1,1),('service','org','service_account','active',NULL,1,1),('disabled','org','user','disabled',NULL,1,1),('viewer','org','user','active',NULL,1,1),('inactive-project-owner','org','user','active',NULL,1,1); INSERT INTO projects VALUES('project','org','Project','active',0,1,1),('inactive-project','org','Inactive','disabled',0,1,1); INSERT INTO project_memberships VALUES('m1','org','project','owner','owner','active','owner',1,1),('m2','org','project','service','owner','active','owner',1,1),('m3','org','project','disabled','owner','active','owner',1,1),('m4','org','project','viewer','viewer','active','owner',1,1),('m5','org','inactive-project','inactive-project-owner','owner','active','owner',1,1);").await.unwrap();
        store
            .with_connection(|connection| backfill_owner_display_labels(connection))
            .await
            .unwrap();
        let labels = store.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT principal_id,display_name FROM principals WHERE organization_id='org' ORDER BY principal_id").map_err(map_sqlite_error)?;
            let rows = statement.query_map([], |row| Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?))).map_err(map_sqlite_error)?;
            rows.collect::<Result<Vec<_>,_>>().map_err(map_sqlite_error)
        }).await.unwrap();
        assert_eq!(
            labels,
            vec![
                ("disabled".into(), None),
                ("inactive-project-owner".into(), None),
                ("owner".into(), Some("Acme owner".into())),
                ("service".into(), None),
                ("viewer".into(), None)
            ]
        );
    }
}
