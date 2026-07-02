//! Runtime tool registry. Services register themselves here during
//! startup; the MCP server walks the registry to expose tools and the
//! catalog module walks it to produce discovery docs.

use labby_apis::core::PluginMeta;
use labby_apis::core::action::ActionSpec;
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

/// Register a standard service (feature name == module name, uses `dispatch::$svc`).
///
/// Expands to the `#[cfg(feature)] { reg.register(RegisteredService { ... }) }` block,
/// eliminating the 7-line boilerplate that would otherwise be repeated per service.
///
/// Two forms:
/// - Default: `register_service!(reg, "foo", foo)` — uses `dispatch::foo::ACTIONS` and
///   `dispatch::foo::dispatch`.
/// - Override: `register_service!(reg, "foo", foo, actions = $expr, dispatch = $expr)` —
///   for services whose catalog is exposed through `actions()` instead of a top-level
///   `ACTIONS` const, or for proven MCP-specific exception modules.
///
/// # Consistency invariant
///
/// The `actions` slice and the `dispatch` function **must be kept in sync** by the author:
///
/// - If `ACTIONS` is non-empty (status `"available"`), the dispatch function **must** handle
///   at least `"help"` and every action listed in `ACTIONS`, returning `Ok(Value)`.
/// - If `ACTIONS` is empty (status `"stub"`), the dispatch function is never called by agents
///   that filter on `status == "available"`, but it may still be invoked directly. A stub
///   dispatch should return an `unknown_action` envelope for all inputs.
///
/// A debug-build runtime check is performed in [`ToolRegistry::register`]: it asserts that
/// `status` is consistent with `actions.len()`.
macro_rules! register_service {
    // Full override: custom actions expr and dispatch expr (for migrated services).
    ($reg:expr, $feature:literal, $svc:ident, actions = $actions:expr, dispatch = $dispatch:expr) => {
        #[cfg(feature = $feature)]
        {
            let meta = labby_apis::$svc::META;
            let actions: &'static [ActionSpec] = $actions;
            $reg.register(RegisteredService {
                name: meta.name,
                description: meta.description,
                category: category_slug(meta.category),
                kind: registered_service_kind(meta.name, meta.category),
                status: if actions.is_empty() {
                    "stub"
                } else {
                    "available"
                },
                actions,
                dispatch: $dispatch,
            });
        }
    };
    // Default: use dispatch::$svc ACTIONS const and dispatch fn.
    ($reg:expr, $feature:literal, $svc:ident) => {
        #[cfg(feature = $feature)]
        {
            let meta = labby_apis::$svc::META;
            let actions: &'static [ActionSpec] = crate::dispatch::$svc::ACTIONS;
            $reg.register(RegisteredService {
                name: meta.name,
                description: meta.description,
                category: category_slug(meta.category),
                kind: registered_service_kind(meta.name, meta.category),
                status: if actions.is_empty() {
                    "stub"
                } else {
                    "available"
                },
                actions,
                dispatch: dispatch_fn!(crate::dispatch::$svc::dispatch),
            });
        }
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
}

/// Runtime policy classification for registered services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisteredServiceKind {
    /// Local/bootstrap/operator surfaces that do not proxy a built-in upstream API.
    BootstrapOperator,
    /// Built-in integrations that call an external service API.
    BuiltInUpstreamApi,
}

/// Collection of registered services, built at startup.
#[derive(Clone, Debug, Default)]
pub struct ToolRegistry {
    services: Vec<RegisteredService>,
    action_names: Vec<&'static str>,
}

impl ToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            services: Vec::new(),
            action_names: Vec::new(),
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
        self.services.push(service);
    }

    /// Borrow the current service list.
    #[must_use]
    pub fn services(&self) -> &[RegisteredService] {
        &self.services
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
}

// === lab-gateway in-process peer seam ===
//
// The standalone `lab-gateway` upstream pool registers built-in lab services as
// in-process upstream peers without depending on this crate's registry types.
// It does that through the `InProcessService` / `InProcessServiceRegistry`
// traits; we implement them here for `RegisteredService` / `ToolRegistry` so the
// gateway pool can enumerate services and hand each one back to the
// `mcp::in_process_peer` connector (which downcasts via `as_any`).

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
            .cloned()
            .map(
                |service| -> Box<dyn labby_gateway::registry::InProcessService> {
                    Box::new(service)
                },
            )
            .collect()
    }
}

// === lab-gateway service-registry seam ===
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
    "init",
    "setup",
    "doctor",
    "plugins",
    "gateway",
    "help",
    "completions",
    "scaffold",
    "audit",
    "marketplace",
    "logs",
    "snippets",
    "device",
    "acp",
    "stash",
];

#[must_use]
pub fn lab_show_all_enabled() -> bool {
    crate::config::env_flag_enabled("LAB_SHOW_ALL")
}

#[must_use]
pub fn filter_by_configured_env(registry: &ToolRegistry) -> ToolRegistry {
    let mut filtered = ToolRegistry::new();
    for service in registry.services() {
        if service_visible_with_env(service.name) {
            filtered.register(service.clone());
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
            filtered.register(service.clone());
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
    let mut reg = ToolRegistry::new();

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
        let meta = labby_apis::doctor::META;
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
        let meta = labby_apis::setup::META;
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

    reg.register(RegisteredService {
        name: "logs",
        description: "Search and stream local-master runtime logs",
        category: "bootstrap",
        kind: RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::dispatch::logs::ACTIONS,
        dispatch: dispatch_fn!(crate::dispatch::logs::dispatch),
    });

    #[cfg(feature = "gateway")]
    reg.register(RegisteredService::bootstrap_operator(
        "snippets",
        "Manage executable Code Mode snippets",
        "bootstrap",
        crate::dispatch::snippets::ACTIONS,
        dispatch_fn!(crate::dispatch::snippets::dispatch),
    ));

    reg.register(RegisteredService {
        name: "device",
        description: "Manage fleet device enrollments",
        category: "bootstrap",
        kind: RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::mcp::services::nodes::ACTIONS,
        dispatch: dispatch_fn!(crate::mcp::services::nodes::dispatch),
    });

    #[cfg(feature = "marketplace")]
    {
        let meta = labby_apis::marketplace::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
            kind: registered_service_kind(meta.name, meta.category),
            status: "available",
            actions: crate::dispatch::marketplace::actions(),
            dispatch: dispatch_fn!(crate::dispatch::marketplace::dispatch),
        });
    }

    // acp is feature-gated: chat/session runtime for the web UI (required by marketplace).
    #[cfg(feature = "acp")]
    {
        let meta = labby_apis::acp::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
            kind: registered_service_kind(meta.name, meta.category),
            status: "available",
            actions: crate::dispatch::acp::catalog::ACTIONS,
            dispatch: dispatch_fn!(crate::dispatch::acp::dispatch::dispatch),
        });
    }

    // stash is feature-gated: versioned component snapshots (required by marketplace).
    #[cfg(feature = "stash")]
    {
        let meta = labby_apis::stash::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
            kind: registered_service_kind(meta.name, meta.category),
            status: "available",
            actions: crate::dispatch::stash::catalog::ACTIONS,
            dispatch: dispatch_fn!(crate::dispatch::stash::dispatch::dispatch),
        });
    }

    // Audit anchor: register_service!(reg, "beads"

    register_service!(
        reg,
        "deploy",
        deploy,
        actions = crate::mcp::services::deploy::ACTIONS,
        dispatch = dispatch_fn!(crate::mcp::services::deploy::dispatch)
    );

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
    // `fs` feature is enabled so the catalog and `lab help` stay discoverable;
    // runtime dispatch returns `workspace_not_configured` per-request when
    // the configured `workspace.root` cannot be resolved. `cli::serve` logs
    // invalid configuration as a warning once at boot.
    //
    // SECURITY: unlike `/v1/fs` (which refuses to mount when
    // `LAB_WEB_UI_AUTH_DISABLED=true`), MCP `fs` registration has no
    // env-driven refusal. MCP transport auth (`LAB_MCP_HTTP_TOKEN` /
    // OAuth, or stdio reachability) is the sole gate. See
    // `crates/lab/src/mcp/CLAUDE.md` § "Transport auth for fs".
    //
    // NOTE: fs has TWO action surfaces. The canonical slice is
    // `dispatch::fs::catalog::ACTIONS` (includes `fs.preview`); the MCP-filtered
    // slice `mcp::services::fs::ACTIONS` omits `fs.preview` because preview
    // streams raw bytes and is HTTP-only for prompt-injection reasons. The
    // registry uses the MCP slice because all current catalog consumers (MCP
    // `lab.help`, `lab://catalog`, CLI `lab help`) correctly treat preview as
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

    reg
}

#[must_use]
pub fn service_meta(name: &str) -> Option<&'static PluginMeta> {
    match name {
        #[cfg(feature = "deploy")]
        "deploy" => Some(&labby_apis::deploy::META),
        _ => None,
    }
}

/// Returns `true` when admin is enabled via `LAB_ADMIN_ENABLED=1` env var
/// or `admin.enabled = true` in config.toml (env var takes precedence).
#[cfg(feature = "lab-admin")]
fn lab_admin_enabled() -> bool {
    // Env var overrides config.toml.
    if let Ok(value) = std::env::var("LAB_ADMIN_ENABLED") {
        return value == "1";
    }
    // Fall back to config.toml — load is cheap (cached by the OS) and this
    // runs once at startup.
    crate::config::load_toml(&crate::config::toml_candidates())
        .map(|cfg| cfg.admin.enabled)
        .unwrap_or(false)
}

const fn category_slug(cat: labby_apis::core::Category) -> &'static str {
    use labby_apis::core::Category;
    match cat {
        Category::Media => "media",
        Category::Servarr => "servarr",
        Category::Indexer => "indexer",
        Category::Download => "download",
        Category::Notes => "notes",
        Category::Documents => "documents",
        Category::Network => "network",
        Category::Notifications => "notifications",
        Category::Ai => "ai",
        Category::Bootstrap => "bootstrap",
        Category::Marketplace => "marketplace",
    }
}

fn registered_service_kind(
    name: &'static str,
    category: labby_apis::core::Category,
) -> RegisteredServiceKind {
    use labby_apis::core::Category;

    if name == "beads" {
        return RegisteredServiceKind::BuiltInUpstreamApi;
    }

    if matches!(category, Category::Bootstrap | Category::Marketplace) {
        return RegisteredServiceKind::BootstrapOperator;
    }

    match name {
        "doctor" | "setup" | "marketplace" | "deploy" | "acp" | "stash" | "loggifly" => {
            RegisteredServiceKind::BootstrapOperator
        }
        _ => RegisteredServiceKind::BuiltInUpstreamApi,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RegisteredService, RegisteredServiceKind, ToolRegistry, build_default_registry,
        filter_built_in_upstream_apis, is_built_in_upstream_api_service, service_meta,
    };
    use labby_apis::core::action::ActionSpec;
    use serde_json::Value;
    use std::future::Future;
    use std::time::Duration;

    #[test]
    fn all_features_registers_all_services() {
        let reg = build_default_registry();
        let names: Vec<&str> = reg.services().iter().map(|s| s.name).collect();
        assert!(!names.contains(&"extract"), "extract has been retired");
        // feature-gated services — present only when the flag is enabled
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
    fn upstream_api_filter_is_noop_after_gateway_pivot() {
        // Post-pivot all surviving services are operator/bootstrap tools — there
        // are no `BuiltInUpstreamApi` services left. The filter is still wired
        // (kept for forward-compat with future plugin-based upstreams) but
        // currently filters nothing.
        let unfiltered = build_default_registry();
        let unfiltered_count = unfiltered.services().len();
        let filtered = filter_built_in_upstream_apis(unfiltered, false);
        assert_eq!(
            filtered.services().len(),
            unfiltered_count,
            "no upstream-API services remain to filter post-pivot"
        );
        let names: std::collections::BTreeSet<&str> = filtered
            .services()
            .iter()
            .map(|service| service.name)
            .collect();
        let mut kept_services = vec!["setup", "doctor"];
        #[cfg(feature = "acp")]
        kept_services.push("acp");
        #[cfg(feature = "stash")]
        kept_services.push("stash");
        #[cfg(feature = "gateway")]
        kept_services.push("gateway");
        #[cfg(feature = "marketplace")]
        kept_services.push("marketplace");
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

    #[cfg(not(feature = "gateway"))]
    #[test]
    fn default_registry_omits_gateway_without_feature() {
        let registry = build_default_registry();
        assert!(
            registry
                .services()
                .iter()
                .all(|service| service.name != "gateway")
        );
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn default_registry_includes_gateway_with_feature() {
        let registry = build_default_registry();
        assert!(
            registry
                .services()
                .iter()
                .any(|service| service.name == "gateway")
        );
    }

    #[cfg(not(feature = "marketplace"))]
    #[test]
    fn default_registry_omits_marketplace_without_feature() {
        let registry = build_default_registry();
        assert!(
            registry
                .services()
                .iter()
                .all(|service| service.name != "marketplace")
        );
    }

    #[cfg(feature = "marketplace")]
    #[test]
    fn default_registry_includes_marketplace_with_feature() {
        let registry = build_default_registry();
        assert!(
            registry
                .services()
                .iter()
                .any(|service| service.name == "marketplace")
        );
    }

    /// Guard that the MCP registry and the HTTP router mount identical service sets.
    ///
    /// Both sides are derived from the same authoritative source — `labby_apis::<service>::META.name`
    /// — guarded by the same `#[cfg(feature)]` attributes used in `build_default_registry()` and
    /// `build_router()`. Adding a new service only requires touching those two sites;
    /// this test self-updates through the shared feature flag.
    ///
    /// If this test fails, a service was registered in the MCP registry but not mounted in the
    /// HTTP router (or vice versa). Both must be updated together.
    #[test]
    fn registry_and_router_service_sets_are_identical() {
        // Derive the expected HTTP router service set from labby_apis META constants.
        // These are the same names used by build_router(), so any rename
        // in labby_apis automatically propagates here without manual updates.
        //
        // Assumption: every HTTP route mount uses exactly `META.name` as its path prefix.
        // If a service is added to build_router() under a different name than
        // META.name, that divergence will NOT be caught here. The trade-off is accepted:
        // the router consistently derives its path prefix from META.name, and if that
        // ever changes the build itself would break on the feature-gated import.
        let http_router_services: std::collections::HashSet<&'static str> = {
            let mut s = std::collections::HashSet::new();
            #[cfg(feature = "acp")]
            s.insert(labby_apis::acp::META.name);
            s.insert("device");
            #[cfg(feature = "gateway")]
            s.insert("gateway");
            #[cfg(feature = "gateway")]
            s.insert("snippets");
            s.insert("logs");
            #[cfg(feature = "marketplace")]
            s.insert(labby_apis::marketplace::META.name);
            s.insert(labby_apis::doctor::META.name); // always-on
            s.insert(labby_apis::setup::META.name); // always-on
            #[cfg(feature = "stash")]
            s.insert(labby_apis::stash::META.name);
            #[cfg(feature = "fs")]
            s.insert("fs");
            s
        };

        let reg = build_default_registry();
        let registry_services: std::collections::HashSet<&str> =
            reg.services().iter().map(|s| s.name).collect();

        let only_in_registry: Vec<&&str> = registry_services
            .iter()
            // lab_admin is MCP-only: no HTTP route by design (runtime opt-in via LAB_ADMIN_ENABLED=1)
            // deploy is MCP+CLI-only for V1; HTTP API surface is deferred (see docs/runtime/DEPLOY_SERVICE.md)
            .filter(|n| {
                !http_router_services.contains(**n) && **n != "lab_admin" && **n != "deploy"
            })
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
            name: "queue.list",
            description: "List queue",
            destructive: false,
            requires_admin: false,
            params: &[],
            returns: "object",
        },
        ActionSpec {
            name: "movie.search",
            description: "Search movies",
            destructive: false,
            requires_admin: false,
            params: &[],
            returns: "object",
        },
    ];

    const ACTIONS_TWO: &[ActionSpec] = &[
        ActionSpec {
            name: "movie.search",
            description: "Search movies again",
            destructive: false,
            requires_admin: false,
            params: &[],
            returns: "object",
        },
        ActionSpec {
            name: "calendar.list",
            description: "List calendar",
            destructive: false,
            requires_admin: false,
            params: &[],
            returns: "object",
        },
    ];

    fn noop_dispatch(
        _action: String,
        _params: Value,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>,
    > {
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
            &["calendar.list", "movie.search", "queue.list"]
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
            registry.action_name_completions("movie."),
            vec!["movie.search"]
        );
    }

    #[test]
    fn action_name_completions_empty_prefix_returns_all_actions_under_one_ms() {
        let registry = build_default_registry();
        let expected = registry.action_names().len();

        let start = std::time::Instant::now();
        let completions = registry.action_name_completions("");
        let elapsed = start.elapsed();

        assert_eq!(completions.len(), expected);
        assert!(
            elapsed < Duration::from_millis(1),
            "empty-prefix action completion took {elapsed:?} for {expected} cached actions"
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
