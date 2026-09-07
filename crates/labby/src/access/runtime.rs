use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

const BOOTSTRAP_WRITER_DEADLINE: std::time::Duration = std::time::Duration::from_millis(100);

use super::bootstrap::{BootstrapOutcome, BootstrapOwnerInput};
use super::credential_verifier::{AccessCredentialAdapter, CredentialReadPool, LiveAuthority};
use super::error::AccessStoreError;
use super::health::{AccessHealthStatus, inspect_health};
use super::store::AccessStore;
use super::{CredentialSnapshot, IssueCredentialInput, MutationOutcome};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccessSetupReason {
    Missing,
    Uninitialized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccessBlockedReason {
    Insecure,
    Corrupt,
    NewerSchema,
    Locked,
    ReadOnly,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccessRuntimeStatus {
    SetupRequired(AccessSetupReason),
    Ready,
    Blocked(AccessBlockedReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum AccessRuntimeError {
    #[error("access setup is required")]
    SetupRequired(AccessSetupReason),
    #[error("access runtime is blocked")]
    Blocked(AccessBlockedReason),
    #[error("access owner bootstrap conflicts with existing state")]
    BootstrapConflict,
    #[error("access owner bootstrap input is invalid")]
    InvalidBootstrapInput,
    #[error("access runtime lifecycle is unavailable")]
    LifecycleUnavailable,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FileStashPrincipalResolutionError {
    #[error("the verified identity has no active durable principal link")]
    IdentityUnavailable,
    #[error(transparent)]
    Runtime(#[from] AccessRuntimeError),
    #[error("the access store could not resolve a principal")]
    StoreUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum CredentialLifecycleError {
    #[error("credential request is invalid")]
    Invalid,
    #[error("credential operation is not authorized")]
    NotAuthorized,
    #[error("credential service is unavailable")]
    Unavailable,
}

enum RuntimeState {
    SetupRequired(AccessSetupReason),
    Prepared,
    Ready {
        store: AccessStore,
        credential_reads: CredentialReadPool,
    },
    Blocked(AccessBlockedReason),
}

/// Process-scoped owner of the access-store lifecycle.
///
/// Construction is observational: it never creates or migrates the store. Only the explicit
/// bootstrap operation is allowed to initialize persistence and promote the runtime to `Ready`.
#[derive(Clone)]
pub(crate) struct AccessRuntime {
    path: Arc<PathBuf>,
    state: Arc<Mutex<RuntimeState>>,
    bootstrap_writer: Arc<Semaphore>,
}

impl AccessRuntime {
    async fn security_store(&self) -> Result<AccessStore, AccessRuntimeError> {
        match &*self.state.lock().await {
            RuntimeState::Ready { store, .. } => Ok(store.clone()),
            RuntimeState::Prepared => AccessStore::open((*self.path).clone())
                .await
                .map_err(|_| AccessRuntimeError::LifecycleUnavailable),
            RuntimeState::SetupRequired(reason) => Err(AccessRuntimeError::SetupRequired(*reason)),
            RuntimeState::Blocked(reason) => Err(AccessRuntimeError::Blocked(*reason)),
        }
    }

    pub(crate) async fn admit_security_operation(
        &self,
        class: String,
        bucket: [u8; 32],
        now: i64,
        window_seconds: i64,
        limit: i64,
    ) -> Result<bool, AccessRuntimeError> {
        let _writer = self.acquire_bootstrap_writer().await?;
        self.security_store()
            .await?
            .admit_security_operation(class, bucket, now, window_seconds, limit)
            .await
            .map_err(|_| AccessRuntimeError::LifecycleUnavailable)
    }

    pub(crate) async fn record_security_event(
        &self,
        event_kind: String,
        decision: String,
        reason: String,
        target: [u8; 32],
        peer: Option<[u8; 32]>,
        now: i64,
    ) -> Result<(), AccessRuntimeError> {
        let _writer = self.acquire_bootstrap_writer().await?;
        self.security_store()
            .await?
            .record_security_event(event_kind, decision, reason, target, peer, now)
            .await
            .map_err(|_| AccessRuntimeError::LifecycleUnavailable)
    }

    pub(crate) async fn record_bootstrap_semantic_failure(
        &self,
        proof_id: String,
        proof_digest: [u8; 32],
        now: i64,
    ) -> Result<i64, AccessRuntimeError> {
        let _writer = self.acquire_bootstrap_writer().await?;
        self.security_store()
            .await?
            .record_bootstrap_semantic_failure(proof_id, proof_digest, now)
            .await
            .map_err(|_| AccessRuntimeError::LifecycleUnavailable)
    }
    /// Constructs a conservative non-I/O runtime for state containers that are not process
    /// lifecycle owners. Production serve wiring must replace this with `initialize`.
    pub(crate) fn blocked_unavailable() -> Self {
        Self {
            // A blocked non-owner can never open or bootstrap this path. Keeping
            // the sentinel internal avoids platform-specific fake absolute paths.
            path: Arc::new(PathBuf::new()),
            state: Arc::new(Mutex::new(RuntimeState::Blocked(
                AccessBlockedReason::Unavailable,
            ))),
            bootstrap_writer: Arc::new(Semaphore::new(1)),
        }
    }

    pub(crate) async fn initialize(path: PathBuf) -> Self {
        let mut state = observe_state(&path).await;
        for _ in 0..20 {
            if !matches!(state, RuntimeState::Blocked(AccessBlockedReason::Locked)) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            state = observe_state(&path).await;
        }
        if let RuntimeState::Blocked(reason) = state {
            tracing::warn!(?reason, "access runtime initialization blocked");
            state = RuntimeState::Blocked(reason);
        }
        Self {
            path: Arc::new(path),
            state: Arc::new(Mutex::new(state)),
            bootstrap_writer: Arc::new(Semaphore::new(1)),
        }
    }

    pub(crate) async fn status(&self) -> AccessRuntimeStatus {
        match &*self.state.lock().await {
            RuntimeState::SetupRequired(reason) => AccessRuntimeStatus::SetupRequired(*reason),
            RuntimeState::Prepared => {
                AccessRuntimeStatus::SetupRequired(AccessSetupReason::Uninitialized)
            }
            RuntimeState::Ready { .. } => AccessRuntimeStatus::Ready,
            RuntimeState::Blocked(reason) => AccessRuntimeStatus::Blocked(*reason),
        }
    }

    /// Returns a handle only while the runtime is atomically observed as Ready.
    ///
    /// Once Ready handles can be issued, this lifecycle never transitions back to Blocked or
    /// SetupRequired. Future enforcement must treat a process restart as the boundary for
    /// re-observing persistent store health.
    pub(crate) async fn store(&self) -> Result<AccessStore, AccessRuntimeError> {
        match &*self.state.lock().await {
            RuntimeState::Ready { store, .. } => Ok(store.clone()),
            RuntimeState::SetupRequired(reason) => Err(AccessRuntimeError::SetupRequired(*reason)),
            RuntimeState::Prepared => Err(AccessRuntimeError::SetupRequired(
                AccessSetupReason::Uninitialized,
            )),
            RuntimeState::Blocked(reason) => Err(AccessRuntimeError::Blocked(*reason)),
        }
    }

    pub(crate) async fn resolve_file_stash_principal(
        &self,
        identity: labby_auth::VerifiedIdentity,
    ) -> Result<super::AccessPrincipalId, FileStashPrincipalResolutionError> {
        self.store()
            .await?
            .resolve_file_stash_principal(identity)
            .await
            .map_err(|error| match error {
                AccessStoreError::IdentityUnavailable | AccessStoreError::NotAuthorized => {
                    FileStashPrincipalResolutionError::IdentityUnavailable
                }
                _ => FileStashPrincipalResolutionError::StoreUnavailable,
            })
    }

    pub(crate) async fn resolve_and_lease_file_stash_principal(
        &self,
        identity: labby_auth::VerifiedIdentity,
    ) -> Result<
        (
            super::AccessPrincipalId,
            super::ActiveFileStashPrincipalLease,
        ),
        FileStashPrincipalResolutionError,
    > {
        self.store()
            .await?
            .resolve_and_lease_file_stash_principal(identity)
            .await
            .map_err(|error| match error {
                AccessStoreError::IdentityUnavailable | AccessStoreError::NotAuthorized => {
                    FileStashPrincipalResolutionError::IdentityUnavailable
                }
                _ => FileStashPrincipalResolutionError::StoreUnavailable,
            })
    }

    pub(crate) async fn resolve_and_lease_file_stash_participants(
        &self,
        identity: labby_auth::VerifiedIdentity,
        recipient: String,
    ) -> Result<
        (
            super::AccessPrincipalId,
            super::AccessPrincipalId,
            super::ActiveFileStashPrincipalLease,
        ),
        FileStashPrincipalResolutionError,
    > {
        self.store()
            .await?
            .resolve_and_lease_file_stash_participants(identity, recipient)
            .await
            .map_err(|error| match error {
                AccessStoreError::IdentityUnavailable | AccessStoreError::NotAuthorized => {
                    FileStashPrincipalResolutionError::IdentityUnavailable
                }
                _ => FileStashPrincipalResolutionError::StoreUnavailable,
            })
    }

    pub(crate) async fn lease_active_file_stash_principal(
        &self,
        principal: super::AccessPrincipalId,
    ) -> Result<super::ActiveFileStashPrincipalLease, FileStashPrincipalResolutionError> {
        self.store()
            .await?
            .lease_active_file_stash_principal(principal)
            .await
            .map_err(|error| match error {
                AccessStoreError::IdentityUnavailable | AccessStoreError::NotAuthorized => {
                    FileStashPrincipalResolutionError::IdentityUnavailable
                }
                _ => FileStashPrincipalResolutionError::StoreUnavailable,
            })
    }

    pub(crate) async fn lease_file_stash_participants(
        &self,
        owner: super::AccessPrincipalId,
        recipient: String,
    ) -> Result<
        (
            super::AccessPrincipalId,
            super::ActiveFileStashPrincipalLease,
        ),
        FileStashPrincipalResolutionError,
    > {
        self.store()
            .await?
            .lease_file_stash_participants(owner, recipient)
            .await
            .map_err(|error| match error {
                AccessStoreError::IdentityUnavailable | AccessStoreError::NotAuthorized => {
                    FileStashPrincipalResolutionError::IdentityUnavailable
                }
                _ => FileStashPrincipalResolutionError::StoreUnavailable,
            })
    }

    /// Resolve an opaque transport-supplied recipient ID only after the
    /// AccessStore has proved that it names an active principal. Keeping the
    /// constructor inside this authority boundary prevents adapters from
    /// treating untrusted text as a durable identity.
    pub(crate) async fn resolve_active_file_stash_recipient(
        &self,
        value: String,
    ) -> Result<
        (
            super::AccessPrincipalId,
            super::ActiveFileStashPrincipalLease,
        ),
        FileStashPrincipalResolutionError,
    > {
        let value = value.trim();
        if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
            return Err(FileStashPrincipalResolutionError::IdentityUnavailable);
        }
        let principal = super::AccessPrincipalId(value.to_owned());
        let lease = self
            .lease_active_file_stash_principal(principal.clone())
            .await?;
        Ok((principal, lease))
    }

    pub(super) async fn credential_reads(&self) -> Result<CredentialReadPool, AccessRuntimeError> {
        match &*self.state.lock().await {
            RuntimeState::Ready {
                credential_reads, ..
            } => Ok(credential_reads.clone()),
            RuntimeState::SetupRequired(reason) => Err(AccessRuntimeError::SetupRequired(*reason)),
            RuntimeState::Prepared => Err(AccessRuntimeError::SetupRequired(
                AccessSetupReason::Uninitialized,
            )),
            RuntimeState::Blocked(reason) => Err(AccessRuntimeError::Blocked(*reason)),
        }
    }

    /// Serializes bootstrap orchestration before it enters the access-store
    /// transaction. Callers acquire Gateway publication leases first; the
    /// returned permit must remain alive through the SQLite commit.
    pub(crate) async fn acquire_bootstrap_writer(
        &self,
    ) -> Result<OwnedSemaphorePermit, AccessRuntimeError> {
        tokio::time::timeout(
            BOOTSTRAP_WRITER_DEADLINE,
            Arc::clone(&self.bootstrap_writer).acquire_owned(),
        )
        .await
        .map_err(|_| AccessRuntimeError::LifecycleUnavailable)?
        .map_err(|_| AccessRuntimeError::LifecycleUnavailable)
    }

    /// Consume the sole prepared proof without exposing general store access
    /// before an owner exists, then promote this process to Ready only after
    /// the durable transaction commits and the current store reopens cleanly.
    pub(crate) async fn consume_prepared_bootstrap(
        &self,
        input: super::ConsumeBootstrapInput,
    ) -> Result<MutationOutcome, AccessStoreError> {
        let mut state = self.state.lock().await;
        let store = match &*state {
            RuntimeState::Prepared => {
                // `Prepared` was classified from an exact current-schema
                // observational health check. Use the normal secure opener for
                // the one permitted writer transaction; the non-migrating
                // opener intentionally requires an already-owned store.
                AccessStore::open((*self.path).clone())
                    .await
                    .inspect_err(|_error| {
                        tracing::warn!(phase = "open_prepared", "bootstrap promotion failed");
                    })?
            }
            RuntimeState::Ready { store, .. } => store.clone(),
            RuntimeState::SetupRequired(_) => return Err(AccessStoreError::NotAuthorized),
            RuntimeState::Blocked(_) => return Err(AccessStoreError::Locked),
        };
        let outcome = store
            .consume_bootstrap_proof(input)
            .await
            .inspect_err(|_error| {
                tracing::warn!(phase = "consume_transaction", "bootstrap promotion failed");
            })?;
        // The just-committed WAL may not admit new connections until this
        // writer is dropped. Reuse its already-validated connection for the
        // remainder of this process; restart rebuilds the normal read pool.
        let credential_reads = CredentialReadPool::from_store(store.clone());
        *state = RuntimeState::Ready {
            store,
            credential_reads,
        };
        Ok(outcome)
    }

    pub(crate) async fn reconcile_project_policy(
        &self,
        project_id: String,
        fingerprint: [u8; 32],
    ) -> Result<u64, AccessRuntimeError> {
        // Product authorities call this only after acquiring the Gateway
        // publication lease. Keep the writer alive through the SQLite commit,
        // establishing lease -> runtime writer -> transaction lock order.
        let _writer = self.acquire_bootstrap_writer().await?;
        let store = self.store().await?;
        let now = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| AccessRuntimeError::LifecycleUnavailable)?
                .as_secs(),
        )
        .map_err(|_| AccessRuntimeError::LifecycleUnavailable)?;
        let epoch = store
            .reconcile_project_policy(project_id, fingerprint, now)
            .await
            .map_err(|_| AccessRuntimeError::LifecycleUnavailable)?;
        u64::try_from(epoch).map_err(|_| AccessRuntimeError::LifecycleUnavailable)
    }

    /// Builds the three auth adapters over this process-owned runtime and an
    /// injected live published-policy authority. The adapter is cheap to clone
    /// and intentionally contains no authorization-result cache.
    pub(crate) fn credential_adapter(
        &self,
        live: Arc<dyn LiveAuthority>,
    ) -> Arc<AccessCredentialAdapter> {
        Arc::new(AccessCredentialAdapter::new(self.clone(), live))
    }

    pub(crate) async fn issue_project_credential(
        &self,
        input: IssueCredentialInput,
    ) -> Result<MutationOutcome, CredentialLifecycleError> {
        self.store()
            .await
            .map_err(|_| CredentialLifecycleError::Unavailable)?
            .issue_project_credential(input)
            .await
            .map_err(credential_runtime_error)
    }

    pub(crate) async fn introspect_project_credential(
        &self,
        credential_id: String,
        credential_generation: i64,
        now: i64,
    ) -> Result<Option<CredentialSnapshot>, CredentialLifecycleError> {
        self.store()
            .await
            .map_err(|_| CredentialLifecycleError::Unavailable)?
            .introspect_project_credential(credential_id, credential_generation, now)
            .await
            .map_err(credential_runtime_error)
    }

    pub(crate) async fn revoke_project_credential(
        &self,
        actor_id: String,
        actor_generation: i64,
        target_id: String,
        now: i64,
    ) -> Result<MutationOutcome, CredentialLifecycleError> {
        self.store()
            .await
            .map_err(|_| CredentialLifecycleError::Unavailable)?
            .revoke_project_credential(actor_id, actor_generation, target_id, now)
            .await
            .map_err(credential_runtime_error)
    }

    pub(crate) async fn bootstrap_owner(
        &self,
        input: BootstrapOwnerInput,
    ) -> Result<BootstrapOutcome, AccessRuntimeError> {
        // The owned task is the single-flight lifecycle operation. If the initiating request is
        // cancelled after SQLite starts work, this task still observes completion and installs
        // the authoritative Ready state.
        let runtime = self.clone();
        tokio::spawn(async move { runtime.bootstrap_owner_owned(input).await })
            .await
            .map_err(|_| AccessRuntimeError::LifecycleUnavailable)?
    }

    async fn bootstrap_owner_owned(
        &self,
        input: BootstrapOwnerInput,
    ) -> Result<BootstrapOutcome, AccessRuntimeError> {
        let mut state = self.state.lock().await;
        let store = match &*state {
            RuntimeState::Ready { store, .. } => {
                return store
                    .bootstrap_owner(input)
                    .await
                    .map_err(|error| runtime_error(&state, Some(&error)));
            }
            RuntimeState::SetupRequired(_) => match AccessStore::open((*self.path).clone()).await {
                Ok(store) => store,
                Err(error) => {
                    *state = observe_state(&self.path).await;
                    return Err(runtime_error(&state, Some(&error)));
                }
            },
            RuntimeState::Prepared => {
                return Err(AccessRuntimeError::BootstrapConflict);
            }
            RuntimeState::Blocked(reason) => return Err(AccessRuntimeError::Blocked(*reason)),
        };

        let outcome = match store.bootstrap_owner(input).await {
            Ok(outcome) => outcome,
            Err(error) => {
                *state = observe_state(&self.path).await;
                return Err(runtime_error(&state, Some(&error)));
            }
        };
        // Reopen through the non-migrating seam so promotion proves the same invariant required
        // during normal process initialization.
        let ready = match AccessStore::open_existing_current((*self.path).clone()).await {
            Ok(store) => store,
            Err(error) => {
                *state = observe_state(&self.path).await;
                return Err(runtime_error(&state, Some(&error)));
            }
        };
        let credential_reads = CredentialReadPool::open(&self.path)
            .await
            .map_err(|_| AccessRuntimeError::LifecycleUnavailable)?;
        *state = RuntimeState::Ready {
            store: ready,
            credential_reads,
        };
        Ok(outcome)
    }
}

async fn observe_state(path: &Path) -> RuntimeState {
    let health_path = path.to_path_buf();
    let health = match tokio::task::spawn_blocking(move || inspect_health(&health_path)).await {
        Ok(health) => health,
        Err(_) => return RuntimeState::Blocked(AccessBlockedReason::Unavailable),
    };
    match health.status {
        AccessHealthStatus::Missing => RuntimeState::SetupRequired(AccessSetupReason::Missing),
        AccessHealthStatus::Uninitialized => {
            RuntimeState::SetupRequired(AccessSetupReason::Uninitialized)
        }
        AccessHealthStatus::Prepared => RuntimeState::Prepared,
        AccessHealthStatus::Ready => {
            match AccessStore::open_existing_current(path.to_path_buf()).await {
                Ok(store) => RuntimeState::Ready {
                    credential_reads: CredentialReadPool::from_store(store.clone()),
                    store,
                },
                Err(error) => RuntimeState::Blocked(blocked_reason(&error)),
            }
        }
        AccessHealthStatus::Insecure => RuntimeState::Blocked(AccessBlockedReason::Insecure),
        AccessHealthStatus::Corrupt => RuntimeState::Blocked(AccessBlockedReason::Corrupt),
        AccessHealthStatus::NewerSchema => RuntimeState::Blocked(AccessBlockedReason::NewerSchema),
        AccessHealthStatus::Locked => RuntimeState::Blocked(AccessBlockedReason::Locked),
        AccessHealthStatus::ReadOnly => RuntimeState::Blocked(AccessBlockedReason::ReadOnly),
        AccessHealthStatus::Unavailable => RuntimeState::Blocked(AccessBlockedReason::Unavailable),
    }
}

fn blocked_reason(error: &AccessStoreError) -> AccessBlockedReason {
    match error {
        AccessStoreError::InsecurePath { .. } | AccessStoreError::InsecurePermissions { .. } => {
            AccessBlockedReason::Insecure
        }
        AccessStoreError::Corrupt
        | AccessStoreError::IntegrityViolation { .. }
        | AccessStoreError::MalformedVocabulary
        | AccessStoreError::ForeignKeyViolation => AccessBlockedReason::Corrupt,
        AccessStoreError::UnsupportedSchema { .. } => AccessBlockedReason::NewerSchema,
        AccessStoreError::Locked => AccessBlockedReason::Locked,
        AccessStoreError::ReadOnly => AccessBlockedReason::ReadOnly,
        AccessStoreError::DiskFull
        | AccessStoreError::MissingParent { .. }
        | AccessStoreError::BootstrapConflict
        | AccessStoreError::InvalidBootstrapInput
        | AccessStoreError::IdentityUnavailable
        | AccessStoreError::ProjectAccessUnavailable
        | AccessStoreError::NotAuthorized
        | AccessStoreError::InvalidProjectLoadoutInput
        | AccessStoreError::ProjectLoadoutConflict
        | AccessStoreError::Unavailable(_) => AccessBlockedReason::Unavailable,
    }
}

pub(super) fn blocked_reason_for_diagnostics(error: &AccessStoreError) -> &'static str {
    match blocked_reason(error) {
        AccessBlockedReason::Insecure => "insecure",
        AccessBlockedReason::Corrupt => "corrupt",
        AccessBlockedReason::NewerSchema => "newer_schema",
        AccessBlockedReason::Locked => "locked",
        AccessBlockedReason::ReadOnly => "read_only",
        AccessBlockedReason::Unavailable => "unavailable",
    }
}

fn runtime_error(state: &RuntimeState, source: Option<&AccessStoreError>) -> AccessRuntimeError {
    match source {
        Some(AccessStoreError::BootstrapConflict) => AccessRuntimeError::BootstrapConflict,
        Some(AccessStoreError::InvalidBootstrapInput) => AccessRuntimeError::InvalidBootstrapInput,
        _ => match state {
            RuntimeState::SetupRequired(reason) => AccessRuntimeError::SetupRequired(*reason),
            RuntimeState::Blocked(reason) => AccessRuntimeError::Blocked(*reason),
            RuntimeState::Prepared => AccessRuntimeError::LifecycleUnavailable,
            RuntimeState::Ready { .. } => AccessRuntimeError::LifecycleUnavailable,
        },
    }
}

fn credential_runtime_error(error: AccessStoreError) -> CredentialLifecycleError {
    match error {
        AccessStoreError::NotAuthorized => CredentialLifecycleError::NotAuthorized,
        AccessStoreError::InvalidBootstrapInput | AccessStoreError::MalformedVocabulary => {
            CredentialLifecycleError::Invalid
        }
        _ => CredentialLifecycleError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_auth::{Authenticator, VerifiedIdentity};

    fn secure_test_path(directory: &tempfile::TempDir) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        directory.path().join("access.db")
    }

    fn input() -> BootstrapOwnerInput {
        BootstrapOwnerInput::new(
            VerifiedIdentity::local_credential(
                Authenticator::StaticBearer,
                "static-bearer:runtime-test",
            )
            .unwrap(),
            "Local",
            "Default",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn initialization_is_observational_for_missing_store() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap().join("access.db");

        let runtime = AccessRuntime::initialize(path.clone()).await;

        assert_eq!(
            runtime.status().await,
            AccessRuntimeStatus::SetupRequired(AccessSetupReason::Missing)
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn current_but_unbootstrapped_store_requires_setup_without_mutation() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path.clone()).await.unwrap();
        let before = store.metadata_for_test().await.unwrap();
        drop(store);

        let runtime = AccessRuntime::initialize(path.clone()).await;

        assert_eq!(
            runtime.status().await,
            AccessRuntimeStatus::SetupRequired(AccessSetupReason::Uninitialized)
        );
        let store = AccessStore::open(path).await.unwrap();
        assert_eq!(store.metadata_for_test().await.unwrap(), before);
    }

    #[tokio::test]
    async fn prepared_store_is_consume_only_and_general_access_stays_denied() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = directory.path().canonicalize().unwrap().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .activate_bootstrap_proof(super::super::ActivateProofInput {
                proof_id: "proof".into(),
                prepare_id: "prepare".into(),
                installation_id: "installation".into(),
                installation_generation: 1,
                proof_digest: [1; 32],
                manifest_digest: [2; 32],
                request_digest: [3; 32],
                idempotency_digest: [4; 32],
                credential_id: "credential".into(),
                credential_digest: [5; 32],
                proof_generation: 1,
                created_at: 10,
                expires_at: 100,
            })
            .await
            .unwrap();
        drop(store);

        let runtime = AccessRuntime::initialize(path).await;
        assert_eq!(
            runtime.status().await,
            AccessRuntimeStatus::SetupRequired(AccessSetupReason::Uninitialized)
        );
        assert!(matches!(
            runtime.store().await,
            Err(AccessRuntimeError::SetupRequired(
                AccessSetupReason::Uninitialized
            ))
        ));
        assert!(matches!(
            runtime.credential_reads().await,
            Err(AccessRuntimeError::SetupRequired(
                AccessSetupReason::Uninitialized
            ))
        ));
    }

    #[tokio::test]
    async fn canonical_v1_is_not_migrated_until_explicit_bootstrap() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(super::super::migrations::V1_METADATA_SCHEMA)
            .unwrap();
        connection
            .execute_batch(super::super::migrations::DOMAIN_SCHEMA)
            .unwrap();
        connection
            .execute(
                "INSERT INTO access_metadata(singleton,schema_version,schema_fingerprint,global_revision,updated_at) VALUES(1,?1,?2,0,123)",
                rusqlite::params![
                    super::super::migrations::V1_SCHEMA_VERSION,
                    super::super::migrations::V1_SCHEMA_FINGERPRINT
                ],
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let runtime = AccessRuntime::initialize(path.clone()).await;
        assert_eq!(
            runtime.status().await,
            AccessRuntimeStatus::SetupRequired(AccessSetupReason::Uninitialized)
        );
        let connection = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            super::super::migrations::V1_SCHEMA_VERSION
        );
        drop(connection);

        assert_eq!(
            runtime.bootstrap_owner(input()).await.unwrap(),
            BootstrapOutcome::Created
        );
        assert_eq!(runtime.status().await, AccessRuntimeStatus::Ready);
    }

    #[tokio::test]
    async fn explicit_bootstrap_promotes_runtime_and_restart_is_ready() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let runtime = AccessRuntime::initialize(path.clone()).await;

        assert_eq!(
            runtime.bootstrap_owner(input()).await.unwrap(),
            BootstrapOutcome::Created
        );
        assert_eq!(runtime.status().await, AccessRuntimeStatus::Ready);
        assert!(runtime.store().await.is_ok());

        let restarted = AccessRuntime::initialize(path).await;
        assert_eq!(restarted.status().await, AccessRuntimeStatus::Ready);
    }

    #[tokio::test]
    async fn concurrent_bootstrap_is_serialized_and_idempotent() {
        let directory = super::super::test_support::secure_tempdir();
        let runtime = AccessRuntime::initialize(secure_test_path(&directory)).await;
        let first = runtime.clone();
        let second = runtime.clone();
        let (first, second) = tokio::join!(
            first.bootstrap_owner(input()),
            second.bootstrap_owner(input())
        );

        let mut outcomes = [first.unwrap(), second.unwrap()];
        outcomes.sort_by_key(|outcome| match outcome {
            BootstrapOutcome::Created => 0,
            BootstrapOutcome::AlreadyApplied => 1,
        });
        assert_eq!(
            outcomes,
            [BootstrapOutcome::Created, BootstrapOutcome::AlreadyApplied]
        );
        assert_eq!(runtime.status().await, AccessRuntimeStatus::Ready);
    }

    #[tokio::test]
    async fn cancelling_request_does_not_cancel_lifecycle_promotion() {
        let directory = super::super::test_support::secure_tempdir();
        let runtime = AccessRuntime::initialize(secure_test_path(&directory)).await;
        let request_runtime = runtime.clone();
        let request = tokio::spawn(async move { request_runtime.bootstrap_owner(input()).await });
        tokio::task::yield_now().await;
        request.abort();

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if runtime.status().await == AccessRuntimeStatus::Ready {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(runtime.store().await.is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn insecure_store_is_typed_blocked_and_not_changed() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        std::fs::write(&path, []).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let runtime = AccessRuntime::initialize(path.clone()).await;

        assert_eq!(
            runtime.status().await,
            AccessRuntimeStatus::Blocked(AccessBlockedReason::Insecure)
        );
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[tokio::test]
    async fn corrupt_and_newer_stores_are_typed_blocked() {
        let corrupt_directory = super::super::test_support::secure_tempdir();
        let corrupt_path = secure_test_path(&corrupt_directory);
        std::fs::write(&corrupt_path, b"not sqlite").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&corrupt_path, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let corrupt = AccessRuntime::initialize(corrupt_path).await;
        assert_eq!(
            corrupt.status().await,
            AccessRuntimeStatus::Blocked(AccessBlockedReason::Corrupt)
        );

        let newer_directory = super::super::test_support::secure_tempdir();
        let newer_path = secure_test_path(&newer_directory);
        let connection = rusqlite::Connection::open(&newer_path).unwrap();
        connection
            .pragma_update(
                None,
                "user_version",
                super::super::migrations::SCHEMA_VERSION + 1,
            )
            .unwrap();
        drop(connection);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&newer_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let newer = AccessRuntime::initialize(newer_path).await;
        assert_eq!(
            newer.status().await,
            AccessRuntimeStatus::Blocked(AccessBlockedReason::NewerSchema)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_only_store_is_typed_blocked() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path.clone()).await.unwrap();
        store.bootstrap_owner(input()).await.unwrap();
        drop(store);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();

        let runtime = AccessRuntime::initialize(path).await;
        assert_eq!(
            runtime.status().await,
            AccessRuntimeStatus::Blocked(AccessBlockedReason::ReadOnly)
        );
    }

    #[tokio::test]
    async fn delete_journal_mode_is_rejected_as_corrupt() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path.clone()).await.unwrap();
        store.bootstrap_owner(input()).await.unwrap();
        drop(store);
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "journal_mode", "DELETE")
            .unwrap();
        drop(connection);

        let runtime = AccessRuntime::initialize(path).await;
        assert_eq!(
            runtime.status().await,
            AccessRuntimeStatus::Blocked(AccessBlockedReason::Corrupt)
        );
    }

    #[tokio::test]
    async fn rollback_journal_is_typed_locked() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path.clone()).await.unwrap();
        store.bootstrap_owner(input()).await.unwrap();
        drop(store);
        let journal = super::super::store::sidecar_path(&path, "-journal");
        std::fs::write(&journal, []).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let runtime = AccessRuntime::initialize(path).await;
        assert_eq!(
            runtime.status().await,
            AccessRuntimeStatus::Blocked(AccessBlockedReason::Locked)
        );
    }

    #[tokio::test]
    async fn file_stash_resolution_distinguishes_unmapped_identity() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path.clone()).await.unwrap();
        store.bootstrap_owner(input()).await.unwrap();
        drop(store);
        let runtime = AccessRuntime::initialize(path).await;
        let missing = VerifiedIdentity::local_credential(
            Authenticator::StaticBearer,
            "static-bearer:not-mapped",
        )
        .unwrap();
        assert!(matches!(
            runtime.resolve_file_stash_principal(missing).await,
            Err(FileStashPrincipalResolutionError::IdentityUnavailable)
        ));
    }

    #[tokio::test]
    async fn file_stash_resolution_preserves_blocked_runtime_reason() {
        let runtime = AccessRuntime::blocked_unavailable();
        let identity =
            VerifiedIdentity::local_credential(Authenticator::StaticBearer, "static-bearer:any")
                .unwrap();
        assert!(matches!(
            runtime.resolve_file_stash_principal(identity).await,
            Err(FileStashPrincipalResolutionError::Runtime(
                AccessRuntimeError::Blocked(AccessBlockedReason::Unavailable)
            ))
        ));
    }
}
