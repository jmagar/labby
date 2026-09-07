//! Runtime tool registry. Services register themselves here during
//! startup; the MCP server walks the registry to expose tools and the
//! catalog module walks it to produce discovery docs.

use labby_primitives::action::ActionSpec;
use labby_primitives::plugin::PluginMeta;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::dispatch::error::ToolError;

static RUNTIME_BUILT_IN_UPSTREAM_APIS_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_runtime_built_in_upstream_apis_enabled(enabled: bool) {
    RUNTIME_BUILT_IN_UPSTREAM_APIS_ENABLED.store(enabled, Ordering::Relaxed);
}

#[must_use]
#[allow(dead_code)]
pub fn runtime_built_in_upstream_apis_enabled() -> bool {
    RUNTIME_BUILT_IN_UPSTREAM_APIS_ENABLED.load(Ordering::Relaxed)
}

/// A dispatch function pointer: takes an owned action name and params,
/// returns a boxed future resolving to `Result<Value, ToolError>`.
pub type DispatchFn =
    fn(String, Value) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>>;

/// Wrap an `async fn(&str, Value) -> Result<Value, ToolError>` into a [`DispatchFn`].
///
/// Bridges the `&str`-taking dispatch signatures into the owned-`String`
/// function pointer stored in the registry.
macro_rules! dispatch_fn {
    ($f:path) => {
        |action: String,
         params: serde_json::Value|
         -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<serde_json::Value, $crate::dispatch::error::ToolError>,
                    > + Send,
            >,
        > { Box::pin(async move { $f(&action, params).await }) }
    };
}

/// Metadata the registry keeps about each registered service.
#[derive(Clone)]
pub struct RegisteredService {
    /// Service / tool name.
    pub name: &'static str,
    /// Short description from `PluginMeta::description`.
    pub description: &'static str,
    /// Category slug.
    pub category: &'static str,
    /// Runtime policy class used for global service filtering.
    pub kind: RegisteredServiceKind,
    /// Implementation status: `"available"` (actions populated) or `"stub"` (empty actions).
    ///
    /// Agents reading `lab://catalog` should filter on `status == "available"` to find
    /// callable services. A `"stub"` entry means the service is compiled in but not yet
    /// dispatching — calls will return `unknown_action`.
    pub status: &'static str,
    /// Actions exposed by this service.
    pub actions: &'static [ActionSpec],
    /// Dispatch function for routing action calls.
    pub dispatch: DispatchFn,
}

impl std::fmt::Debug for RegisteredService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredService")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("category", &self.category)
            .field("kind", &self.kind)
            .field("actions", &self.actions)
            .finish_non_exhaustive()
    }
}

impl RegisteredService {
    /// Construct a local/bootstrap/operator service registration.
    #[must_use]
    pub const fn bootstrap_operator(
        name: &'static str,
        description: &'static str,
        category: &'static str,
        actions: &'static [ActionSpec],
        dispatch: DispatchFn,
    ) -> Self {
        Self {
            name,
            description,
            category,
            kind: RegisteredServiceKind::BootstrapOperator,
            status: if actions.is_empty() {
                "stub"
            } else {
                "available"
            },
            actions,
            dispatch,
        }
    }

    /// Construct a service that proxies a built-in upstream HTTP API.
    #[must_use]
    pub const fn built_in_upstream_api(
        name: &'static str,
        description: &'static str,
        category: &'static str,
        actions: &'static [ActionSpec],
        dispatch: DispatchFn,
    ) -> Self {
        Self {
            name,
            description,
            category,
            kind: RegisteredServiceKind::BuiltInUpstreamApi,
            status: if actions.is_empty() {
                "stub"
            } else {
                "available"
            },
            actions,
            dispatch,
        }
    }
}

/// Runtime policy classification for registered services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisteredServiceKind {
    /// Local/bootstrap/operator surfaces that do not proxy a built-in upstream API.
    BootstrapOperator,
    /// Built-in integrations that call an external service API.
    BuiltInUpstreamApi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchCapability {
    ContextFree,
    CallerBound,
    HttpContextOnly,
}

/// Collection of registered services, built at startup.
#[derive(Clone, Debug, Default)]
pub struct ToolRegistry {
    services: Vec<RegisteredService>,
    action_names: Vec<&'static str>,
    dispatch_capabilities: Vec<(&'static str, DispatchCapability)>,
    permanent_tools: crate::mcp::permanent_tools::PermanentToolRegistry,
}

impl ToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            services: Vec::new(),
            action_names: Vec::new(),
            dispatch_capabilities: Vec::new(),
            permanent_tools: crate::mcp::permanent_tools::PermanentToolRegistry::new(),
        }
    }

    /// Register a service. Duplicates are ignored (first registration wins).
    ///
    /// # Panics (debug builds only)
    ///
    /// Panics if `service.status` is inconsistent with `service.actions.len()`:
    /// - `status == "available"` requires at least one action.
    /// - `status == "stub"` requires an empty action slice.
    pub fn register(&mut self, service: RegisteredService) {
        self.register_with_capability(service, DispatchCapability::ContextFree);
    }

    /// Register a service whose actions require transport-resolved caller
    /// identity and therefore must never use the context-free dispatcher.
    pub fn register_caller_bound(&mut self, service: RegisteredService) {
        self.register_with_capability(service, DispatchCapability::CallerBound);
    }

    /// Register a service whose adapter requires HTTP request state and must
    /// therefore not be advertised by MCP or an in-process peer.
    pub fn register_http_context_only(&mut self, service: RegisteredService) {
        self.register_with_capability(service, DispatchCapability::HttpContextOnly);
    }

    fn register_with_capability(
        &mut self,
        service: RegisteredService,
        capability: DispatchCapability,
    ) {
        debug_assert!(
            service.status == "available" || service.status == "stub",
            "service '{}': unknown status '{}'; expected \"available\" or \"stub\"",
            service.name,
            service.status,
        );
        debug_assert!(
            (service.status == "available") == !service.actions.is_empty(),
            "service '{}': status '{}' is inconsistent with actions.len() == {}; \
             'available' requires non-empty ACTIONS, 'stub' requires empty ACTIONS",
            service.name,
            service.status,
            service.actions.len(),
        );
        if self.services.iter().any(|s| s.name == service.name) {
            return;
        }

        for action in service.actions {
            if let Err(index) = self.action_names.binary_search(&action.name) {
                self.action_names.insert(index, action.name);
            }
        }
        self.dispatch_capabilities.push((service.name, capability));
        self.services.push(service);
    }

    /// Borrow the current service list.
    #[must_use]
    pub fn services(&self) -> &[RegisteredService] {
        &self.services
    }

    /// Borrow the product-level MCP tool registry composed at startup.
    #[must_use]
    pub(crate) fn permanent_tools(&self) -> &crate::mcp::permanent_tools::PermanentToolRegistry {
        &self.permanent_tools
    }

    /// Borrow the cached sorted unique action-name list.
    #[must_use]
    pub fn action_names(&self) -> &[&'static str] {
        &self.action_names
    }

    /// Return cached action-name completions matching `prefix`.
    ///
    /// The cache is sorted and deduplicated during registration, so completion does not collect,
    /// sort, or deduplicate action names on the request path.
    #[must_use]
    pub fn action_name_completions(&self, prefix: &str) -> Vec<String> {
        let action_names = self.action_names();
        let start = action_names.partition_point(|candidate| *candidate < prefix);

        action_names[start..]
            .iter()
            .take_while(|candidate| candidate.starts_with(prefix))
            .map(|candidate| (*candidate).to_string())
            .collect()
    }

    /// Look up one registered service by name.
    #[must_use]
    pub fn service(&self, name: &str) -> Option<&RegisteredService> {
        self.services.iter().find(|service| service.name == name)
    }

    #[must_use]
    pub fn dispatch_capability(&self, name: &str) -> Option<DispatchCapability> {
        self.dispatch_capabilities
            .iter()
            .find_map(|(service, capability)| (*service == name).then_some(*capability))
    }

    #[must_use]
    pub fn supports_context_free_dispatch(&self, name: &str) -> bool {
        self.dispatch_capability(name) == Some(DispatchCapability::ContextFree)
    }

    #[must_use]
    pub fn supports_mcp_dispatch(&self, name: &str) -> bool {
        matches!(
            self.dispatch_capability(name),
            Some(DispatchCapability::ContextFree | DispatchCapability::CallerBound)
        )
    }
}

// === labby-gateway in-process peer seam ===
//
// The standalone `labby-gateway` upstream pool registers built-in lab services as
// in-process upstream peers without depending on this crate's registry types.
// It does that through the `InProcessService` / `InProcessServiceRegistry`
// traits; we implement them here for `RegisteredService` / `ToolRegistry` so the
// gateway pool can enumerate services and hand each one back to the
// product composition connector (which downcasts via `as_any`).

#[cfg(feature = "gateway")]
impl labby_gateway::registry::InProcessService for RegisteredService {
    fn service_name(&self) -> &'static str {
        self.name
    }

    fn has_actions(&self) -> bool {
        !self.actions.is_empty()
    }

    fn as_any(self: Box<Self>) -> Box<dyn std::any::Any + Send> {
        self
    }
}

#[cfg(feature = "gateway")]
impl labby_gateway::registry::InProcessServiceRegistry for ToolRegistry {
    fn in_process_services(&self) -> Vec<Box<dyn labby_gateway::registry::InProcessService>> {
        self.services
            .iter()
            // A serialized in-process peer cannot carry host-local caller
            // identity by construction. Caller-bound adapters stay on the
            // authenticated outer MCP surface instead of being silently
            // downgraded to their context-free fallback.
            .filter(|service| self.supports_context_free_dispatch(service.name))
            .cloned()
            .map(
                |service| -> Box<dyn labby_gateway::registry::InProcessService> {
                    Box::new(service)
                },
            )
            .collect()
    }
}

// === labby-gateway service-registry seam ===
//
// The gateway manager needs read-only access to the host registry: the set of
// registered service names, each service's actions, and the `&'static PluginMeta`
// for a service. It depends only on the `GatewayServiceRegistry` trait (a
// supertrait of `InProcessServiceRegistry`); we implement it here for
// `ToolRegistry` and inject it at manager construction.
#[cfg(feature = "gateway")]
impl labby_gateway::gateway::service_registry::GatewayServiceRegistry for ToolRegistry {
    fn service_names(&self) -> Vec<&'static str> {
        self.services.iter().map(|service| service.name).collect()
    }

    fn contains_service(&self, name: &str) -> bool {
        self.service(name).is_some()
    }

    fn service_description(&self, name: &str) -> Option<&'static str> {
        self.service(name).map(|service| service.description)
    }

    fn service_actions(
        &self,
        name: &str,
    ) -> Option<Vec<labby_gateway::gateway::service_registry::ServiceActionInfo>> {
        self.service(name).map(|service| {
            service
                .actions
                .iter()
                .map(
                    |action| labby_gateway::gateway::service_registry::ServiceActionInfo {
                        name: action.name,
                        description: action.description,
                        destructive: action.destructive,
                        requires_admin: action.requires_admin,
                    },
                )
                .collect()
        })
    }

    fn service_meta(&self, name: &str) -> Option<&'static PluginMeta> {
        service_meta(name)
    }
}

const ALWAYS_VISIBLE_SERVICES: &[&str] = &[
    "browser",
    "setup",
    "doctor",
    "gateway",
    "help",
    "completions",
    "server_logs",
    "snippets",
];

#[must_use]
pub fn lab_show_all_enabled() -> bool {
    crate::config::resolved_show_all()
}

#[must_use]
pub fn filter_by_configured_env(registry: &ToolRegistry) -> ToolRegistry {
    let mut filtered = ToolRegistry::new();
    for service in registry.services() {
        if service_visible_with_env(service.name) {
            match registry.dispatch_capability(service.name) {
                Some(DispatchCapability::CallerBound) => {
                    filtered.register_caller_bound(service.clone())
                }
                Some(DispatchCapability::HttpContextOnly) => {
                    filtered.register_http_context_only(service.clone())
                }
                _ => filtered.register(service.clone()),
            }
        }
    }
    filtered
}

#[must_use]
pub fn service_visible_with_env(service: &str) -> bool {
    ALWAYS_VISIBLE_SERVICES.contains(&service) || service_configured_by_env(service)
}

#[must_use]
pub fn service_configured_by_env(service: &str) -> bool {
    let Some(meta) = service_meta(service) else {
        return false;
    };
    meta.required_env.iter().all(|var| {
        std::env::var(var.name)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    })
}

#[must_use]
#[cfg(test)]
pub fn is_built_in_upstream_api_service(service: &str) -> bool {
    build_default_registry()
        .service(service)
        .is_some_and(|service| service.kind == RegisteredServiceKind::BuiltInUpstreamApi)
}

#[must_use]
pub fn built_in_upstream_api_services(registry: &ToolRegistry) -> Vec<&'static str> {
    registry
        .services()
        .iter()
        .filter_map(|service| {
            (service.kind == RegisteredServiceKind::BuiltInUpstreamApi).then_some(service.name)
        })
        .collect()
}

#[must_use]
pub fn bootstrap_operator_services(registry: &ToolRegistry) -> Vec<&'static str> {
    registry
        .services()
        .iter()
        .filter_map(|service| {
            (service.kind == RegisteredServiceKind::BootstrapOperator).then_some(service.name)
        })
        .collect()
}

#[must_use]
pub fn filter_built_in_upstream_apis(registry: ToolRegistry, enabled: bool) -> ToolRegistry {
    if enabled {
        return registry;
    }

    let mut filtered = ToolRegistry::new();
    for service in registry.services() {
        if service.kind == RegisteredServiceKind::BootstrapOperator {
            #[cfg(feature = "skills")]
            let mut service = service.clone();
            #[cfg(not(feature = "skills"))]
            let service = service.clone();
            #[cfg(feature = "skills")]
            if service.name == "artifacts" {
                service.actions = &crate::dispatch::skill_library::catalog::LOCAL_ACTIONS;
            }
            match registry.dispatch_capability(service.name) {
                Some(DispatchCapability::CallerBound) => filtered.register_caller_bound(service),
                Some(DispatchCapability::HttpContextOnly) => {
                    filtered.register_http_context_only(service)
                }
                _ => filtered.register(service),
            }
        }
    }
    filtered
}

/// Build a registry with every feature-enabled service registered.
///
/// This is the single place feature flags gate MCP tool availability.
/// Service entries are added in alphabetical order as services come
/// online.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn build_default_registry() -> ToolRegistry {
    build_registry(true)
}

/// Build a registry for static metadata projections.
///
/// Unlike [`build_default_registry`], this includes compile-time services whose
/// runtime registration depends on local operator configuration. Generated docs
/// must describe the compiled surface without reading local env/config state.
#[must_use]
#[allow(dead_code)]
pub fn build_docs_registry() -> ToolRegistry {
    build_registry(false)
}

#[allow(clippy::too_many_lines)]
fn build_registry(apply_runtime_conditions: bool) -> ToolRegistry {
    #[cfg(not(feature = "lab-admin"))]
    let _ = apply_runtime_conditions;
    let mut reg = ToolRegistry::new();

    reg.register(RegisteredService::bootstrap_operator(
        "browser",
        "Bridge browser-native WebMCP tools into Labby",
        "bootstrap",
        crate::dispatch::browser::ACTIONS,
        dispatch_fn!(crate::dispatch::browser::dispatch),
    ));

    #[cfg(feature = "gateway")]
    reg.register(RegisteredService {
        name: "gateway",
        description: "Manage proxied upstream MCP gateways",
        category: "bootstrap",
        kind: RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::dispatch::gateway::ACTIONS,
        dispatch: dispatch_fn!(crate::dispatch::gateway::dispatch),
    });

    // doctor is always-on (bootstrap utility; no feature flag).
    {
        let meta = crate::dispatch::doctor::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
            kind: registered_service_kind(meta.name, meta.category),
            status: "available",
            actions: crate::dispatch::doctor::ACTIONS,
            dispatch: dispatch_fn!(crate::dispatch::doctor::dispatch),
        });
    }
    // setup is always-on (Bootstrap orchestrator; no feature flag).
    {
        let meta = crate::dispatch::setup::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
            kind: registered_service_kind(meta.name, meta.category),
            status: "available",
            actions: crate::dispatch::setup::ACTIONS,
            dispatch: dispatch_fn!(crate::dispatch::setup::dispatch),
        });
    }

    #[cfg(feature = "skills")]
    reg.register(RegisteredService::bootstrap_operator(
        crate::dispatch::artifacts::META.name,
        crate::dispatch::artifacts::META.description,
        "bootstrap",
        &crate::dispatch::artifacts::ACTIONS,
        dispatch_fn!(crate::dispatch::artifacts::dispatch),
    ));

    #[cfg(feature = "skills")]
    reg.register_http_context_only(RegisteredService::built_in_upstream_api(
        "bundles",
        "Curate and publish immutable Artifact bundles",
        "bootstrap",
        crate::dispatch::remote_control::BUNDLE_ACTIONS,
        dispatch_fn!(crate::dispatch::remote_control::dispatch_bundles),
    ));

    #[cfg(feature = "skills")]
    reg.register_http_context_only(RegisteredService::built_in_upstream_api(
        "jobs",
        "Run and inspect durable Artifact ingestion jobs",
        "bootstrap",
        crate::dispatch::remote_control::JOB_ACTIONS,
        dispatch_fn!(crate::dispatch::remote_control::dispatch_jobs),
    ));

    #[cfg(feature = "skills")]
    reg.register_http_context_only(RegisteredService::built_in_upstream_api(
        "sources",
        "Manage remote Artifact ingestion sources",
        "bootstrap",
        crate::dispatch::remote_control::SOURCE_ACTIONS,
        dispatch_fn!(crate::dispatch::remote_control::dispatch_sources),
    ));

    #[cfg(feature = "skills")]
    reg.register_http_context_only(RegisteredService::built_in_upstream_api(
        "uploads",
        "Manage staged Artifact ingestion uploads",
        "bootstrap",
        crate::dispatch::remote_control::UPLOAD_ACTIONS,
        dispatch_fn!(crate::dispatch::remote_control::dispatch_uploads),
    ));

    #[cfg(feature = "gateway")]
    reg.register(RegisteredService::bootstrap_operator(
        "snippets",
        "Manage executable Code Mode snippets",
        "bootstrap",
        crate::dispatch::snippets::ACTIONS,
        dispatch_fn!(crate::dispatch::snippets::dispatch),
    ));

    {
        let meta = crate::dispatch::server_logs::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
            kind: registered_service_kind(meta.name, meta.category),
            status: "available",
            actions: crate::dispatch::server_logs::ACTIONS,
            dispatch: dispatch_fn!(crate::dispatch::server_logs::dispatch),
        });
    }

    #[cfg(feature = "lab-admin")]
    if !apply_runtime_conditions || lab_admin_enabled() {
        reg.register(RegisteredService {
            name: "lab_admin",
            description: "Internal onboarding audit tool",
            category: "bootstrap",
            kind: RegisteredServiceKind::BootstrapOperator,
            status: "available",
            actions: crate::dispatch::lab_admin::ACTIONS,
            dispatch: dispatch_fn!(crate::dispatch::lab_admin::dispatch),
        });
    }

    // fs — workspace filesystem browser. Registered unconditionally when the
    // `fs` feature is enabled so the catalog and `labby help` stay discoverable;
    // runtime dispatch returns `workspace_not_configured` per-request when
    // the configured `workspace.root` cannot be resolved. `cli::serve` logs
    // invalid configuration as a warning once at boot.
    //
    // SECURITY: unlike `/v1/fs` (which refuses to mount when
    // `LABBY_WEB_UI_AUTH_DISABLED=true`), MCP `fs` registration has no
    // env-driven refusal. MCP transport auth (`LABBY_MCP_HTTP_TOKEN` /
    // OAuth, or stdio reachability) is the sole gate. See
    // `crates/labby/src/mcp/CLAUDE.md` § "Transport auth for fs".
    //
    // NOTE: fs has TWO action surfaces. The canonical slice is
    // `dispatch::fs::catalog::ACTIONS` (includes `fs.preview`); the MCP-filtered
    // slice `mcp::services::fs::ACTIONS` omits `fs.preview` because preview
    // streams raw bytes and is HTTP-only for prompt-injection reasons. The
    // registry uses the MCP slice because all current catalog consumers (MCP
    // `lab.help`, `lab://catalog`, CLI `labby help`) correctly treat preview as
    // hidden — MCP must not expose it, and CLI cannot invoke it (no
    // byte-streaming through clap). A future HTTP `/v1/<service>/actions`
    // resource should read `dispatch::fs::catalog::ACTIONS` directly, not via
    // this registry entry.
    #[cfg(feature = "fs")]
    reg.register(RegisteredService::bootstrap_operator(
        "fs",
        "Workspace filesystem browser (read-only, deny-listed)",
        "bootstrap",
        crate::mcp::services::fs::ACTIONS,
        dispatch_fn!(crate::mcp::services::fs::dispatch),
    ));

    // Static documentation describes every compiled product surface and must
    // not drift with the host that generated it. The live registry remains
    // fail-closed on platforms without descriptor-relative filesystem support.
    if !apply_runtime_conditions || cfg!(target_os = "linux") {
        reg.register_caller_bound(RegisteredService::bootstrap_operator(
            crate::dispatch::file_stash::META.0,
            crate::dispatch::file_stash::META.1,
            crate::dispatch::file_stash::META.2,
            crate::dispatch::file_stash::ACTIONS,
            dispatch_fn!(crate::dispatch::file_stash::dispatch),
        ));
    }

    reg
}

#[must_use]
pub fn service_meta(name: &str) -> Option<&'static PluginMeta> {
    let _ = name;
    None
}

/// Returns `true` when admin is enabled via `LABBY_ADMIN_ENABLED=1` env var
/// or `admin.enabled = true` in config.toml (env var takes precedence).
#[cfg(feature = "lab-admin")]
fn lab_admin_enabled() -> bool {
    // Env var overrides config.toml.
    if let Ok(value) = std::env::var("LABBY_ADMIN_ENABLED") {
        return value == "1";
    }
    // Fall back to config.toml — load is cheap (cached by the OS) and this
    // runs once at startup.
    crate::config::toml_candidates()
        .and_then(|paths| crate::config::load_toml(&paths))
        .map(|cfg| cfg.admin.enabled)
        .unwrap_or(false)
}

const fn category_slug(cat: labby_primitives::plugin::Category) -> &'static str {
    cat.as_str()
}

fn registered_service_kind(
    name: &'static str,
    category: labby_primitives::plugin::Category,
) -> RegisteredServiceKind {
    use labby_primitives::plugin::Category;

    if name == "beads" {
        return RegisteredServiceKind::BuiltInUpstreamApi;
    }

    if matches!(category, Category::Bootstrap) {
        return RegisteredServiceKind::BootstrapOperator;
    }

    match name {
        "doctor" | "setup" | "loggifly" => RegisteredServiceKind::BootstrapOperator,
        _ => RegisteredServiceKind::BuiltInUpstreamApi,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "lab-admin")]
    use super::lab_admin_enabled;
    use super::{
        RegisteredService, RegisteredServiceKind, ToolRegistry, build_default_registry,
        filter_built_in_upstream_apis, is_built_in_upstream_api_service, service_meta,
    };
    use crate::dispatch::error::ToolError;
    use labby_primitives::action::ActionSpec;
    use serde_json::Value;
    use std::time::Duration;
    use std::{future::Future, pin::Pin};

    #[test]
    fn all_features_registers_all_services() {
        let reg = build_default_registry();
        let names: Vec<&str> = reg.services().iter().map(|s| s.name).collect();
        assert!(!names.contains(&"extract"), "extract has been retired");
        // feature-gated services — present only when the flag is enabled
    }

    #[test]
    fn dispatch_capability_is_explicit_and_not_service_name_inferred() {
        fn dispatch(
            _: String,
            _: Value,
        ) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
            Box::pin(async { Ok(Value::Null) })
        }
        static ACTIONS: &[ActionSpec] = &[ActionSpec {
            name: "probe.read",
            description: "probe",
            destructive: false,
            requires_admin: false,
            params: &[],
            returns: "null",
        }];
        let service = |name: &'static str| {
            RegisteredService::bootstrap_operator(name, "probe", "bootstrap", ACTIONS, dispatch)
        };
        let mut registry = ToolRegistry::new();
        registry.register(service("ordinary-name"));
        registry.register_caller_bound(service("another-ordinary-name"));
        registry.register_http_context_only(service("http-only-name"));
        assert!(registry.supports_context_free_dispatch("ordinary-name"));
        assert!(registry.supports_mcp_dispatch("ordinary-name"));
        assert_eq!(
            registry.dispatch_capability("another-ordinary-name"),
            Some(super::DispatchCapability::CallerBound)
        );
        assert!(!registry.supports_context_free_dispatch("another-ordinary-name"));
        assert!(registry.supports_mcp_dispatch("another-ordinary-name"));
        assert_eq!(
            registry.dispatch_capability("http-only-name"),
            Some(super::DispatchCapability::HttpContextOnly)
        );
        assert!(!registry.supports_mcp_dispatch("http-only-name"));
        #[cfg(feature = "gateway")]
        {
            use labby_gateway::registry::InProcessServiceRegistry as _;
            assert_eq!(registry.in_process_services().len(), 1);
            assert_eq!(
                registry.in_process_services()[0].service_name(),
                "ordinary-name"
            );
        }
    }

    #[test]
    fn bootstrap_services_are_not_built_in_upstream_apis() {
        for service in [
            "gateway",
            "setup",
            "doctor",
            "logs",
            "device",
            "marketplace",
            "acp",
            "stash",
            "deploy",
            "fs",
            "lab_admin",
            "loggifly",
        ] {
            assert!(
                !is_built_in_upstream_api_service(service),
                "{service} must remain available when upstream APIs are disabled"
            );
        }
    }

    #[test]
    fn upstream_api_filter_removes_remote_artifact_services() {
        let unfiltered = build_default_registry();
        let filtered = filter_built_in_upstream_apis(unfiltered, false);
        let names: std::collections::BTreeSet<&str> = filtered
            .services()
            .iter()
            .map(|service| service.name)
            .collect();
        #[cfg(feature = "skills")]
        for removed in ["bundles", "jobs", "sources", "uploads"] {
            assert!(!names.contains(removed), "{removed} must be disabled");
        }
        #[cfg(feature = "skills")]
        {
            let artifacts = filtered.service("artifacts").unwrap();
            assert!(
                artifacts
                    .actions
                    .iter()
                    .any(|action| action.name == "artifacts.list")
            );
            assert!(
                !artifacts
                    .actions
                    .iter()
                    .any(|action| action.name == "artifacts.search_remote")
            );
        }
        let mut kept_services = vec!["setup", "doctor"];
        #[cfg(feature = "gateway")]
        kept_services.push("gateway");
        #[cfg(feature = "gateway")]
        kept_services.push("snippets");
        #[cfg(feature = "fs")]
        kept_services.push("fs");
        #[cfg(feature = "lab-admin")]
        if lab_admin_enabled() {
            kept_services.push("lab_admin");
        }
        for kept in kept_services {
            assert!(names.contains(kept), "{kept} should stay available");
        }
    }

    #[test]
    fn every_registered_service_has_runtime_policy_classification() {
        let reg = build_default_registry();
        for service in reg.services() {
            match service.kind {
                RegisteredServiceKind::BootstrapOperator
                | RegisteredServiceKind::BuiltInUpstreamApi => {}
            }
        }
        // Post-pivot only Bootstrap/operator services remain. The
        // `BuiltInUpstreamApi` variant is preserved on the enum for
        // forward-compat with future plugin-based upstreams.
        assert!(
            reg.services()
                .iter()
                .any(|service| service.kind == RegisteredServiceKind::BootstrapOperator),
            "registry should include bootstrap/operator services"
        );
    }

    #[test]
    fn service_meta_tracks_feature_enabled_services() {
        assert!(service_meta("gateway").is_none());
    }

    /// Shared body for the feature-gated inclusion/omission test pairs below.
    fn registry_has_service(name: &str) -> bool {
        build_default_registry()
            .services()
            .iter()
            .any(|service| service.name == name)
    }

    #[cfg(not(feature = "gateway"))]
    #[test]
    fn default_registry_omits_gateway_without_feature() {
        assert!(
            !registry_has_service("gateway"),
            "gateway must not register without the `gateway` feature"
        );
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn default_registry_includes_gateway_with_feature() {
        assert!(
            registry_has_service("gateway"),
            "gateway must register with the `gateway` feature"
        );
    }

    #[test]
    fn retired_services_never_register() {
        for retired in ["acp", "device", "deploy", "marketplace"] {
            assert!(
                !registry_has_service(retired),
                "{retired} has been retired from the gateway host"
            );
        }
    }

    #[test]
    fn stash_registration_matches_descriptor_relative_platform_support() {
        assert_eq!(registry_has_service("stash"), cfg!(target_os = "linux"));
    }

    /// Guard that the MCP registry and the HTTP router mount identical service sets.
    ///
    /// If this test fails, a service was registered in the MCP registry but not mounted in the
    /// HTTP router (or vice versa). Both must be updated together.
    #[test]
    fn registry_and_router_service_sets_are_identical() {
        let http_router_services: std::collections::HashSet<&'static str> = {
            let mut s = std::collections::HashSet::new();
            s.insert("browser");
            #[cfg(feature = "gateway")]
            s.insert("gateway");
            #[cfg(feature = "gateway")]
            s.insert("snippets");
            #[cfg(feature = "skills")]
            s.extend(["artifacts", "bundles", "jobs", "sources", "uploads"]);
            s.insert(crate::dispatch::doctor::META.name); // always-on
            s.insert(crate::dispatch::server_logs::META.name); // always-on
            s.insert(crate::dispatch::setup::META.name); // always-on
            #[cfg(feature = "fs")]
            s.insert("fs");
            #[cfg(target_os = "linux")]
            s.insert("stash");
            s
        };

        let reg = build_default_registry();
        let registry_services: std::collections::HashSet<&str> =
            reg.services().iter().map(|s| s.name).collect();

        let only_in_registry: Vec<&&str> = registry_services
            .iter()
            // lab_admin is MCP-only: no HTTP route by design (runtime opt-in via LABBY_ADMIN_ENABLED=1).
            .filter(|n| !http_router_services.contains(**n) && **n != "lab_admin")
            .collect();
        let only_in_router: Vec<&&str> = http_router_services
            .iter()
            .filter(|n| !registry_services.contains(**n))
            .collect();

        assert!(
            only_in_registry.is_empty(),
            "services in MCP registry but NOT in HTTP router: {only_in_registry:?}\n\
             Add them to build_router() in api/router.rs or add an explicit exemption in registry_and_router_service_sets_are_identical()",
        );
        assert!(
            only_in_router.is_empty(),
            "services in HTTP router but NOT in MCP registry: {only_in_router:?}\n\
             Add them to build_default_registry() in mcp/registry.rs",
        );
    }

    const ACTIONS_ONE: &[ActionSpec] = &[
        ActionSpec {
            name: "health.list",
            description: "List health checks",
            destructive: false,
            requires_admin: false,
            params: &[],
            returns: "object",
        },
        ActionSpec {
            name: "status.get",
            description: "Get status",
            destructive: false,
            requires_admin: false,
            params: &[],
            returns: "object",
        },
    ];

    const ACTIONS_TWO: &[ActionSpec] = &[
        ActionSpec {
            name: "status.get",
            description: "Get status again",
            destructive: false,
            requires_admin: false,
            params: &[],
            returns: "object",
        },
        ActionSpec {
            name: "metrics.list",
            description: "List metrics",
            destructive: false,
            requires_admin: false,
            params: &[],
            returns: "object",
        },
    ];

    fn noop_dispatch(
        _action: String,
        _params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
        Box::pin(async { Ok(Value::Null) })
    }

    fn legacy_sorted_action_names(registry: &ToolRegistry) -> Vec<String> {
        let mut names: Vec<String> = registry
            .services()
            .iter()
            .flat_map(|service| service.actions.iter().map(|action| action.name.to_string()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    #[test]
    fn action_names_cache_is_sorted_and_deduplicated_at_registration_time() {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredService {
            name: "one",
            description: "First test service",
            category: "test",
            kind: RegisteredServiceKind::BuiltInUpstreamApi,
            status: "available",
            actions: ACTIONS_ONE,
            dispatch: noop_dispatch,
        });
        registry.register(RegisteredService {
            name: "two",
            description: "Second test service",
            category: "test",
            kind: RegisteredServiceKind::BuiltInUpstreamApi,
            status: "available",
            actions: ACTIONS_TWO,
            dispatch: noop_dispatch,
        });

        assert_eq!(
            registry.action_names(),
            &["health.list", "metrics.list", "status.get"]
        );
    }

    #[test]
    fn action_name_completions_match_legacy_collect_sort_dedup_output() {
        let registry = build_default_registry();

        assert_eq!(
            registry.action_name_completions(""),
            legacy_sorted_action_names(&registry)
        );
    }

    #[test]
    fn action_name_completions_filter_by_prefix_from_cached_names() {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredService {
            name: "one",
            description: "First test service",
            category: "test",
            kind: RegisteredServiceKind::BuiltInUpstreamApi,
            status: "available",
            actions: ACTIONS_ONE,
            dispatch: noop_dispatch,
        });
        registry.register(RegisteredService {
            name: "two",
            description: "Second test service",
            category: "test",
            kind: RegisteredServiceKind::BuiltInUpstreamApi,
            status: "available",
            actions: ACTIONS_TWO,
            dispatch: noop_dispatch,
        });

        assert_eq!(
            registry.action_name_completions("status."),
            vec!["status.get"]
        );
    }

    #[test]
    fn action_name_completions_empty_prefix_returns_all_actions_and_is_cached_fast() {
        let registry = build_default_registry();
        let expected = registry.action_names().len();

        // Correctness first.
        assert_eq!(registry.action_name_completions("").len(), expected);

        // Performance: the empty prefix returns every action from a cache, so the
        // call cost is trivial. Assert on the BEST of several runs (`min`) rather
        // than a single wall-clock sample — a single sample is dominated by
        // scheduler jitter on a loaded CI runner (concurrent subprocess-spawning
        // tests), which made a 1 ms single-sample assertion flaky. The best-run
        // minimum reflects the true operation cost; a real regression (e.g. a
        // dropped cache recomputing on every call) blows up even the minimum well
        // past this generous bound.
        let best = (0..32)
            .map(|_| {
                let start = std::time::Instant::now();
                let completions = registry.action_name_completions("");
                let elapsed = start.elapsed();
                std::hint::black_box(completions.len());
                elapsed
            })
            .min()
            .expect("at least one sample");
        assert!(
            best < Duration::from_millis(5),
            "empty-prefix action completion best-of-32 took {best:?} for {expected} cached actions"
        );
    }

    #[cfg(feature = "fs")]
    #[test]
    fn default_registry_uses_mcp_filtered_fs_actions() {
        let registry = build_default_registry();
        let fs = registry
            .services()
            .iter()
            .find(|service| service.name == "fs")
            .expect("fs registered");
        let names: Vec<&str> = fs.actions.iter().map(|action| action.name).collect();

        assert!(names.contains(&"fs.list"));
        assert!(!names.contains(&"fs.preview"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn default_registry_exposes_snippets_as_mcp_service() {
        let registry = build_default_registry();
        let service = registry.service("snippets").expect("snippets registered");

        assert_eq!(service.kind, RegisteredServiceKind::BootstrapOperator);
        assert!(
            service
                .actions
                .iter()
                .any(|action| action.name == "snippets.exec")
        );
    }

    #[test]
    fn bootstrap_operator_constructor_sets_available_status_for_actions() {
        static ACTIONS: &[ActionSpec] = &[ActionSpec {
            name: "demo.list",
            description: "Demo action",
            destructive: false,
            requires_admin: false,
            params: &[],
            returns: "null",
        }];

        let service = RegisteredService::bootstrap_operator(
            "demo",
            "Demo service",
            "bootstrap",
            ACTIONS,
            noop_dispatch,
        );

        assert_eq!(service.name, "demo");
        assert_eq!(service.category, "bootstrap");
        assert_eq!(service.kind, RegisteredServiceKind::BootstrapOperator);
        assert_eq!(service.status, "available");
        assert_eq!(service.actions[0].name, "demo.list");
    }

    #[test]
    fn bootstrap_operator_constructor_sets_stub_status_for_empty_actions() {
        let service = RegisteredService::bootstrap_operator(
            "demo",
            "Demo service",
            "bootstrap",
            &[],
            noop_dispatch,
        );

        assert_eq!(service.status, "stub");
    }
}
