//! MCP access-context lifecycle kernel and protected-HTTP shadow binding.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::PublishedProjectRouteSnapshot;
use labby_gateway::gateway::manager::GatewayManager;
use thiserror::Error;

use crate::access::{
    AccessRuntime, Permission, ProjectRuntimeMcpCatalogContext, project_runtime_mcp_catalog_context,
};
use crate::mcp::runtime::ProjectShadowSnapshotKey;
use crate::registry::RegisteredService;

const BIND_ATTEMPTS: usize = 3;
static NEXT_CONTEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct BoundAccessContextId(u64);

/// Immutable server-owned evidence for one MCP request/session lifecycle.
///
/// This type is deliberately non-`Clone`, non-`Debug`, and non-serializable.
/// Its inputs are server-derived authentication and protected-route facts; MCP
/// request params and `_meta` never participate. It is not a dispatch grant.
/// Resume/session validation remains deferred; the protected HTTP transport
/// wraps this core with the current access-token instance and expiry.
pub(crate) struct BoundAccessContext {
    id: BoundAccessContextId,
    catalog: ProjectRuntimeMcpCatalogContext,
    route: PublishedProjectRouteSnapshot,
    credential_binding_fingerprint: String,
    safe_fingerprint: String,
}

/// Request-owned protected HTTP binding around the coherent core evidence.
pub(crate) struct TransportBoundAccessContext {
    core: BoundAccessContext,
    credential_instance_fingerprint: String,
    expires_at_unix: u64,
}

pub(crate) struct TransportCredentialBinding {
    fingerprint: String,
    expires_at_unix: u64,
}

impl TransportBoundAccessContext {
    pub(crate) fn new(
        core: BoundAccessContext,
        credential: TransportCredentialBinding,
        now: SystemTime,
    ) -> Result<Self, BoundAccessContextError> {
        if unix_seconds(now)? >= credential.expires_at_unix {
            return Err(BoundAccessContextError::Unavailable);
        }
        Ok(Self {
            core,
            credential_instance_fingerprint: credential.fingerprint,
            expires_at_unix: credential.expires_at_unix,
        })
    }

    pub(crate) fn core(&self) -> &BoundAccessContext {
        &self.core
    }

    pub(crate) fn credential_instance_fingerprint(&self) -> &str {
        &self.credential_instance_fingerprint
    }

    pub(crate) fn matches_identity(&self, identity: &VerifiedIdentity) -> bool {
        self.core.credential_binding_fingerprint() == identity.safe_binding_fingerprint()
    }

    pub(crate) fn validate_not_expired(
        &self,
        now: SystemTime,
    ) -> Result<(), BoundAccessContextError> {
        if unix_seconds(now)? >= self.expires_at_unix {
            Err(BoundAccessContextError::Unavailable)
        } else {
            Ok(())
        }
    }
}

pub(crate) enum ProjectExecutionBinding<'a> {
    Legacy,
    Unavailable,
    Bound {
        transport: &'a TransportBoundAccessContext,
        identity: &'a VerifiedIdentity,
    },
}

pub(crate) fn project_execution_binding(
    extensions: &rmcp::model::Extensions,
    now: SystemTime,
) -> ProjectExecutionBinding<'_> {
    let Some(parts) = extensions.get::<axum::http::request::Parts>() else {
        return ProjectExecutionBinding::Legacy;
    };
    match parts.extensions.get::<ProjectAccessObservation>() {
        None => ProjectExecutionBinding::Legacy,
        Some(ProjectAccessObservation::Unavailable) => ProjectExecutionBinding::Unavailable,
        Some(ProjectAccessObservation::Bound(transport)) => {
            let Some(identity) = parts.extensions.get::<VerifiedIdentity>() else {
                return ProjectExecutionBinding::Unavailable;
            };
            if transport.validate_not_expired(now).is_err() || !transport.matches_identity(identity)
            {
                return ProjectExecutionBinding::Unavailable;
            }
            ProjectExecutionBinding::Bound {
                transport,
                identity,
            }
        }
    }
}

#[derive(Clone)]
pub(crate) enum ProjectAccessObservation {
    Bound(Arc<TransportBoundAccessContext>),
    Unavailable,
}

fn unix_seconds(now: SystemTime) -> Result<u64, BoundAccessContextError> {
    now.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| BoundAccessContextError::Unavailable)
}

pub(crate) fn validate_transport_credential_binding(
    issuer: &str,
    token_id: &str,
    expires_at_unix: usize,
    now: SystemTime,
) -> Result<TransportCredentialBinding, BoundAccessContextError> {
    let expires_at_unix =
        u64::try_from(expires_at_unix).map_err(|_| BoundAccessContextError::Unavailable)?;
    if !labby_auth::jwt::is_canonical_access_token_id(token_id)
        || unix_seconds(now)? >= expires_at_unix
    {
        return Err(BoundAccessContextError::Unavailable);
    }
    Ok(TransportCredentialBinding {
        fingerprint: labby_auth::util::fingerprint(&format!(
            "labby.mcp.transport-binding.v1\0{}:{}{}:{}",
            issuer.len(),
            issuer,
            token_id.len(),
            token_id
        )),
        expires_at_unix,
    })
}

pub(crate) fn validated_product_transport_binding(
    issuer: &str,
    credential_id: &str,
    credential_generation: u64,
    expires_at_unix: u64,
    now: SystemTime,
) -> Result<TransportCredentialBinding, BoundAccessContextError> {
    if issuer.is_empty()
        || credential_id.is_empty()
        || credential_generation == 0
        || unix_seconds(now)? >= expires_at_unix
    {
        return Err(BoundAccessContextError::Unavailable);
    }
    Ok(TransportCredentialBinding {
        fingerprint: labby_auth::util::fingerprint(&format!(
            "labby.mcp.product-transport-binding.v1\0{}:{}{}:{}:{credential_generation}",
            issuer.len(),
            issuer,
            credential_id.len(),
            credential_id
        )),
        expires_at_unix,
    })
}

#[allow(dead_code)]
impl BoundAccessContext {
    pub(crate) fn id(&self) -> BoundAccessContextId {
        self.id
    }

    pub(crate) fn catalog(&self) -> &ProjectRuntimeMcpCatalogContext {
        &self.catalog
    }

    pub(crate) fn route(&self) -> &PublishedProjectRouteSnapshot {
        &self.route
    }

    pub(crate) fn safe_fingerprint(&self) -> &str {
        &self.safe_fingerprint
    }

    pub(crate) fn credential_binding_fingerprint(&self) -> &str {
        &self.credential_binding_fingerprint
    }

    pub(crate) fn same_publication_as(&self, other: &Self) -> bool {
        self.credential_binding_fingerprint == other.credential_binding_fingerprint
            && self.catalog.same_publication_as(&other.catalog)
            && self.route.same_publication_as(&other.route)
    }

    pub(crate) fn allows_upstream_prompt_pair(&self, upstream: &str, native_name: &str) -> bool {
        let route = self.route();
        route.effective_loadout().expose_prompts
            && route
                .effective_loadout()
                .upstreams
                .iter()
                .any(|name| name == upstream)
            && self
                .catalog()
                .catalog()
                .prompts()
                .routes()
                .iter()
                .any(|candidate| {
                    candidate.upstream_name.as_ref() == upstream
                        && candidate.native_name.as_ref() == native_name
                })
    }

    pub(crate) fn allows_upstream_tool_pair(&self, upstream: &str, native_name: &str) -> bool {
        let route = self.route();
        route.effective_loadout().expose_tools
            && route
                .effective_loadout()
                .upstreams
                .iter()
                .any(|name| name == upstream)
            && self
                .catalog()
                .catalog()
                .tools()
                .routes()
                .iter()
                .any(|candidate| {
                    candidate.upstream_name.as_ref() == upstream
                        && candidate.tool_name.as_ref() == native_name
                })
    }

    pub(crate) fn allows_upstream_resource_pair(&self, upstream: &str, native_uri: &str) -> bool {
        let route = self.route();
        route.effective_loadout().expose_resources
            && route
                .effective_loadout()
                .upstreams
                .iter()
                .any(|name| name == upstream)
            && self
                .catalog()
                .catalog()
                .resources()
                .routes()
                .iter()
                .any(|candidate| {
                    candidate.upstream_name.as_ref() == upstream
                        && candidate.native_uri.as_ref() == native_uri
                })
    }
}

pub(crate) fn attach_project_access_observation(
    extensions: &mut axum::http::Extensions,
    binding: Result<TransportBoundAccessContext, BoundAccessContextError>,
) {
    let observation = match binding {
        Ok(binding) => ProjectAccessObservation::Bound(Arc::new(binding)),
        Err(_) => ProjectAccessObservation::Unavailable,
    };
    extensions.insert(observation);
}

pub(crate) fn project_access_observation_from_mcp_extensions(
    extensions: &rmcp::model::Extensions,
) -> Option<&ProjectAccessObservation> {
    extensions
        .get::<axum::http::request::Parts>()?
        .extensions
        .get::<ProjectAccessObservation>()
}

/// Non-enforcing Project discovery policy observed by discovery handlers.
///
/// `Legacy` means the request was not opted into Project binding. `Unavailable`
/// is an explicit opted-in failure (including expiry) and must never be treated
/// as legacy fallback. Only `Bound` can classify catalog-backed candidates.
pub(crate) enum ProjectDiscoveryShadow<'a> {
    Legacy,
    Unavailable,
    Bound(&'a TransportBoundAccessContext),
}

impl ProjectDiscoveryShadow<'_> {
    pub(crate) fn cursor_binding_fingerprint(&self, now: SystemTime) -> Option<String> {
        self.snapshot_key(now)
            .map(|key| key.tools_cursor_fingerprint())
    }

    pub(crate) fn snapshot_key(&self, now: SystemTime) -> Option<ProjectShadowSnapshotKey> {
        let Self::Bound(binding) = self else {
            return None;
        };
        binding.validate_not_expired(now).ok()?;
        let core = binding.core();
        let route = core.route();
        let catalog = core.catalog().catalog();
        Some(ProjectShadowSnapshotKey {
            credential_instance_fingerprint: binding.credential_instance_fingerprint().to_owned(),
            credential_binding_fingerprint: core.credential_binding_fingerprint().to_owned(),
            route_binding_fingerprint: labby_auth::util::fingerprint(&format!(
                "labby.mcp.project-shadow-route.v1\0{}\0{}\0{}",
                route.project_id(),
                route.route_name(),
                route.resource()
            )),
            access_global_revision: core.catalog().access().global_revision,
            runtime: catalog.tools().runtime_config_generation(),
            pool: catalog.tools().pool_publication_generation(),
            tools: catalog.tools().tool_catalog_generation(),
            resources: catalog.resources().resource_catalog_generation(),
            resource_templates: catalog
                .resource_templates()
                .resource_template_catalog_generation(),
            prompts: catalog.prompts().prompt_catalog_generation(),
            services: catalog.services().service_registry_generation(),
        })
    }
    pub(crate) fn state_label_at(&self, now: SystemTime) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Unavailable => "unavailable",
            Self::Bound(binding) if binding.validate_not_expired(now).is_ok() => "bound",
            Self::Bound(_) => "unavailable",
        }
    }

    /// `None` means shadow policy is unavailable/legacy, not allow or deny.
    pub(crate) fn allows_builtin_service(&self, service: &str, now: SystemTime) -> Option<bool> {
        let Self::Bound(binding) = self else {
            return None;
        };
        if binding.validate_not_expired(now).is_err() {
            return None;
        }
        let core = binding.core();
        let route = core.route();
        Some(
            route.effective_loadout().expose_tools
                && route
                    .effective_service_names()
                    .iter()
                    .any(|name| name.as_ref() == service)
                && core
                    .catalog()
                    .catalog()
                    .services()
                    .services()
                    .iter()
                    .any(|published| published.name() == service),
        )
    }

    pub(crate) fn allows_code_mode_tools(&self, now: SystemTime) -> Option<bool> {
        let Self::Bound(binding) = self else {
            return None;
        };
        binding.validate_not_expired(now).ok()?;
        Some(binding.core().route().effective_loadout().expose_code_mode)
    }

    pub(crate) fn allows_builtin_service_descriptor(
        &self,
        service: &RegisteredService,
        now: SystemTime,
    ) -> Option<bool> {
        let Self::Bound(binding) = self else {
            return None;
        };
        binding.validate_not_expired(now).ok()?;
        if self.allows_builtin_service(service.name, now) != Some(true) {
            return Some(false);
        }
        let published = binding
            .core()
            .catalog()
            .catalog()
            .services()
            .services()
            .iter()
            .find(|candidate| candidate.name() == service.name)?;
        Some(
            published.description() == service.description
                && published.actions().len() == service.actions.len()
                && published.actions().iter().all(|published| {
                    service.actions.iter().any(|current| {
                        published.name() == current.name
                            && published.description() == current.description
                            && published.destructive() == current.destructive
                            && published.requires_admin() == current.requires_admin
                    })
                }),
        )
    }

    /// Classify a built-in action resource using the route publication rather
    /// than the handler's potentially older route scope. `None` means shadow
    /// policy is unavailable/legacy, not allow or deny.
    pub(crate) fn allows_builtin_action_resource(
        &self,
        service: &str,
        now: SystemTime,
    ) -> Option<bool> {
        let Self::Bound(binding) = self else {
            return None;
        };
        if binding.validate_not_expired(now).is_err() {
            return None;
        }
        let route = binding.core().route();
        Some(
            route.effective_loadout().expose_resources
                && route
                    .effective_service_names()
                    .iter()
                    .any(|name| name.as_ref() == service),
        )
    }

    /// `None` means shadow policy is unavailable/legacy, not allow or deny.
    pub(crate) fn allows_upstream_tool(
        &self,
        upstream: &str,
        tool: &str,
        now: SystemTime,
    ) -> Option<bool> {
        let Self::Bound(binding) = self else {
            return None;
        };
        if binding.validate_not_expired(now).is_err() {
            return None;
        }
        Some(binding.core().allows_upstream_tool_pair(upstream, tool))
    }

    /// Classify one regular non-OAuth upstream Resource by exact provenance.
    /// `None` means the request is legacy/unavailable, not allow or deny.
    pub(crate) fn allows_upstream_resource(
        &self,
        upstream: &str,
        native_uri: &str,
        now: SystemTime,
    ) -> Option<bool> {
        let Self::Bound(binding) = self else {
            return None;
        };
        if binding.validate_not_expired(now).is_err() {
            return None;
        }
        Some(
            binding
                .core()
                .allows_upstream_resource_pair(upstream, native_uri),
        )
    }

    pub(crate) fn allows_upstream_resource_template(
        &self,
        upstream: &str,
        native_uri_template: &str,
        now: SystemTime,
    ) -> Option<bool> {
        let Self::Bound(binding) = self else {
            return None;
        };
        if binding.validate_not_expired(now).is_err() {
            return None;
        }
        let core = binding.core();
        let route = core.route();
        Some(
            route.effective_loadout().expose_resources
                && route
                    .effective_loadout()
                    .upstreams
                    .iter()
                    .any(|name| name == upstream)
                && core
                    .catalog()
                    .catalog()
                    .resource_templates()
                    .routes()
                    .iter()
                    .any(|candidate| {
                        candidate.upstream_name.as_ref() == upstream
                            && candidate.native_uri_template.as_ref() == native_uri_template
                    }),
        )
    }

    pub(crate) fn allows_upstream_prompt(
        &self,
        upstream: &str,
        native_name: &str,
        now: SystemTime,
    ) -> Option<bool> {
        let Self::Bound(binding) = self else {
            return None;
        };
        if binding.validate_not_expired(now).is_err() {
            return None;
        }
        Some(
            binding
                .core()
                .allows_upstream_prompt_pair(upstream, native_name),
        )
    }
}

pub(crate) fn project_discovery_shadow(
    extensions: &rmcp::model::Extensions,
    now: SystemTime,
) -> ProjectDiscoveryShadow<'_> {
    match project_access_observation_from_mcp_extensions(extensions) {
        None => ProjectDiscoveryShadow::Legacy,
        Some(ProjectAccessObservation::Unavailable) => ProjectDiscoveryShadow::Unavailable,
        Some(ProjectAccessObservation::Bound(binding)) => {
            if binding.validate_not_expired(now).is_ok() {
                ProjectDiscoveryShadow::Bound(binding)
            } else {
                ProjectDiscoveryShadow::Unavailable
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum BoundAccessContextError {
    #[error("MCP access context is unavailable")]
    Unavailable,
    #[error("MCP access context changed during observation")]
    Unstable,
}

pub(crate) async fn bind_access_context(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    identity: VerifiedIdentity,
    route_name: &str,
    resource: &str,
    project_id: &str,
) -> Result<BoundAccessContext, BoundAccessContextError> {
    bind_access_context_with_permission(
        runtime,
        manager,
        identity,
        route_name,
        resource,
        project_id,
        Permission::AssetDiscover,
    )
    .await
}

async fn bind_access_context_with_permission(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    identity: VerifiedIdentity,
    route_name: &str,
    resource: &str,
    project_id: &str,
    permission: Permission,
) -> Result<BoundAccessContext, BoundAccessContextError> {
    let context_identity = identity.clone();
    bind_stable_context(
        || async {
            project_runtime_mcp_catalog_context(
                runtime,
                manager,
                context_identity.clone(),
                project_id,
                permission,
            )
            .await
            .map_err(map_context_error)
        },
        |loadout_name| async move {
            manager
                .published_project_route_snapshot(route_name, project_id, &loadout_name)
                .await
                .map_err(map_route_error)
        },
        identity,
        route_name,
        resource,
    )
    .await
}

pub(crate) async fn bind_asset_use_access_context(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    identity: VerifiedIdentity,
    route_name: &str,
    resource: &str,
    project_id: &str,
) -> Result<BoundAccessContext, BoundAccessContextError> {
    bind_access_context_with_permission(
        runtime,
        manager,
        identity,
        route_name,
        resource,
        project_id,
        Permission::AssetUse,
    )
    .await
}

/// Three outer attempts cap construction at six stable Project-context reads
/// and six protected-route publications. Each child publication is itself
/// independently bounded; no client parameter or request metadata participates.
async fn bind_stable_context<CF, CFut, RF, RFut>(
    mut read_context: CF,
    mut read_route: RF,
    identity: VerifiedIdentity,
    expected_route_name: &str,
    expected_resource: &str,
) -> Result<BoundAccessContext, BoundAccessContextError>
where
    CF: FnMut() -> CFut,
    CFut: Future<Output = Result<ProjectRuntimeMcpCatalogContext, BoundAccessContextError>>,
    RF: FnMut(String) -> RFut,
    RFut: Future<Output = Result<PublishedProjectRouteSnapshot, BoundAccessContextError>>,
{
    let (second_context, second_route) = observe_coherent_pair(
        || read_context(),
        |loadout| read_route(loadout),
        |context| context.access().loadout_name.clone(),
        ProjectRuntimeMcpCatalogContext::same_publication_as,
        PublishedProjectRouteSnapshot::same_publication_as,
        |context, route| {
            context.catalog().tools().runtime_config_generation()
                == route.runtime_config_generation()
        },
    )
    .await?;
    let access = second_context.access();
    if access.project_id != second_route.project_id()
        || access.loadout_name != second_route.assigned_loadout_name()
        || second_route.route_name() != expected_route_name
        || second_route.resource() != expected_resource
    {
        return Err(BoundAccessContextError::Unavailable);
    }
    let id = NEXT_CONTEXT_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map(BoundAccessContextId)
        .map_err(|_| BoundAccessContextError::Unavailable)?;
    let credential_binding_fingerprint = identity.safe_binding_fingerprint();
    let safe_fingerprint = labby_auth::util::fingerprint(&format!(
        "{}\0{}\0{}\0{}\0{}",
        id.0,
        access.project_id,
        second_route.route_name(),
        second_route.resource(),
        credential_binding_fingerprint
    ));
    Ok(BoundAccessContext {
        id,
        catalog: second_context,
        route: second_route,
        credential_binding_fingerprint,
        safe_fingerprint,
    })
}

async fn observe_coherent_pair<C, R, CF, CFut, RF, RFut, KF, SC, SR, SG>(
    mut read_context: CF,
    mut read_route: RF,
    route_key: KF,
    same_context: SC,
    same_route: SR,
    same_generation: SG,
) -> Result<(C, R), BoundAccessContextError>
where
    CF: FnMut() -> CFut,
    CFut: Future<Output = Result<C, BoundAccessContextError>>,
    RF: FnMut(String) -> RFut,
    RFut: Future<Output = Result<R, BoundAccessContextError>>,
    KF: Fn(&C) -> String,
    SC: Fn(&C, &C) -> bool,
    SR: Fn(&R, &R) -> bool,
    SG: Fn(&C, &R) -> bool,
{
    for _ in 0..BIND_ATTEMPTS {
        let first_context = read_context().await?;
        let first_route = read_route(route_key(&first_context)).await?;
        let second_context = read_context().await?;
        let second_route = read_route(route_key(&second_context)).await?;
        if same_context(&first_context, &second_context)
            && same_route(&first_route, &second_route)
            && same_generation(&second_context, &second_route)
        {
            return Ok((second_context, second_route));
        }
    }
    Err(BoundAccessContextError::Unstable)
}

fn map_route_error(
    error: labby_gateway::gateway::ProjectRoutePublicationError,
) -> BoundAccessContextError {
    match error {
        labby_gateway::gateway::ProjectRoutePublicationError::Unavailable => {
            BoundAccessContextError::Unavailable
        }
        labby_gateway::gateway::ProjectRoutePublicationError::Unstable => {
            BoundAccessContextError::Unstable
        }
    }
}

fn map_context_error(
    error: crate::access::ProjectRuntimeMcpCatalogError,
) -> BoundAccessContextError {
    match error {
        crate::access::ProjectRuntimeMcpCatalogError::SnapshotUnstable => {
            BoundAccessContextError::Unstable
        }
        _ => BoundAccessContextError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "proxy-testkit")]
    use std::io;
    #[cfg(feature = "proxy-testkit")]
    use std::sync::Mutex;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    #[cfg(feature = "proxy-testkit")]
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    #[cfg(feature = "proxy-testkit")]
    struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

    #[cfg(feature = "proxy-testkit")]
    impl io::Write for CapturedLogWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[cfg(feature = "proxy-testkit")]
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = CapturedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CapturedLogWriter(Arc::clone(&self.0))
        }
    }

    #[cfg(feature = "proxy-testkit")]
    fn project_shadow_test_server() -> crate::mcp::server::LabMcpServer {
        crate::mcp::server::LabMcpServer {
            registry: Arc::new(crate::registry::build_default_registry()),
            access_runtime: Arc::new(AccessRuntime::blocked_unavailable()),
            file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            gateway_manager: None,
            peers: Default::default(),
            code_mode_app_state: Default::default(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(0)),
            route_scope: crate::mcp::route_scope::McpRouteScope::protected_subset(
                "project-route",
                std::iter::empty::<&str>(),
                ["fs", "setup"],
                false,
            ),
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    #[cfg(feature = "proxy-testkit")]
    fn project_shadow_context(
        peer: rmcp::service::Peer<rmcp::RoleServer>,
        observation: Option<ProjectAccessObservation>,
    ) -> rmcp::service::RequestContext<rmcp::RoleServer> {
        let mut context =
            rmcp::service::RequestContext::new(rmcp::model::NumberOrString::Number(1), peer);
        if let Some(observation) = observation {
            let mut parts = axum::http::Request::new(()).into_parts().0;
            parts.extensions.insert(observation);
            context.extensions.insert(parts);
        }
        context
    }

    #[cfg(feature = "proxy-testkit")]
    async fn list_tools_with_project_observation(
        observation: Option<ProjectAccessObservation>,
        identity: Option<VerifiedIdentity>,
    ) -> (rmcp::model::ListToolsResult, String) {
        use tracing::instrument::WithSubscriber as _;

        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, io::Error, _>(
            project_shadow_test_server(),
            transport,
            None,
        );
        let mut context = project_shadow_context(running.peer().clone(), observation);
        if let Some(identity) = identity {
            context
                .extensions
                .get_mut::<axum::http::request::Parts>()
                .expect("request parts")
                .extensions
                .insert(identity);
        }
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(logs.clone())
            .finish();
        let result = running
            .service()
            .list_tools_impl(None, context)
            .with_subscriber(tracing::Dispatch::new(subscriber))
            .await
            .expect("tools/list");
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        (result, logs)
    }

    #[cfg(feature = "proxy-testkit")]
    async fn list_resources_with_project_observation(
        observation: Option<ProjectAccessObservation>,
    ) -> (rmcp::model::ListResourcesResult, String) {
        use tracing::instrument::WithSubscriber as _;

        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, io::Error, _>(
            project_shadow_test_server(),
            transport,
            None,
        );
        let context = project_shadow_context(running.peer().clone(), observation);
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(logs.clone())
            .finish();
        let result = running
            .service()
            .list_resources_impl(None, context)
            .with_subscriber(tracing::Dispatch::new(subscriber))
            .await
            .expect("resources/list");
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        (result, logs)
    }

    #[test]
    fn transport_binding_fingerprint_is_token_specific_redacted_and_expiring() {
        let now = UNIX_EPOCH + std::time::Duration::from_secs(100);
        let first =
            validate_transport_credential_binding("issuer-secret", "jti-secret-a", 101, now)
                .expect("live token");
        let second =
            validate_transport_credential_binding("issuer-secret", "jti-secret-b", 101, now)
                .expect("distinct live token");
        assert_ne!(first.fingerprint, second.fingerprint);
        assert!(!first.fingerprint.contains("issuer-secret"));
        assert!(!first.fingerprint.contains("jti-secret-a"));
        assert_eq!(
            validate_transport_credential_binding("issuer", "jti", 100, now).err(),
            Some(BoundAccessContextError::Unavailable)
        );
        for invalid in ["", " padded", &"x".repeat(257)] {
            assert!(validate_transport_credential_binding("issuer", invalid, 101, now).is_err());
        }
    }

    #[test]
    fn product_transport_binding_is_generation_specific_redacted_and_expiring() {
        let now = UNIX_EPOCH + std::time::Duration::from_secs(100);
        let first = validated_product_transport_binding(
            "product-issuer-secret",
            "credential-secret-id",
            1,
            101,
            now,
        )
        .unwrap();
        let rotated = validated_product_transport_binding(
            "product-issuer-secret",
            "credential-secret-id",
            2,
            101,
            now,
        )
        .unwrap();
        assert_ne!(first.fingerprint, rotated.fingerprint);
        assert!(!first.fingerprint.contains("product-issuer-secret"));
        assert!(!first.fingerprint.contains("credential-secret-id"));
        assert!(validated_product_transport_binding("issuer", "credential", 1, 100, now).is_err());
        assert!(validated_product_transport_binding("issuer", "credential", 0, 101, now).is_err());
    }

    #[derive(Clone, Copy)]
    struct Observation {
        publication: usize,
        runtime: usize,
    }

    #[tokio::test]
    async fn coherent_pair_retries_cross_generation_then_binds_with_exact_counts() {
        let contexts = Arc::new(AtomicUsize::new(0));
        let routes = Arc::new(AtomicUsize::new(0));
        let context_reads = Arc::clone(&contexts);
        let route_reads = Arc::clone(&routes);
        let result = observe_coherent_pair(
            move || {
                let call = context_reads.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(Observation {
                        publication: call / 2,
                        runtime: usize::from(call >= 2),
                    })
                }
            },
            move |_| {
                let call = route_reads.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(Observation {
                        publication: call / 2,
                        runtime: 1,
                    })
                }
            },
            |_| "production".to_string(),
            |first, second| first.publication == second.publication,
            |first, second| first.publication == second.publication,
            |context, route| context.runtime == route.runtime,
        )
        .await
        .expect("second attempt converges");
        assert_eq!(result.0.runtime, 1);
        assert_eq!(contexts.load(Ordering::SeqCst), 4);
        assert_eq!(routes.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn coherent_pair_sustained_cross_generation_stops_at_bound() {
        let contexts = Arc::new(AtomicUsize::new(0));
        let routes = Arc::new(AtomicUsize::new(0));
        let context_reads = Arc::clone(&contexts);
        let route_reads = Arc::clone(&routes);
        let result = observe_coherent_pair(
            move || {
                let call = context_reads.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(Observation {
                        publication: call / 2,
                        runtime: 0,
                    })
                }
            },
            move |_| {
                let call = route_reads.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(Observation {
                        publication: call / 2,
                        runtime: 1,
                    })
                }
            },
            |_| "production".to_string(),
            |first, second| first.publication == second.publication,
            |first, second| first.publication == second.publication,
            |context, route| context.runtime == route.runtime,
        )
        .await;
        assert_eq!(result.err(), Some(BoundAccessContextError::Unstable));
        assert_eq!(contexts.load(Ordering::SeqCst), 6);
        assert_eq!(routes.load(Ordering::SeqCst), 6);
    }

    #[tokio::test]
    async fn coherent_pair_context_failure_precedes_route_read() {
        let routes = Arc::new(AtomicUsize::new(0));
        let route_reads = Arc::clone(&routes);
        let result = observe_coherent_pair::<Observation, Observation, _, _, _, _, _, _, _, _>(
            || async { Err(BoundAccessContextError::Unavailable) },
            move |_| {
                route_reads.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(Observation {
                        publication: 0,
                        runtime: 0,
                    })
                }
            },
            |_| "production".to_string(),
            |first, second| first.publication == second.publication,
            |first, second| first.publication == second.publication,
            |context, route| context.runtime == route.runtime,
        )
        .await;
        assert_eq!(result.err(), Some(BoundAccessContextError::Unavailable));
        assert_eq!(routes.load(Ordering::SeqCst), 0);
    }

    #[cfg(feature = "proxy-testkit")]
    #[tokio::test]
    async fn real_binding_owns_exact_facts_and_remains_immutable() {
        use labby_auth::Authenticator;
        use labby_gateway::gateway::config_store::FsGatewayConfigStore;
        use labby_gateway::gateway::manager::GatewayRuntimeHandle;
        use labby_gateway::upstream::pool::UpstreamPool;
        use labby_runtime::gateway_config::{
            GatewayConfig, GatewayLoadoutConfig, ProtectedGatewaySubsetTarget,
            ProtectedMcpRouteConfig, ProtectedMcpRouteTarget, UpstreamConfig, VirtualServerConfig,
            VirtualServerSurfacesConfig,
        };
        use rmcp::model::Prompt;

        use crate::access::{AssignProjectLoadoutInput, BootstrapOwnerInput};

        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let runtime = AccessRuntime::initialize(directory.path().join("access.db")).await;
        let identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::StaticBearer,
            "server-static-issuer",
            "server-credential",
        )
        .unwrap();
        runtime
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        runtime
            .store()
            .await
            .unwrap()
            .assign_project_loadout(
                AssignProjectLoadoutInput::new(identity.clone(), "bootstrap-default", "production")
                    .unwrap(),
            )
            .await
            .unwrap();

        let gateway_runtime = GatewayRuntimeHandle::default();
        let pool = Arc::new(UpstreamPool::new());
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("deploy", Some("prompt metadata"), None)],
        )
        .await;
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        let gateway_path = directory.path().join("bound-context.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            gateway_runtime,
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        )
        .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry()));
        let config = || GatewayConfig {
            upstream: vec![UpstreamConfig {
                enabled: true,
                name: "alpha".into(),
                url: None,
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: None,
                command: Some("node".into()),
                args: Vec::new(),
                env: Default::default(),
                proxy_resources: false,
                proxy_prompts: true,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                proxy_skills: false,
                expose_skills: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            }],
            loadouts: vec![GatewayLoadoutConfig {
                name: "production".into(),
                upstreams: vec!["alpha".into()],
                services: vec!["fs-primary".into()],
                ..Default::default()
            }],
            virtual_servers: vec![VirtualServerConfig {
                id: "fs-primary".into(),
                service: "fs".into(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    mcp: true,
                    ..Default::default()
                },
                mcp_policy: None,
            }],
            protected_mcp_routes: vec![ProtectedMcpRouteConfig {
                name: "project-route".into(),
                enabled: true,
                public_host: "MCP.Example.com.".into(),
                public_path: "/project".into(),
                upstream: None,
                backend_url: String::new(),
                backend_mcp_path: "/mcp".into(),
                scopes: vec![],
                health_path: None,
                target: Some(ProtectedMcpRouteTarget::GatewaySubset(
                    ProtectedGatewaySubsetTarget {
                        project_id: Some("bootstrap-default".into()),
                        loadout: Some("production".into()),
                        ..Default::default()
                    },
                )),
            }],
            ..Default::default()
        };
        manager.try_seed_config(config()).await.unwrap();

        let first = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .expect("first binding");
        let second = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .expect("second binding");

        assert_eq!(
            first.catalog().access().permission,
            Permission::AssetDiscover
        );
        assert_eq!(first.catalog().access().project_id, "bootstrap-default");
        assert_eq!(first.route().route_name(), "project-route");
        assert_eq!(first.route().resource(), "https://mcp.example.com/project");
        assert!(
            first
                .catalog()
                .catalog()
                .resource_templates()
                .routes()
                .is_empty()
        );
        assert_eq!(
            first.catalog().catalog().prompts().routes()[0]
                .prompt
                .description
                .as_deref(),
            Some("prompt metadata")
        );
        assert!(first.id() != second.id());
        assert_ne!(first.safe_fingerprint(), second.safe_fingerprint());
        assert_eq!(
            first.credential_binding_fingerprint(),
            identity.safe_binding_fingerprint()
        );

        let now = UNIX_EPOCH + std::time::Duration::from_secs(100);
        let credential = validate_transport_credential_binding("issuer", "request-jti", 101, now)
            .expect("transport credential");
        let transport = TransportBoundAccessContext::new(second, credential, now)
            .expect("still live at attachment");
        let request = {
            let mut request = axum::http::Request::new(());
            attach_project_access_observation(request.extensions_mut(), Ok(transport));
            request
        };
        let (parts, _) = request.into_parts();
        let mut extensions = rmcp::model::Extensions::new();
        extensions.insert(parts);
        let ProjectAccessObservation::Bound(observed) =
            project_access_observation_from_mcp_extensions(&extensions)
                .expect("bound observation crosses HTTP Parts")
        else {
            panic!("expected bound observation");
        };
        assert_eq!(
            observed.credential_instance_fingerprint(),
            labby_auth::util::fingerprint(concat!(
                "labby.mcp.transport-binding.v1\0",
                "6:issuer11:request-jti"
            ))
        );
        let shadow = ProjectDiscoveryShadow::Bound(observed.as_ref());
        let key = shadow
            .snapshot_key(now)
            .expect("template-bound snapshot key");
        assert_eq!(
            key.resource_templates,
            observed
                .core()
                .catalog()
                .catalog()
                .resource_templates()
                .resource_template_catalog_generation()
        );
        let shadow = project_discovery_shadow(&extensions, now);
        assert_eq!(shadow.state_label_at(now), "bound");
        let snapshot_key = shadow.snapshot_key(now).expect("stable shadow key");
        let same_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .expect("same publication binding");
        let same_credential =
            validate_transport_credential_binding("issuer", "request-jti", 101, now).unwrap();
        let same_transport =
            TransportBoundAccessContext::new(same_core, same_credential, now).unwrap();
        assert!(
            snapshot_key
                == ProjectDiscoveryShadow::Bound(&same_transport)
                    .snapshot_key(now)
                    .unwrap(),
            "fresh context ids must not perturb the cursor shadow key"
        );
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("deploy-v2", Some("changed"), None)],
        )
        .await;
        let changed_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .expect("prompt-only publication binding");
        assert_eq!(
            changed_core.catalog().catalog().prompts().routes()[0]
                .native_name
                .as_ref(),
            "deploy-v2"
        );
        let changed_transport = TransportBoundAccessContext::new(
            changed_core,
            validate_transport_credential_binding("issuer", "request-jti", 101, now).unwrap(),
            now,
        )
        .unwrap();
        let changed_key = ProjectDiscoveryShadow::Bound(&changed_transport)
            .snapshot_key(now)
            .unwrap();
        assert_eq!(snapshot_key.runtime, changed_key.runtime);
        assert_eq!(snapshot_key.pool, changed_key.pool);
        assert_eq!(snapshot_key.tools, changed_key.tools);
        assert_eq!(snapshot_key.resources, changed_key.resources);
        assert_eq!(
            snapshot_key.resource_templates,
            changed_key.resource_templates
        );
        assert_eq!(snapshot_key.services, changed_key.services);
        assert_ne!(snapshot_key.prompts, changed_key.prompts);
        assert_ne!(snapshot_key, changed_key);
        let other_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .unwrap();
        let other_credential =
            validate_transport_credential_binding("issuer", "other-request-jti", 101, now).unwrap();
        let other_transport =
            TransportBoundAccessContext::new(other_core, other_credential, now).unwrap();
        assert!(
            snapshot_key
                != ProjectDiscoveryShadow::Bound(&other_transport)
                    .snapshot_key(now)
                    .unwrap(),
            "a different credential instance must not reuse cursor shadow telemetry"
        );
        let cursor_now = SystemTime::now();
        let cursor_expiry = usize::try_from(unix_seconds(cursor_now).unwrap() + 3_600).unwrap();
        let cursor_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .unwrap();
        let cursor_transport = TransportBoundAccessContext::new(
            cursor_core,
            validate_transport_credential_binding(
                "issuer",
                "cursor-request-jti",
                cursor_expiry,
                cursor_now,
            )
            .unwrap(),
            cursor_now,
        )
        .unwrap();
        let cursor_key = ProjectDiscoveryShadow::Bound(&cursor_transport)
            .snapshot_key(cursor_now)
            .unwrap();
        let cursor_other_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .unwrap();
        let cursor_other_transport = TransportBoundAccessContext::new(
            cursor_other_core,
            validate_transport_credential_binding(
                "issuer",
                "cursor-other-jti",
                cursor_expiry,
                cursor_now,
            )
            .unwrap(),
            cursor_now,
        )
        .unwrap();
        let snapshot_server = project_shadow_test_server();
        let snapshot_items = (0..101)
            .map(|index| rmcp::model::Resource::new(format!("file:///{index}"), index.to_string()))
            .collect::<Vec<_>>();
        snapshot_server
            .route_runtime
            .store_resource_snapshot(
                crate::mcp::runtime::catalog_snapshot_audience(None),
                "wave31".into(),
                Arc::from(snapshot_items),
                Arc::from([crate::mcp::runtime::ResourceProvenance {
                    upstream: "alpha".into(),
                    native_uri: "file:///100".into(),
                }]),
                Some(cursor_key),
            )
            .await;
        let (snapshot_transport, _snapshot_client) = tokio::io::duplex(64 * 1024);
        let snapshot_running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, io::Error, _>(
            snapshot_server,
            snapshot_transport,
            None,
        );
        let cursor = rmcp::model::PaginatedRequestParams::default()
            .with_cursor(Some("v1:100:wave31".into()));
        use tracing::instrument::WithSubscriber as _;
        let matching_logs = CapturedLogs::default();
        let matching_subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(matching_logs.clone())
            .finish();
        let matching = snapshot_running
            .service()
            .list_resources_impl(
                Some(cursor.clone()),
                project_shadow_context(
                    snapshot_running.peer().clone(),
                    Some(ProjectAccessObservation::Bound(Arc::new(cursor_transport))),
                ),
            )
            .with_subscriber(tracing::Dispatch::new(matching_subscriber))
            .await
            .unwrap();
        let matching_logs = String::from_utf8(matching_logs.0.lock().unwrap().clone()).unwrap();
        let mismatched_logs = CapturedLogs::default();
        let mismatched_subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(mismatched_logs.clone())
            .finish();
        let mismatched = snapshot_running
            .service()
            .list_resources_impl(
                Some(cursor),
                project_shadow_context(
                    snapshot_running.peer().clone(),
                    Some(ProjectAccessObservation::Bound(Arc::new(
                        cursor_other_transport,
                    ))),
                ),
            )
            .with_subscriber(tracing::Dispatch::new(mismatched_subscriber))
            .await
            .unwrap();
        let mismatched_logs = String::from_utf8(mismatched_logs.0.lock().unwrap().clone()).unwrap();
        assert_eq!(
            serde_json::to_vec(&matching).unwrap(),
            serde_json::to_vec(&mismatched).unwrap(),
            "cursor shadow-key mismatch must not alter the retained page"
        );
        assert!(matching_logs.contains("project_shadow_state=\"bound\""));
        assert!(matching_logs.contains("project_shadow_checked_resource_count=1"));
        assert!(matching_logs.contains("project_shadow_would_suppress_resource_count=1"));
        assert!(mismatched_logs.contains("project_shadow_state=\"unavailable\""));
        assert!(mismatched_logs.contains("project_shadow_checked_resource_count=0"));
        assert!(mismatched_logs.contains("project_shadow_would_suppress_resource_count=0"));

        let template_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .unwrap();
        let template_transport = TransportBoundAccessContext::new(
            template_core,
            validate_transport_credential_binding(
                "issuer",
                "cursor-request-jti",
                cursor_expiry,
                cursor_now,
            )
            .unwrap(),
            cursor_now,
        )
        .unwrap();
        let template_key = ProjectDiscoveryShadow::Bound(&template_transport)
            .snapshot_key(cursor_now)
            .unwrap();
        let template_other_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .unwrap();
        let template_other_transport = TransportBoundAccessContext::new(
            template_other_core,
            validate_transport_credential_binding(
                "issuer",
                "cursor-other-jti",
                cursor_expiry,
                cursor_now,
            )
            .unwrap(),
            cursor_now,
        )
        .unwrap();
        let template_items = (0..101)
            .map(|index| {
                rmcp::model::ResourceTemplate::new(
                    format!("file:///{index}/{{id}}"),
                    index.to_string(),
                )
            })
            .collect::<Vec<_>>();
        snapshot_running
            .service()
            .route_runtime
            .store_resource_template_snapshot(
                crate::mcp::runtime::catalog_snapshot_audience(None),
                "wave36".into(),
                Arc::from(template_items),
                Arc::from([crate::mcp::runtime::ResourceTemplateProvenance {
                    upstream: "alpha".into(),
                    native_uri_template: "file:///100/{id}".into(),
                }]),
                Some(template_key),
            )
            .await;
        let template_cursor = rmcp::model::PaginatedRequestParams::default()
            .with_cursor(Some("v1:100:wave36".into()));
        let template_matching_logs = CapturedLogs::default();
        let template_matching_subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(template_matching_logs.clone())
            .finish();
        let template_matching = snapshot_running
            .service()
            .list_resource_templates_impl(
                Some(template_cursor.clone()),
                project_shadow_context(
                    snapshot_running.peer().clone(),
                    Some(ProjectAccessObservation::Bound(Arc::new(
                        template_transport,
                    ))),
                ),
            )
            .with_subscriber(tracing::Dispatch::new(template_matching_subscriber))
            .await
            .unwrap();
        let template_matching_logs =
            String::from_utf8(template_matching_logs.0.lock().unwrap().clone()).unwrap();
        let template_mismatched_logs = CapturedLogs::default();
        let template_mismatched_subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(template_mismatched_logs.clone())
            .finish();
        let template_mismatched = snapshot_running
            .service()
            .list_resource_templates_impl(
                Some(template_cursor),
                project_shadow_context(
                    snapshot_running.peer().clone(),
                    Some(ProjectAccessObservation::Bound(Arc::new(
                        template_other_transport,
                    ))),
                ),
            )
            .with_subscriber(tracing::Dispatch::new(template_mismatched_subscriber))
            .await
            .unwrap();
        let template_mismatched_logs =
            String::from_utf8(template_mismatched_logs.0.lock().unwrap().clone()).unwrap();
        assert_eq!(
            serde_json::to_vec(&template_matching).unwrap(),
            serde_json::to_vec(&template_mismatched).unwrap()
        );
        assert!(template_matching_logs.contains("project_shadow_state=\"bound\""));
        assert!(template_matching_logs.contains("project_shadow_checked_template_count=1"));
        assert!(template_matching_logs.contains("project_shadow_would_suppress_template_count=1"));
        assert!(template_mismatched_logs.contains("project_shadow_state=\"unavailable\""));
        assert!(template_mismatched_logs.contains("project_shadow_checked_template_count=0"));
        assert!(
            template_mismatched_logs.contains("project_shadow_would_suppress_template_count=0")
        );
        let prompt_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .unwrap();
        let prompt_transport = TransportBoundAccessContext::new(
            prompt_core,
            validate_transport_credential_binding(
                "issuer",
                "prompt-cursor-jti",
                cursor_expiry,
                cursor_now,
            )
            .unwrap(),
            cursor_now,
        )
        .unwrap();
        let prompt_key = ProjectDiscoveryShadow::Bound(&prompt_transport)
            .snapshot_key(cursor_now)
            .unwrap();
        let prompt_other_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .unwrap();
        let prompt_other_transport = TransportBoundAccessContext::new(
            prompt_other_core,
            validate_transport_credential_binding(
                "issuer",
                "prompt-other-jti",
                cursor_expiry,
                cursor_now,
            )
            .unwrap(),
            cursor_now,
        )
        .unwrap();
        let prompt_items = (0..101)
            .map(|index| {
                let name = if index == 100 {
                    "alpha/deploy-v2".to_string()
                } else {
                    index.to_string()
                };
                Prompt::new(name, None::<String>, None)
            })
            .collect::<Vec<_>>();
        snapshot_running
            .service()
            .route_runtime
            .store_prompt_snapshot(
                crate::mcp::runtime::catalog_snapshot_audience(None),
                "wave41".into(),
                Arc::from(prompt_items),
                Arc::from([crate::mcp::runtime::PromptProvenance {
                    upstream: "alpha".into(),
                    native_name: "deploy-v2".into(),
                }]),
                Some(prompt_key),
            )
            .await;
        let prompt_cursor = rmcp::model::PaginatedRequestParams::default()
            .with_cursor(Some("v1:100:wave41".into()));
        let prompt_matching_logs = CapturedLogs::default();
        let prompt_matching_subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(prompt_matching_logs.clone())
            .finish();
        let prompt_matching = snapshot_running
            .service()
            .list_prompts_impl(
                Some(prompt_cursor.clone()),
                project_shadow_context(
                    snapshot_running.peer().clone(),
                    Some(ProjectAccessObservation::Bound(Arc::new(prompt_transport))),
                ),
            )
            .with_subscriber(tracing::Dispatch::new(prompt_matching_subscriber))
            .await
            .unwrap();
        let prompt_matching_logs =
            String::from_utf8(prompt_matching_logs.0.lock().unwrap().clone()).unwrap();
        let prompt_mismatched_logs = CapturedLogs::default();
        let prompt_mismatched_subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(prompt_mismatched_logs.clone())
            .finish();
        let prompt_mismatched = snapshot_running
            .service()
            .list_prompts_impl(
                Some(prompt_cursor),
                project_shadow_context(
                    snapshot_running.peer().clone(),
                    Some(ProjectAccessObservation::Bound(Arc::new(
                        prompt_other_transport,
                    ))),
                ),
            )
            .with_subscriber(tracing::Dispatch::new(prompt_mismatched_subscriber))
            .await
            .unwrap();
        let prompt_mismatched_logs =
            String::from_utf8(prompt_mismatched_logs.0.lock().unwrap().clone()).unwrap();
        assert_eq!(
            serde_json::to_vec(&prompt_matching).unwrap(),
            serde_json::to_vec(&prompt_mismatched).unwrap()
        );
        assert!(prompt_matching_logs.contains("project_shadow_state=\"bound\""));
        assert!(prompt_matching_logs.contains("project_shadow_checked_prompt_count=1"));
        assert!(prompt_matching_logs.contains("project_shadow_would_suppress_prompt_count=0"));
        assert!(prompt_mismatched_logs.contains("project_shadow_state=\"unavailable\""));
        assert!(prompt_mismatched_logs.contains("project_shadow_checked_prompt_count=0"));
        assert!(prompt_mismatched_logs.contains("project_shadow_would_suppress_prompt_count=0"));
        #[cfg(feature = "fs")]
        assert_eq!(
            shadow.allows_builtin_service("fs", now),
            Some(true),
            "route={:?} catalog={:?}",
            observed.core().route().effective_service_names(),
            observed
                .core()
                .catalog()
                .catalog()
                .services()
                .services()
                .iter()
                .map(|service| service.name())
                .collect::<Vec<_>>()
        );
        #[cfg(not(feature = "fs"))]
        assert_eq!(shadow.allows_builtin_service("fs", now), Some(false));
        assert_eq!(shadow.allows_builtin_service("setup", now), Some(false));
        #[cfg(feature = "fs")]
        {
            let registry = crate::registry::build_default_registry();
            let fs_service = registry.service("fs").expect("fs registry service");
            assert_eq!(
                shadow.allows_builtin_service_descriptor(fs_service, now),
                Some(true)
            );
            let changed_fs = RegisteredService {
                name: fs_service.name,
                description: "description changed after publication",
                category: fs_service.category,
                kind: fs_service.kind,
                status: fs_service.status,
                actions: fs_service.actions,
                dispatch: fs_service.dispatch,
            };
            assert_eq!(
                shadow.allows_builtin_service_descriptor(&changed_fs, now),
                Some(false),
                "live service description must not drift from the immutable Bound descriptor"
            );
            let service_with_actions =
                |actions: &'static [labby_primitives::action::ActionSpec]| RegisteredService {
                    description: fs_service.description,
                    actions,
                    ..changed_fs
                };
            let mut reordered = fs_service.actions.to_vec();
            reordered.reverse();
            let reordered = Box::leak(reordered.into_boxed_slice());
            assert_eq!(
                shadow.allows_builtin_service_descriptor(&service_with_actions(reordered), now),
                Some(true),
                "canonical action order must not depend on live registry insertion order"
            );
            let mutate_first = |mutate: fn(&mut labby_primitives::action::ActionSpec)| {
                let mut actions = fs_service.actions.to_vec();
                mutate(&mut actions[0]);
                let leaked: &'static mut [labby_primitives::action::ActionSpec] =
                    Box::leak(actions.into_boxed_slice());
                &*leaked
            };
            let changed_description = mutate_first(|action| action.description = "changed");
            let changed_destructive =
                mutate_first(|action| action.destructive = !action.destructive);
            let changed_admin =
                mutate_first(|action| action.requires_admin = !action.requires_admin);
            for (label, actions) in [
                ("description", changed_description),
                ("destructive", changed_destructive),
                ("requires_admin", changed_admin),
            ] {
                assert_eq!(
                    shadow.allows_builtin_service_descriptor(&service_with_actions(actions), now),
                    Some(false),
                    "same-name/same-count {label} drift must be rejected"
                );
            }
            let mut changed_name = fs_service.actions.to_vec();
            changed_name[0].name = "changed.action";
            let changed_actions = service_with_actions(Box::leak(changed_name.into_boxed_slice()));
            assert_eq!(
                shadow.allows_builtin_service_descriptor(&changed_actions, now),
                Some(false),
                "live action metadata must not widen the immutable Bound service descriptor"
            );
        }
        assert_eq!(
            shadow.allows_upstream_tool("unpublished", "missing", now),
            Some(false)
        );
        assert_eq!(
            shadow.state_label_at(UNIX_EPOCH + std::time::Duration::from_secs(101)),
            "unavailable"
        );
        let live_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .expect("live shadow core");
        let live_now = SystemTime::now();
        let live_expiry = usize::try_from(unix_seconds(live_now).unwrap() + 3_600).unwrap();
        let live_credential =
            validate_transport_credential_binding("issuer", "live-jti", live_expiry, live_now)
                .expect("live credential");
        let bound_observation = ProjectAccessObservation::Bound(Arc::new(
            TransportBoundAccessContext::new(live_core, live_credential, live_now)
                .expect("live transport"),
        ));
        let (legacy_result, _) = list_tools_with_project_observation(None, None).await;
        let (unavailable_result, _) =
            list_tools_with_project_observation(Some(ProjectAccessObservation::Unavailable), None)
                .await;
        let (bound_result, bound_logs) = list_tools_with_project_observation(
            Some(bound_observation.clone()),
            Some(identity.clone()),
        )
        .await;
        assert!(unavailable_result.tools.is_empty());
        assert!(unavailable_result.next_cursor.is_none());
        assert_ne!(
            serde_json::to_value(&legacy_result).unwrap(),
            serde_json::to_value(&bound_result).unwrap(),
            "Bound listing must enforce its published Project catalog"
        );
        assert!(
            legacy_result
                .tools
                .iter()
                .any(|tool| tool.name.as_ref() == "setup"),
            "the unchanged response must retain a service absent from the Bound catalog"
        );
        assert!(
            !bound_result
                .tools
                .iter()
                .any(|tool| tool.name.as_ref() == "setup"),
            "a service absent from the Bound catalog must be suppressed"
        );
        assert!(bound_logs.contains("project_shadow_state=\"bound\""));
        assert!(bound_logs.contains("project_shadow_would_suppress_tool_count=1"));
        for secret in [
            "bootstrap-default",
            "project-route",
            "live-jti",
            "server-credential",
            "fs-primary",
        ] {
            assert!(!bound_logs.contains(secret), "shadow log leaked {secret}");
        }

        let (legacy_resources, _) = list_resources_with_project_observation(None).await;
        let (unavailable_resources, _) =
            list_resources_with_project_observation(Some(ProjectAccessObservation::Unavailable))
                .await;
        let (bound_resources, bound_resource_logs) =
            list_resources_with_project_observation(Some(bound_observation)).await;
        assert_eq!(
            serde_json::to_vec(&legacy_resources).unwrap(),
            serde_json::to_vec(&unavailable_resources).unwrap(),
            "explicit shadow unavailability must not filter resources/list"
        );
        assert_eq!(
            serde_json::to_vec(&legacy_resources).unwrap(),
            serde_json::to_vec(&bound_resources).unwrap(),
            "Bound shadow differences must not filter resources/list"
        );
        assert!(
            legacy_resources
                .resources
                .iter()
                .any(|resource| { resource.uri == "lab://setup/actions" })
        );
        assert!(bound_resource_logs.contains("project_shadow_state=\"bound\""));
        assert!(bound_resource_logs.contains("project_shadow_would_suppress_resource_count=1"));
        for secret in [
            "bootstrap-default",
            "project-route",
            "live-jti",
            "server-credential",
            "fs-primary",
        ] {
            assert!(
                !bound_resource_logs.contains(secret),
                "resource shadow log leaked {secret}"
            );
        }

        let mut unavailable_request = axum::http::Request::new(());
        attach_project_access_observation(
            unavailable_request.extensions_mut(),
            Err(BoundAccessContextError::Unavailable),
        );
        assert!(matches!(
            unavailable_request
                .extensions()
                .get::<ProjectAccessObservation>(),
            Some(ProjectAccessObservation::Unavailable)
        ));
        assert!(
            axum::http::Request::new(())
                .extensions()
                .get::<ProjectAccessObservation>()
                .is_none()
        );
        assert_eq!(
            project_discovery_shadow(&rmcp::model::Extensions::new(), now).state_label_at(now),
            "legacy"
        );

        let expiring_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .expect("expiring core");
        let expiring = validate_transport_credential_binding("issuer", "expiring-jti", 101, now)
            .expect("valid at preflight");
        assert!(matches!(
            TransportBoundAccessContext::new(
                expiring_core,
                expiring,
                UNIX_EPOCH + std::time::Duration::from_secs(101),
            ),
            Err(BoundAccessContextError::Unavailable)
        ));

        manager.try_seed_config(config()).await.unwrap();
        assert_eq!(first.route().resource(), "https://mcp.example.com/project");
        let mismatch = bind_access_context(
            &runtime,
            &manager,
            identity,
            "project-route",
            "https://wrong.example/project",
            "bootstrap-default",
        )
        .await
        .err()
        .expect("stable mismatch");
        assert_eq!(mismatch, BoundAccessContextError::Unavailable);
        assert_eq!(mismatch.to_string(), "MCP access context is unavailable");
    }

    #[cfg(feature = "proxy-testkit")]
    #[tokio::test]
    async fn project_bound_tools_cursor_requires_the_exact_credential_snapshot() {
        use labby_auth::Authenticator;
        use labby_gateway::gateway::config_store::FsGatewayConfigStore;
        use labby_gateway::gateway::manager::GatewayRuntimeHandle;
        use labby_runtime::gateway_config::{
            GatewayConfig, GatewayLoadoutConfig, ProtectedGatewaySubsetTarget,
            ProtectedMcpRouteConfig, ProtectedMcpRouteTarget, VirtualServerConfig,
            VirtualServerSurfacesConfig,
        };
        use rmcp::model::PaginatedRequestParams;

        use crate::access::{AssignProjectLoadoutInput, BootstrapOwnerInput};
        use crate::registry::{RegisteredService, RegisteredServiceKind, ToolRegistry};

        const ACTIONS: &[labby_primitives::action::ActionSpec] =
            &[labby_primitives::action::ActionSpec {
                name: "status.get",
                description: "Get status",
                destructive: false,
                requires_admin: false,
                params: &[],
                returns: "object",
            }];

        fn dispatch(
            _action: String,
            _params: serde_json::Value,
        ) -> std::pin::Pin<
            Box<
                dyn Future<Output = Result<serde_json::Value, crate::dispatch::error::ToolError>>
                    + Send,
            >,
        > {
            Box::pin(async { Ok(serde_json::Value::Null) })
        }

        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let runtime = Arc::new(AccessRuntime::initialize(directory.path().join("access.db")).await);
        let identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::StaticBearer,
            "server-static-issuer",
            "server-credential",
        )
        .unwrap();
        runtime
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        runtime
            .store()
            .await
            .unwrap()
            .assign_project_loadout(
                AssignProjectLoadoutInput::new(identity.clone(), "bootstrap-default", "production")
                    .unwrap(),
            )
            .await
            .unwrap();

        let mut registry = ToolRegistry::new();
        let mut services = Vec::new();
        let mut virtual_servers = Vec::new();
        for index in 0..105 {
            let name: &'static str =
                Box::leak(format!("project_service_{index:03}").into_boxed_str());
            registry.register(RegisteredService {
                name,
                description: "Project pagination fixture",
                category: "test",
                kind: RegisteredServiceKind::BootstrapOperator,
                status: "available",
                actions: ACTIONS,
                dispatch,
            });
            services.push(name.to_string());
            virtual_servers.push(VirtualServerConfig {
                id: name.to_string(),
                service: name.to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    mcp: true,
                    ..Default::default()
                },
                mcp_policy: None,
            });
        }
        let registry = Arc::new(registry);
        let gateway_registry: Arc<dyn labby_gateway::gateway::GatewayServiceRegistry> =
            registry.clone();
        let gateway_runtime = GatewayRuntimeHandle::default();
        gateway_runtime
            .swap(Some(Arc::new(
                labby_gateway::upstream::pool::UpstreamPool::new(),
            )))
            .await;
        let gateway_path = directory.path().join("project-pagination.toml");
        let manager = Arc::new(
            GatewayManager::with_store(
                gateway_path.clone(),
                gateway_runtime,
                Arc::new(FsGatewayConfigStore::new(gateway_path)),
            )
            .with_builtin_service_registry(gateway_registry),
        );
        manager
            .try_seed_config(GatewayConfig {
                loadouts: vec![GatewayLoadoutConfig {
                    name: "production".into(),
                    services: services.clone(),
                    ..Default::default()
                }],
                virtual_servers,
                protected_mcp_routes: vec![ProtectedMcpRouteConfig {
                    name: "project-route".into(),
                    enabled: true,
                    public_host: "mcp.example.com".into(),
                    public_path: "/project".into(),
                    upstream: None,
                    backend_url: String::new(),
                    backend_mcp_path: "/mcp".into(),
                    scopes: vec![],
                    health_path: None,
                    target: Some(ProtectedMcpRouteTarget::GatewaySubset(
                        ProtectedGatewaySubsetTarget {
                            project_id: Some("bootstrap-default".into()),
                            loadout: Some("production".into()),
                            ..Default::default()
                        },
                    )),
                }],
                ..Default::default()
            })
            .await
            .unwrap();

        let now = SystemTime::now();
        let long_expiry = usize::try_from(unix_seconds(now).unwrap() + 3_600).unwrap();
        let make_transport = |jti: &'static str, expiry| {
            let runtime = Arc::clone(&runtime);
            let manager = Arc::clone(&manager);
            let identity = identity.clone();
            async move {
                let core = bind_access_context(
                    &runtime,
                    &manager,
                    identity,
                    "project-route",
                    "https://mcp.example.com/project",
                    "bootstrap-default",
                )
                .await
                .unwrap();
                TransportBoundAccessContext::new(
                    core,
                    validate_transport_credential_binding("issuer", jti, expiry, now).unwrap(),
                    now,
                )
                .unwrap()
            }
        };
        let first_transport = make_transport("first-request-jti", long_expiry).await;
        let resume_transport = make_transport("first-request-jti", long_expiry).await;
        let replay_transport = make_transport("different-request-jti", long_expiry).await;
        let expiring_first_transport = make_transport("expiring-request-jti", long_expiry).await;
        let expired_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .unwrap();
        let expired_resume_transport = TransportBoundAccessContext::new(
            expired_core,
            validate_transport_credential_binding(
                "issuer",
                "expiring-request-jti",
                101,
                UNIX_EPOCH + std::time::Duration::from_secs(100),
            )
            .unwrap(),
            UNIX_EPOCH + std::time::Duration::from_secs(100),
        )
        .unwrap();

        let server = crate::mcp::server::LabMcpServer {
            registry,
            access_runtime: Arc::clone(&runtime),
            file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            gateway_manager: Some(manager),
            peers: Default::default(),
            code_mode_app_state: Default::default(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(0)),
            route_scope: crate::mcp::route_scope::McpRouteScope::protected_subset(
                "project-route",
                std::iter::empty::<&str>(),
                services,
                false,
            ),
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        };
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, io::Error, _>(
            server, transport, None,
        );
        let context = |binding| {
            let mut context = project_shadow_context(
                running.peer().clone(),
                Some(ProjectAccessObservation::Bound(Arc::new(binding))),
            );
            context
                .extensions
                .get_mut::<axum::http::request::Parts>()
                .unwrap()
                .extensions
                .insert(identity.clone());
            context
        };

        let first = running
            .service()
            .list_tools_impl(None, context(first_transport))
            .await
            .expect("first Project page");
        assert_eq!(
            first.tools.len(),
            crate::mcp::pagination::MCP_LIST_PAGE_SIZE
        );
        let cursor = first.next_cursor.clone().expect("Project cursor");

        let second = running
            .service()
            .list_tools_impl(
                Some(PaginatedRequestParams::default().with_cursor(Some(cursor.clone()))),
                context(resume_transport),
            )
            .await
            .expect("same credential snapshot resumes");
        assert_eq!(second.tools.len(), 5);
        assert!(second.next_cursor.is_none());

        let replay = running
            .service()
            .list_tools_impl(
                Some(PaginatedRequestParams::default().with_cursor(Some(cursor))),
                context(replay_transport),
            )
            .await
            .expect_err("different credential must not replay a Project cursor");
        assert_eq!(
            replay.data.as_ref().expect("error data")["kind"],
            serde_json::json!("invalid_cursor")
        );

        let expiring_first = running
            .service()
            .list_tools_impl(None, context(expiring_first_transport))
            .await
            .expect("first expiring Project page");
        let expiring_cursor = expiring_first.next_cursor.expect("expiring Project cursor");
        let expired = running
            .service()
            .list_tools_impl(
                Some(PaginatedRequestParams::default().with_cursor(Some(expiring_cursor))),
                context(expired_resume_transport),
            )
            .await
            .expect("expired Project binding fails closed without revealing rows");
        assert!(expired.tools.is_empty());
        assert!(expired.next_cursor.is_none());
        assert_eq!(expired.ttl_ms, Some(0));
        assert_eq!(expired.cache_scope, Some(rmcp::model::CacheScope::Private));
    }
}
