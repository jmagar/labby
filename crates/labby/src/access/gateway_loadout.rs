use std::future::Future;

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::manager::GatewayManager;
use labby_gateway::gateway::manager::{
    GatewayRuntimeConfigGeneration, LoadoutMcpCatalogPublicationError,
    PublishedLoadoutMcpCatalogSnapshot, PublishedRuntimeLoadoutSnapshot,
};
use labby_runtime::gateway_config::GatewayLoadoutConfig;
use thiserror::Error;

use super::error::AccessStoreError;
use super::{
    AccessRuntime, AccessRuntimeError, AssignProjectLoadoutInput, AssignProjectLoadoutOutcome,
    AuthorizeProjectInput, Permission, ProjectPermissionSnapshot,
};

const CONTEXT_ATTEMPTS: usize = 3;

/// One coherent, point-in-time Project and published-runtime Loadout view.
///
/// This is deliberately non-`Clone` and is not a dispatch grant: it binds neither an exact
/// gateway action/target nor a catalog generation. A consumer must still authorize at dispatch.
pub(crate) struct ProjectRuntimeLoadoutContext {
    access: ProjectPermissionSnapshot,
    loadout: GatewayLoadoutConfig,
    runtime_config_generation: GatewayRuntimeConfigGeneration,
}

impl ProjectRuntimeLoadoutContext {
    pub(crate) fn access(&self) -> &ProjectPermissionSnapshot {
        &self.access
    }

    pub(crate) fn loadout(&self) -> &GatewayLoadoutConfig {
        &self.loadout
    }

    pub(crate) fn runtime_config_generation(&self) -> GatewayRuntimeConfigGeneration {
        self.runtime_config_generation
    }
}

/// Stable Project authorization/runtime-Loadout evidence paired with one
/// common-interval MCP catalog. Observational and unmounted; not a grant.
pub(crate) struct ProjectRuntimeMcpCatalogContext {
    access: ProjectPermissionSnapshot,
    catalog: PublishedLoadoutMcpCatalogSnapshot,
}

impl ProjectRuntimeMcpCatalogContext {
    pub(crate) fn access(&self) -> &ProjectPermissionSnapshot {
        &self.access
    }
    pub(crate) fn catalog(&self) -> &PublishedLoadoutMcpCatalogSnapshot {
        &self.catalog
    }

    pub(crate) fn same_publication_as(&self, other: &Self) -> bool {
        self.access == other.access && self.catalog.same_publication_as(&other.catalog)
    }
}

#[derive(Debug, Error)]
pub(crate) enum ProjectRuntimeMcpCatalogError {
    #[error("access runtime is unavailable")]
    RuntimeUnavailable,
    #[error("project access is unavailable")]
    ProjectAccessUnavailable,
    #[error("access persistence is unavailable")]
    AccessUnavailable,
    #[error("gateway MCP catalog is unavailable")]
    CatalogUnavailable,
    #[error("project MCP catalog snapshot is unstable")]
    SnapshotUnstable,
}

pub(crate) async fn project_runtime_mcp_catalog_context(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    identity: VerifiedIdentity,
    project_id: impl Into<String>,
    permission: Permission,
) -> Result<ProjectRuntimeMcpCatalogContext, ProjectRuntimeMcpCatalogError> {
    let store = runtime
        .store()
        .await
        .map_err(|_| ProjectRuntimeMcpCatalogError::RuntimeUnavailable)?;
    let project_id = project_id.into();
    stable_project_mcp_context(
        || {
            let store = store.clone();
            let identity = identity.clone();
            let project_id = project_id.clone();
            async move {
                store
                    .authorize_project(AuthorizeProjectInput::new(identity, project_id, permission))
                    .await
            }
        },
        |name| async move { manager.published_loadout_mcp_catalog_snapshot(&name).await },
    )
    .await
}

/// At most three outer attempts: six Access reads and six manager snapshot
/// invocations. Each manager invocation independently caps its internal
/// G-T-R-Q-S protocol at three attempts (18 inner attempts worst case).
async fn stable_project_mcp_context<PF, PFut, MF, MFut>(
    mut read_project: PF,
    mut read_catalog: MF,
) -> Result<ProjectRuntimeMcpCatalogContext, ProjectRuntimeMcpCatalogError>
where
    PF: FnMut() -> PFut,
    PFut: Future<Output = Result<ProjectPermissionSnapshot, AccessStoreError>>,
    MF: FnMut(String) -> MFut,
    MFut: Future<
        Output = Result<PublishedLoadoutMcpCatalogSnapshot, LoadoutMcpCatalogPublicationError>,
    >,
{
    for _ in 0..CONTEXT_ATTEMPTS {
        let first_access = read_project().await.map_err(map_mcp_context_access_error)?;
        let first_catalog = read_catalog(first_access.loadout_name.clone())
            .await
            .map_err(map_mcp_catalog_error)?;
        let second_access = read_project().await.map_err(map_mcp_context_access_error)?;
        let second_catalog = read_catalog(second_access.loadout_name.clone())
            .await
            .map_err(map_mcp_catalog_error)?;
        if first_access == second_access && first_catalog.same_publication_as(&second_catalog) {
            return Ok(ProjectRuntimeMcpCatalogContext {
                access: second_access,
                catalog: second_catalog,
            });
        }
    }
    Err(ProjectRuntimeMcpCatalogError::SnapshotUnstable)
}

fn map_mcp_context_access_error(error: AccessStoreError) -> ProjectRuntimeMcpCatalogError {
    if is_project_access_denial(&error) {
        ProjectRuntimeMcpCatalogError::ProjectAccessUnavailable
    } else {
        ProjectRuntimeMcpCatalogError::AccessUnavailable
    }
}

fn map_mcp_catalog_error(
    error: LoadoutMcpCatalogPublicationError,
) -> ProjectRuntimeMcpCatalogError {
    match error {
        LoadoutMcpCatalogPublicationError::MissingLoadout
        | LoadoutMcpCatalogPublicationError::MissingPool
        | LoadoutMcpCatalogPublicationError::CatalogUnavailable => {
            ProjectRuntimeMcpCatalogError::CatalogUnavailable
        }
        LoadoutMcpCatalogPublicationError::Unstable => {
            ProjectRuntimeMcpCatalogError::SnapshotUnstable
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum ProjectRuntimeLoadoutError {
    #[error("access runtime is unavailable")]
    RuntimeUnavailable,
    #[error("project access is unavailable")]
    ProjectAccessUnavailable,
    #[error("access persistence is unavailable")]
    AccessUnavailable,
    #[error("gateway loadout is unavailable")]
    LoadoutUnavailable,
    #[error("project runtime snapshot is unstable")]
    SnapshotUnstable,
}

/// Resolves a stable A-G-A-G view across the independent Access and Gateway stores.
///
/// The first Access read is intentionally before any Gateway read, so unauthorized, unknown, or
/// malformed Project selectors cannot probe Gateway Loadout state.
pub(crate) async fn project_runtime_loadout_context(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    identity: VerifiedIdentity,
    project_id: impl Into<String>,
    permission: Permission,
) -> Result<ProjectRuntimeLoadoutContext, ProjectRuntimeLoadoutError> {
    let store = runtime
        .store()
        .await
        .map_err(|_| ProjectRuntimeLoadoutError::RuntimeUnavailable)?;
    let project_id = project_id.into();

    stable_runtime_context(
        || {
            let store = store.clone();
            let identity = identity.clone();
            let project_id = project_id.clone();
            async move {
                store
                    .authorize_project(AuthorizeProjectInput::new(identity, project_id, permission))
                    .await
            }
        },
        |name| async move { manager.published_runtime_loadout_snapshot(&name).await },
    )
    .await
}

async fn stable_runtime_context<AF, AFut, GF, GFut>(
    mut read_access: AF,
    mut read_gateway: GF,
) -> Result<ProjectRuntimeLoadoutContext, ProjectRuntimeLoadoutError>
where
    AF: FnMut() -> AFut,
    AFut: Future<Output = Result<ProjectPermissionSnapshot, AccessStoreError>>,
    GF: FnMut(String) -> GFut,
    GFut: Future<Output = PublishedRuntimeLoadoutSnapshot>,
{
    for _ in 0..CONTEXT_ATTEMPTS {
        let first_access = read_access()
            .await
            .map_err(map_runtime_context_access_error)?;
        let first_gateway = read_gateway(first_access.loadout_name.clone()).await;
        let second_access = read_access()
            .await
            .map_err(map_runtime_context_access_error)?;
        let second_gateway = read_gateway(second_access.loadout_name.clone()).await;

        if first_access == second_access && first_gateway == second_gateway {
            let runtime_config_generation = second_gateway.generation();
            let loadout = second_gateway
                .into_loadout()
                .ok_or(ProjectRuntimeLoadoutError::LoadoutUnavailable)?;
            return Ok(ProjectRuntimeLoadoutContext {
                access: second_access,
                loadout,
                runtime_config_generation,
            });
        }
    }
    Err(ProjectRuntimeLoadoutError::SnapshotUnstable)
}

fn map_runtime_context_access_error(error: AccessStoreError) -> ProjectRuntimeLoadoutError {
    if is_project_access_denial(&error) {
        ProjectRuntimeLoadoutError::ProjectAccessUnavailable
    } else {
        ProjectRuntimeLoadoutError::AccessUnavailable
    }
}

fn is_project_access_denial(error: &AccessStoreError) -> bool {
    matches!(
        error,
        AccessStoreError::NotAuthorized
            | AccessStoreError::IdentityUnavailable
            | AccessStoreError::ProjectAccessUnavailable
            | AccessStoreError::InvalidProjectLoadoutInput
    )
}

/// Redacted failure contract for the unmounted Gateway compatibility adapter.
#[derive(Debug, Error)]
pub(crate) enum GatewayLoadoutAssignmentError {
    #[error("access runtime is unavailable")]
    RuntimeUnavailable,
    #[error("project loadout assignment input is invalid")]
    InvalidInput,
    #[error("project access is unavailable")]
    ProjectAccessUnavailable,
    #[error("project already has a different loadout assignment")]
    ProjectLoadoutConflict,
    #[error("access persistence is unavailable")]
    AccessUnavailable,
    #[error("gateway loadout is unavailable")]
    LoadoutUnavailable,
}

/// Admits one point-in-time desired-config Loadout name before atomically assigning it.
///
/// This is intentionally crate-private and unmounted. Validation is compatibility checking, not
/// authorization: access is checked before Gateway lookup and again by the store mutation.
pub(crate) async fn assign_admitted_project_loadout(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    identity: VerifiedIdentity,
    project_id: impl Into<String>,
    loadout_name: impl Into<String>,
) -> Result<AssignProjectLoadoutOutcome, GatewayLoadoutAssignmentError> {
    assign_with_validator(
        runtime,
        identity,
        project_id.into(),
        loadout_name.into(),
        |name| async move { manager.loadout_get(&name).await.map(|_| ()) },
    )
    .await
}

async fn assign_with_validator<F, Fut, E>(
    runtime: &AccessRuntime,
    identity: VerifiedIdentity,
    project_id: String,
    loadout_name: String,
    validator: F,
) -> Result<AssignProjectLoadoutOutcome, GatewayLoadoutAssignmentError>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<(), E>>,
{
    let store = runtime
        .store()
        .await
        .map_err(|_error: AccessRuntimeError| GatewayLoadoutAssignmentError::RuntimeUnavailable)?;
    // Authorize the opaque Project selector before validating any caller-controlled Loadout fact.
    // The store query safely treats malformed or unknown selectors as the same denial.
    store
        .authorize_project_management_without_loadout(identity.clone(), project_id.clone())
        .await
        .map_err(map_access_error)?;
    let input = AssignProjectLoadoutInput::new(identity, project_id, loadout_name)
        .map_err(map_access_error)?;
    validator(input.loadout_name().to_owned())
        .await
        .map_err(|_| GatewayLoadoutAssignmentError::LoadoutUnavailable)?;
    // This immediate transaction resolves identity, membership, role, and status again. The
    // preflight above is not a grant and cannot survive a concurrent revocation.
    store
        .assign_project_loadout(input)
        .await
        .map_err(map_access_error)
}

fn map_access_error(error: AccessStoreError) -> GatewayLoadoutAssignmentError {
    match error {
        AccessStoreError::InvalidProjectLoadoutInput => GatewayLoadoutAssignmentError::InvalidInput,
        AccessStoreError::ProjectAccessUnavailable
        | AccessStoreError::IdentityUnavailable
        | AccessStoreError::NotAuthorized => {
            GatewayLoadoutAssignmentError::ProjectAccessUnavailable
        }
        AccessStoreError::ProjectLoadoutConflict => {
            GatewayLoadoutAssignmentError::ProjectLoadoutConflict
        }
        AccessStoreError::Locked
        | AccessStoreError::Corrupt
        | AccessStoreError::DiskFull
        | AccessStoreError::ReadOnly
        | AccessStoreError::InsecurePath { .. }
        | AccessStoreError::MissingParent { .. }
        | AccessStoreError::InsecurePermissions { .. }
        | AccessStoreError::UnsupportedSchema { .. }
        | AccessStoreError::MigrationApprovalRequired { .. }
        | AccessStoreError::MigrationEvidenceInvalid { .. }
        | AccessStoreError::IntegrityViolation { .. }
        | AccessStoreError::ForeignKeyViolation
        | AccessStoreError::BootstrapConflict
        | AccessStoreError::InvalidBootstrapInput
        | AccessStoreError::InvalidTeamInput
        | AccessStoreError::TeamUnavailable
        | AccessStoreError::LastActiveTeamOwner
        | AccessStoreError::MalformedVocabulary
        | AccessStoreError::Unavailable(_) => GatewayLoadoutAssignmentError::AccessUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[cfg(feature = "proxy-testkit")]
    use std::sync::atomic::AtomicUsize;

    use labby_auth::{Authenticator, VerifiedIdentity};
    #[cfg(feature = "proxy-testkit")]
    use labby_gateway::gateway::config_store::FsGatewayConfigStore;
    #[cfg(feature = "proxy-testkit")]
    use labby_gateway::gateway::manager::GatewayRuntimeHandle;
    #[cfg(feature = "proxy-testkit")]
    use labby_gateway::upstream::pool::UpstreamPool;
    #[cfg(feature = "proxy-testkit")]
    use labby_runtime::gateway_config::{
        GatewayConfig, GatewayLoadoutConfig, VirtualServerConfig, VirtualServerSurfacesConfig,
    };

    use super::*;
    use crate::access::BootstrapOwnerInput;

    fn identity(credential: &str) -> VerifiedIdentity {
        VerifiedIdentity::local_credential(Authenticator::StaticBearer, credential).unwrap()
    }

    async fn fixture() -> (tempfile::TempDir, AccessRuntime, VerifiedIdentity) {
        let directory = super::super::test_support::secure_tempdir();
        let runtime = AccessRuntime::initialize(directory.path().join("access.db")).await;
        let owner = identity("static-bearer:gateway-adapter-owner");
        runtime
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        (directory, runtime, owner)
    }

    #[tokio::test]
    async fn validator_is_not_invoked_for_unauthorized_identity() {
        let (_directory, runtime, _owner) = fixture().await;
        let invoked = Arc::new(AtomicBool::new(false));
        let marker = Arc::clone(&invoked);
        let result = assign_with_validator(
            &runtime,
            identity("static-bearer:unknown"),
            "bootstrap-default".into(),
            "production".into(),
            move |_| async move {
                marker.store(true, Ordering::SeqCst);
                Ok::<_, ()>(())
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(GatewayLoadoutAssignmentError::ProjectAccessUnavailable)
        ));
        assert!(!invoked.load(Ordering::SeqCst));

        let result = assign_with_validator(
            &runtime,
            identity("static-bearer:unknown"),
            "bad\nproject".into(),
            "bad\nloadout".into(),
            |_| async { Err::<(), ()>(()) },
        )
        .await;
        assert!(matches!(
            result,
            Err(GatewayLoadoutAssignmentError::ProjectAccessUnavailable)
        ));
    }

    #[tokio::test]
    async fn validation_failure_has_no_state_change() {
        let (_directory, runtime, owner) = fixture().await;
        let store = runtime.store().await.unwrap();
        let before = store.loadout_state_for_test().await.unwrap();
        let result = assign_with_validator(
            &runtime,
            owner,
            "bootstrap-default".into(),
            "missing".into(),
            |_| async { Err::<(), _>("raw gateway detail") },
        )
        .await;

        assert!(matches!(
            &result,
            Err(GatewayLoadoutAssignmentError::LoadoutUnavailable)
        ));
        assert_eq!(
            result.unwrap_err().to_string(),
            "gateway loadout is unavailable"
        );
        assert_eq!(store.loadout_state_for_test().await.unwrap(), before);
    }

    #[test]
    fn persistence_error_details_are_redacted() {
        let error = map_access_error(AccessStoreError::Unavailable(
            "secret path and sqlite detail".into(),
        ));

        assert_eq!(error.to_string(), "access persistence is unavailable");
    }

    #[tokio::test]
    async fn successful_validation_assigns_and_audits() {
        let (_directory, runtime, owner) = fixture().await;
        let store = runtime.store().await.unwrap();
        let outcome = assign_with_validator(
            &runtime,
            owner,
            "bootstrap-default".into(),
            "production".into(),
            |name| async move {
                assert_eq!(name, "production");
                Ok::<_, ()>(())
            },
        )
        .await
        .unwrap();

        assert_eq!(outcome, AssignProjectLoadoutOutcome::Assigned);
        assert_eq!(
            store.loadout_state_for_test().await.unwrap(),
            (1, 2, 1, 1, 4)
        );
        assert_eq!(
            store.loadout_audit_for_test().await.unwrap().0,
            "access.project_loadout.assign"
        );
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn concrete_adapter_reads_desired_gateway_loadouts() {
        let (directory, runtime, owner) = fixture().await;
        let gateway_path = directory.path().join("gateway.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            GatewayRuntimeHandle::default(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        );
        manager
            .try_seed_config(GatewayConfig {
                loadouts: vec![GatewayLoadoutConfig {
                    name: "production".into(),
                    ..GatewayLoadoutConfig::default()
                }],
                ..GatewayConfig::default()
            })
            .await
            .unwrap();

        assert!(matches!(
            assign_admitted_project_loadout(
                &runtime,
                &manager,
                owner.clone(),
                "bootstrap-default",
                "missing",
            )
            .await,
            Err(GatewayLoadoutAssignmentError::LoadoutUnavailable)
        ));
        assert_eq!(
            assign_admitted_project_loadout(
                &runtime,
                &manager,
                owner,
                "bootstrap-default",
                "production",
            )
            .await
            .unwrap(),
            AssignProjectLoadoutOutcome::Assigned
        );
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn runtime_context_combines_exact_access_and_published_loadout_snapshots() {
        let (directory, runtime, owner) = fixture().await;
        let gateway_path = directory.path().join("runtime-context-gateway.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            GatewayRuntimeHandle::default(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        );
        manager
            .try_seed_config(GatewayConfig {
                loadouts: vec![GatewayLoadoutConfig {
                    name: "production".into(),
                    ..GatewayLoadoutConfig::default()
                }],
                ..GatewayConfig::default()
            })
            .await
            .unwrap();
        assign_admitted_project_loadout(
            &runtime,
            &manager,
            owner.clone(),
            "bootstrap-default",
            "production",
        )
        .await
        .unwrap();

        let context = project_runtime_loadout_context(
            &runtime,
            &manager,
            owner,
            "bootstrap-default",
            Permission::AssetUse,
        )
        .await
        .unwrap();

        assert_eq!(context.access().project_id, "bootstrap-default");
        assert_eq!(context.access().permission, Permission::AssetUse);
        assert_eq!(context.access().loadout_name, "production");
        assert_eq!(context.loadout().name, "production");
        assert_eq!(
            context.runtime_config_generation(),
            manager
                .published_runtime_loadout_snapshot("production")
                .await
                .generation()
        );
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn stable_project_mcp_catalog_context_returns_exact_authorized_catalog() {
        let (directory, runtime, owner) = fixture().await;
        let gateway_runtime = GatewayRuntimeHandle::default();
        gateway_runtime
            .swap(Some(Arc::new(UpstreamPool::new())))
            .await;
        let gateway_path = directory.path().join("project-mcp-context.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            gateway_runtime,
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        )
        .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry()));
        manager
            .try_seed_config(GatewayConfig {
                loadouts: vec![GatewayLoadoutConfig {
                    name: "production".into(),
                    services: vec!["setup".into()],
                    ..Default::default()
                }],
                virtual_servers: vec![VirtualServerConfig {
                    id: "setup".into(),
                    service: "setup".into(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        mcp: true,
                        ..Default::default()
                    },
                    mcp_policy: None,
                }],
                ..Default::default()
            })
            .await
            .unwrap();
        runtime.store().await.unwrap().execute_test_statement(
            "INSERT INTO project_loadouts VALUES ('bootstrap-local','bootstrap-default','production','bootstrap-owner',2,2)"
        ).await.unwrap();

        let access_reads = Arc::new(AtomicUsize::new(0));
        let manager_reads = Arc::new(AtomicUsize::new(0));
        let store = runtime.store().await.unwrap();
        let context = stable_project_mcp_context(
            || {
                let store = store.clone();
                let owner = owner.clone();
                let reads = Arc::clone(&access_reads);
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    store
                        .authorize_project(AuthorizeProjectInput::new(
                            owner,
                            "bootstrap-default",
                            Permission::AssetUse,
                        ))
                        .await
                }
            },
            |name| {
                let manager = manager.clone();
                let reads = Arc::clone(&manager_reads);
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    manager.published_loadout_mcp_catalog_snapshot(&name).await
                }
            },
        )
        .await
        .expect("stable context");
        assert_eq!(context.access().project_id, "bootstrap-default");
        assert_eq!(context.access().loadout_name, "production");
        assert!(context.catalog().tools().routes().is_empty());
        assert_eq!(context.catalog().services().services()[0].name(), "setup");
        assert_eq!(access_reads.load(Ordering::SeqCst), 2);
        assert_eq!(manager_reads.load(Ordering::SeqCst), 2);

        let access_reads = Arc::new(AtomicUsize::new(0));
        let manager_reads = Arc::new(AtomicUsize::new(0));
        let denied = stable_project_mcp_context(
            || {
                let store = store.clone();
                let owner = owner.clone();
                let reads = Arc::clone(&access_reads);
                async move {
                    if reads.fetch_add(1, Ordering::SeqCst) == 1 {
                        return Err(AccessStoreError::NotAuthorized);
                    }
                    store
                        .authorize_project(AuthorizeProjectInput::new(
                            owner,
                            "bootstrap-default",
                            Permission::AssetUse,
                        ))
                        .await
                }
            },
            |name| {
                let manager = manager.clone();
                let reads = Arc::clone(&manager_reads);
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    manager.published_loadout_mcp_catalog_snapshot(&name).await
                }
            },
        )
        .await;
        assert!(matches!(
            denied,
            Err(ProjectRuntimeMcpCatalogError::ProjectAccessUnavailable)
        ));
        assert_eq!(access_reads.load(Ordering::SeqCst), 2);
        assert_eq!(manager_reads.load(Ordering::SeqCst), 1);

        let access_reads = Arc::new(AtomicUsize::new(0));
        let manager_reads = Arc::new(AtomicUsize::new(0));
        let unstable = stable_project_mcp_context(
            || {
                let store = store.clone();
                let owner = owner.clone();
                let reads = Arc::clone(&access_reads);
                async move {
                    let mut access = store
                        .authorize_project(AuthorizeProjectInput::new(
                            owner,
                            "bootstrap-default",
                            Permission::AssetUse,
                        ))
                        .await?;
                    access.global_revision += reads.fetch_add(1, Ordering::SeqCst) as u64;
                    Ok(access)
                }
            },
            |name| {
                let manager = manager.clone();
                let reads = Arc::clone(&manager_reads);
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    manager.published_loadout_mcp_catalog_snapshot(&name).await
                }
            },
        )
        .await;
        assert!(matches!(
            unstable,
            Err(ProjectRuntimeMcpCatalogError::SnapshotUnstable)
        ));
        assert_eq!(access_reads.load(Ordering::SeqCst), CONTEXT_ATTEMPTS * 2);
        assert_eq!(manager_reads.load(Ordering::SeqCst), CONTEXT_ATTEMPTS * 2);

        let access_reads = Arc::new(AtomicUsize::new(0));
        let manager_reads = Arc::new(AtomicUsize::new(0));
        let unavailable = stable_project_mcp_context(
            || {
                let store = store.clone();
                let owner = owner.clone();
                let reads = Arc::clone(&access_reads);
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    store
                        .authorize_project(AuthorizeProjectInput::new(
                            owner,
                            "bootstrap-default",
                            Permission::AssetUse,
                        ))
                        .await
                }
            },
            |_| {
                let manager = manager.clone();
                let reads = Arc::clone(&manager_reads);
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    manager
                        .published_loadout_mcp_catalog_snapshot("missing")
                        .await
                }
            },
        )
        .await;
        assert!(matches!(
            unavailable,
            Err(ProjectRuntimeMcpCatalogError::CatalogUnavailable)
        ));
        assert_eq!(access_reads.load(Ordering::SeqCst), 1);
        assert_eq!(manager_reads.load(Ordering::SeqCst), 1);

        let access_reads = Arc::new(AtomicUsize::new(0));
        let manager_reads = Arc::new(AtomicUsize::new(0));
        let aba = stable_project_mcp_context(
            || {
                let store = store.clone();
                let owner = owner.clone();
                let reads = Arc::clone(&access_reads);
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    store
                        .authorize_project(AuthorizeProjectInput::new(
                            owner,
                            "bootstrap-default",
                            Permission::AssetUse,
                        ))
                        .await
                }
            },
            |name| {
                let manager = manager.clone();
                let reads = Arc::clone(&manager_reads);
                async move {
                    let call = reads.fetch_add(1, Ordering::SeqCst);
                    let snapshot = manager.published_loadout_mcp_catalog_snapshot(&name).await;
                    if call == 0 {
                        let config = manager.current_config().await;
                        manager.try_seed_config(config.clone()).await.unwrap();
                        manager.try_seed_config(config).await.unwrap();
                    }
                    snapshot
                }
            },
        )
        .await
        .expect("manager ABA retries");
        assert_eq!(aba.access().project_id, "bootstrap-default");
        assert_eq!(access_reads.load(Ordering::SeqCst), 4);
        assert_eq!(manager_reads.load(Ordering::SeqCst), 4);

        let access_reads = Arc::new(AtomicUsize::new(0));
        let manager_reads = Arc::new(AtomicUsize::new(0));
        let unstable_manager = stable_project_mcp_context(
            || {
                let store = store.clone();
                let owner = owner.clone();
                let reads = Arc::clone(&access_reads);
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    store
                        .authorize_project(AuthorizeProjectInput::new(
                            owner,
                            "bootstrap-default",
                            Permission::AssetUse,
                        ))
                        .await
                }
            },
            |name| {
                let manager = manager.clone();
                let reads = Arc::clone(&manager_reads);
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    let snapshot = manager.published_loadout_mcp_catalog_snapshot(&name).await;
                    let config = manager.current_config().await;
                    manager.try_seed_config(config).await.unwrap();
                    snapshot
                }
            },
        )
        .await;
        assert!(matches!(
            unstable_manager,
            Err(ProjectRuntimeMcpCatalogError::SnapshotUnstable)
        ));
        assert_eq!(access_reads.load(Ordering::SeqCst), CONTEXT_ATTEMPTS * 2);
        assert_eq!(manager_reads.load(Ordering::SeqCst), CONTEXT_ATTEMPTS * 2);
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn unauthorized_or_malformed_project_is_denied_before_gateway_resolution() {
        let (directory, runtime, _owner) = fixture().await;
        let gateway_path = directory.path().join("denial-gateway.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            GatewayRuntimeHandle::default(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        );

        let result = project_runtime_loadout_context(
            &runtime,
            &manager,
            identity("static-bearer:unknown"),
            "bad\nproject",
            Permission::AssetUse,
        )
        .await;

        assert!(matches!(
            result,
            Err(ProjectRuntimeLoadoutError::ProjectAccessUnavailable)
        ));

        let gateway_reads = Arc::new(AtomicUsize::new(0));
        let observed_reads = Arc::clone(&gateway_reads);
        let result = stable_runtime_context(
            || async { Err(AccessStoreError::NotAuthorized) },
            |name| {
                let manager = manager.clone();
                let observed_reads = Arc::clone(&observed_reads);
                async move {
                    observed_reads.fetch_add(1, Ordering::SeqCst);
                    manager.published_runtime_loadout_snapshot(&name).await
                }
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(ProjectRuntimeLoadoutError::ProjectAccessUnavailable)
        ));
        assert_eq!(gateway_reads.load(Ordering::SeqCst), 0);

        let manager_reads = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&manager_reads);
        let result = stable_project_mcp_context(
            || async { Err(AccessStoreError::NotAuthorized) },
            |name| {
                let manager = manager.clone();
                let observed = Arc::clone(&observed);
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    manager.published_loadout_mcp_catalog_snapshot(&name).await
                }
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(ProjectRuntimeMcpCatalogError::ProjectAccessUnavailable)
        ));
        assert_eq!(manager_reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn missing_published_loadout_is_redacted() {
        let (directory, runtime, owner) = fixture().await;
        let gateway_path = directory.path().join("missing-runtime-loadout.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            GatewayRuntimeHandle::default(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        );
        let store = runtime.store().await.unwrap();
        store
            .execute_test_statement(
                "INSERT INTO project_loadouts VALUES
                 ('bootstrap-local','bootstrap-default','secret-missing-name','bootstrap-owner',2,2)",
            )
            .await
            .unwrap();

        let error = match project_runtime_loadout_context(
            &runtime,
            &manager,
            owner,
            "bootstrap-default",
            Permission::ProjectRead,
        )
        .await
        {
            Ok(_) => panic!("missing published loadout must fail closed"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            ProjectRuntimeLoadoutError::LoadoutUnavailable
        ));
        assert_eq!(error.to_string(), "gateway loadout is unavailable");
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn bounded_retries_end_in_redacted_snapshot_unstable() {
        let (directory, runtime, owner) = fixture().await;
        let gateway_path = directory.path().join("unstable-runtime-loadout.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            GatewayRuntimeHandle::default(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        );
        manager
            .try_seed_config(GatewayConfig {
                loadouts: vec![GatewayLoadoutConfig {
                    name: "production".into(),
                    ..GatewayLoadoutConfig::default()
                }],
                ..GatewayConfig::default()
            })
            .await
            .unwrap();
        let store = runtime.store().await.unwrap();
        store
            .execute_test_statement(
                "INSERT INTO project_loadouts VALUES
                 ('bootstrap-local','bootstrap-default','production','bootstrap-owner',2,2)",
            )
            .await
            .unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let access_reads = Arc::clone(&reads);

        let error = match stable_runtime_context(
            || {
                let store = store.clone();
                let owner = owner.clone();
                let access_reads = Arc::clone(&access_reads);
                async move {
                    let mut snapshot = store
                        .authorize_project(AuthorizeProjectInput::new(
                            owner,
                            "bootstrap-default",
                            Permission::AssetUse,
                        ))
                        .await?;
                    snapshot.global_revision += access_reads.fetch_add(1, Ordering::SeqCst) as u64;
                    Ok(snapshot)
                }
            },
            |name| {
                let manager = manager.clone();
                async move { manager.published_runtime_loadout_snapshot(&name).await }
            },
        )
        .await
        {
            Ok(_) => panic!("continuous revision churn must not produce a context"),
            Err(error) => error,
        };

        assert_eq!(reads.load(Ordering::SeqCst), CONTEXT_ATTEMPTS * 2);
        assert!(matches!(
            error,
            ProjectRuntimeLoadoutError::SnapshotUnstable
        ));
        assert_eq!(error.to_string(), "project runtime snapshot is unstable");
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn transient_gateway_publication_change_retries_to_one_stable_context() {
        let (directory, runtime, owner) = fixture().await;
        let gateway_path = directory.path().join("retry-runtime-loadout.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            GatewayRuntimeHandle::default(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        );
        let alpha = GatewayLoadoutConfig {
            name: "production".into(),
            description: Some("alpha".into()),
            ..GatewayLoadoutConfig::default()
        };
        let bravo = GatewayLoadoutConfig {
            name: "production".into(),
            description: Some("bravo".into()),
            ..GatewayLoadoutConfig::default()
        };
        manager
            .try_seed_config(GatewayConfig {
                loadouts: vec![alpha],
                ..GatewayConfig::default()
            })
            .await
            .unwrap();
        let store = runtime.store().await.unwrap();
        store
            .execute_test_statement(
                "INSERT INTO project_loadouts VALUES
                 ('bootstrap-local','bootstrap-default','production','bootstrap-owner',2,2)",
            )
            .await
            .unwrap();
        let gateway_reads = Arc::new(AtomicUsize::new(0));

        let context = stable_runtime_context(
            || {
                let store = store.clone();
                let owner = owner.clone();
                async move {
                    store
                        .authorize_project(AuthorizeProjectInput::new(
                            owner,
                            "bootstrap-default",
                            Permission::AssetUse,
                        ))
                        .await
                }
            },
            |name| {
                let manager = manager.clone();
                let bravo = bravo.clone();
                let gateway_reads = Arc::clone(&gateway_reads);
                async move {
                    if gateway_reads.fetch_add(1, Ordering::SeqCst) == 1 {
                        manager
                            .try_seed_config(GatewayConfig {
                                loadouts: vec![bravo],
                                ..GatewayConfig::default()
                            })
                            .await
                            .unwrap();
                    }
                    manager.published_runtime_loadout_snapshot(&name).await
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(gateway_reads.load(Ordering::SeqCst), 4);
        assert_eq!(context.loadout().description.as_deref(), Some("bravo"));
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn continuous_gateway_publication_churn_exhausts_the_bound() {
        let (directory, runtime, owner) = fixture().await;
        let gateway_path = directory.path().join("gateway-churn.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            GatewayRuntimeHandle::default(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        );
        let store = runtime.store().await.unwrap();
        store
            .execute_test_statement(
                "INSERT INTO project_loadouts VALUES
                 ('bootstrap-local','bootstrap-default','production','bootstrap-owner',2,2)",
            )
            .await
            .unwrap();
        let gateway_reads = Arc::new(AtomicUsize::new(0));

        let result = stable_runtime_context(
            || {
                let store = store.clone();
                let owner = owner.clone();
                async move {
                    store
                        .authorize_project(AuthorizeProjectInput::new(
                            owner,
                            "bootstrap-default",
                            Permission::AssetUse,
                        ))
                        .await
                }
            },
            |name| {
                let manager = manager.clone();
                let gateway_reads = Arc::clone(&gateway_reads);
                async move {
                    let read = gateway_reads.fetch_add(1, Ordering::SeqCst);
                    manager
                        .try_seed_config(GatewayConfig {
                            loadouts: vec![GatewayLoadoutConfig {
                                name,
                                description: Some(format!("revision-{read}")),
                                ..GatewayLoadoutConfig::default()
                            }],
                            ..GatewayConfig::default()
                        })
                        .await
                        .unwrap();
                    manager
                        .published_runtime_loadout_snapshot("production")
                        .await
                }
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(ProjectRuntimeLoadoutError::SnapshotUnstable)
        ));
        assert_eq!(gateway_reads.load(Ordering::SeqCst), CONTEXT_ATTEMPTS * 2);
    }

    #[tokio::test]
    async fn mutation_reauthorizes_after_validation() {
        let (_directory, runtime, owner) = fixture().await;
        let store = runtime.store().await.unwrap();
        let validator_store = store.clone();
        let result = assign_with_validator(&runtime, owner, "bootstrap-default".into(), "production".into(), move |_| async move {
            validator_store.execute_test_statement("UPDATE project_memberships SET status='suspended' WHERE project_id='bootstrap-default'").await.unwrap();
            Ok::<_, ()>(())
        }).await;

        assert!(matches!(
            result,
            Err(GatewayLoadoutAssignmentError::ProjectAccessUnavailable)
        ));
        assert_eq!(
            store.loadout_state_for_test().await.unwrap(),
            (0, 1, 0, 0, 3)
        );
    }
}
