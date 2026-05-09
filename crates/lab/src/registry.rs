//! Runtime tool registry. Services register themselves here during
//! startup; the MCP server walks the registry to expose tools and the
//! catalog module walks it to produce discovery docs.

use lab_apis::core::PluginMeta;
use lab_apis::core::action::ActionSpec;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use crate::dispatch::error::ToolError;

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
            let meta = lab_apis::$svc::META;
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
            let meta = lab_apis::$svc::META;
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

/// Runtime policy classification for registered services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisteredServiceKind {
    /// Local/bootstrap/operator surfaces that do not proxy a built-in upstream API.
    BootstrapOperator,
    /// Built-in integrations that call an external service API.
    BuiltInUpstreamApi,
}

/// Collection of registered services, built at startup.
#[derive(Debug, Default)]
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

const ALWAYS_VISIBLE_SERVICES: &[&str] = &[
    "init",
    "setup",
    "doctor",
    "plugins",
    "extract",
    "gateway",
    "help",
    "completions",
    "scaffold",
    "audit",
    "marketplace",
    "logs",
    "device",
    "acp",
    "stash",
];

#[must_use]
pub fn lab_show_all_enabled() -> bool {
    std::env::var("LAB_SHOW_ALL")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
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

    // extract is always-on (no feature flag).
    {
        let meta = lab_apis::extract::META;
        let actions: &'static [ActionSpec] = crate::dispatch::extract::ACTIONS;
        reg.register(RegisteredService {
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
            dispatch: dispatch_fn!(crate::dispatch::extract::dispatch),
        });
    }

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
        let meta = lab_apis::doctor::META;
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
        let meta = lab_apis::setup::META;
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

    reg.register(RegisteredService {
        name: "device",
        description: "Manage fleet device enrollments",
        category: "bootstrap",
        kind: RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::mcp::services::nodes::ACTIONS,
        dispatch: dispatch_fn!(crate::mcp::services::nodes::dispatch),
    });

    // marketplace is always-on (synthetic service, no feature flag).
    {
        let meta = lab_apis::marketplace::META;
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

    // acp is always-on (no feature flag). MCP and CLI surfaces are Phase 2.
    {
        let meta = lab_apis::acp::META;
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

    // stash is always-on (no feature flag). Manages versioned component snapshots.
    {
        let meta = lab_apis::stash::META;
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

    register_service!(
        reg,
        "radarr",
        radarr,
        actions = crate::dispatch::radarr::actions(),
        dispatch = dispatch_fn!(crate::dispatch::radarr::dispatch)
    );

    register_service!(reg, "sonarr", sonarr);

    register_service!(
        reg,
        "prowlarr",
        prowlarr,
        actions = crate::dispatch::prowlarr::ACTIONS,
        dispatch = dispatch_fn!(crate::dispatch::prowlarr::dispatch)
    );

    register_service!(
        reg,
        "plex",
        plex,
        actions = crate::dispatch::plex::ACTIONS,
        dispatch = dispatch_fn!(crate::dispatch::plex::dispatch)
    );
    register_service!(
        reg,
        "tautulli",
        tautulli,
        actions = crate::dispatch::tautulli::ACTIONS,
        dispatch = dispatch_fn!(crate::dispatch::tautulli::dispatch)
    );

    register_service!(
        reg,
        "sabnzbd",
        sabnzbd,
        actions = crate::dispatch::sabnzbd::ACTIONS,
        dispatch = dispatch_fn!(crate::dispatch::sabnzbd::dispatch)
    );

    register_service!(reg, "qbittorrent", qbittorrent);
    register_service!(
        reg,
        "tailscale",
        tailscale,
        actions = crate::dispatch::tailscale::ACTIONS,
        dispatch = dispatch_fn!(crate::dispatch::tailscale::dispatch)
    );
    register_service!(
        reg,
        "linkding",
        linkding,
        actions = crate::dispatch::linkding::ACTIONS,
        dispatch = dispatch_fn!(crate::dispatch::linkding::dispatch)
    );
    register_service!(reg, "memos", memos);

    register_service!(
        reg,
        "bytestash",
        bytestash,
        actions = crate::dispatch::bytestash::ACTIONS,
        dispatch = dispatch_fn!(crate::dispatch::bytestash::dispatch)
    );
    // Audit anchor: register_service!(reg, "beads"
    register_service!(
        reg,
        "beads",
        beads,
        actions = crate::dispatch::beads::ACTIONS,
        dispatch = dispatch_fn!(crate::dispatch::beads::dispatch)
    );

    register_service!(
        reg,
        "paperless",
        paperless,
        actions = crate::dispatch::paperless::ACTIONS,
        dispatch = dispatch_fn!(crate::dispatch::paperless::dispatch)
    );
    register_service!(reg, "arcane", arcane);

    register_service!(
        reg,
        "unraid",
        unraid,
        actions = crate::dispatch::unraid::ACTIONS,
        dispatch = dispatch_fn!(crate::dispatch::unraid::dispatch)
    );

    register_service!(
        reg,
        "unifi",
        unifi,
        actions = crate::dispatch::unifi::actions(),
        dispatch = dispatch_fn!(crate::dispatch::unifi::dispatch)
    );

    register_service!(reg, "overseerr", overseerr);
    register_service!(reg, "gotify", gotify);
    register_service!(reg, "openacp", openacp);
    register_service!(reg, "openai", openai);
    register_service!(reg, "notebooklm", notebooklm);
    register_service!(reg, "qdrant", qdrant);
    register_service!(reg, "tei", tei);
    register_service!(reg, "apprise", apprise);
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
    reg.register(RegisteredService {
        name: "fs",
        description: "Workspace filesystem browser (read-only, deny-listed)",
        category: "bootstrap",
        kind: RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::mcp::services::fs::ACTIONS,
        dispatch: dispatch_fn!(crate::mcp::services::fs::dispatch),
    });

    register_service!(reg, "dozzle", dozzle);

    register_service!(reg, "immich", immich);

    register_service!(reg, "jellyfin", jellyfin);

    register_service!(reg, "navidrome", navidrome);

    register_service!(reg, "scrutiny", scrutiny);

    register_service!(reg, "freshrss", freshrss);

    register_service!(reg, "loggifly", loggifly);

    register_service!(reg, "adguard", adguard);

    register_service!(reg, "glances", glances);

    register_service!(reg, "uptime_kuma", uptime_kuma);

    register_service!(reg, "pihole", pihole);

    register_service!(reg, "neo4j", neo4j);

    reg
}

#[must_use]
pub fn service_meta(name: &str) -> Option<&'static PluginMeta> {
    match name {
        #[cfg(feature = "radarr")]
        "radarr" => Some(&lab_apis::radarr::META),
        #[cfg(feature = "sonarr")]
        "sonarr" => Some(&lab_apis::sonarr::META),
        #[cfg(feature = "prowlarr")]
        "prowlarr" => Some(&lab_apis::prowlarr::META),
        #[cfg(feature = "plex")]
        "plex" => Some(&lab_apis::plex::META),
        #[cfg(feature = "jellyfin")]
        "jellyfin" => Some(&lab_apis::jellyfin::META),
        #[cfg(feature = "tautulli")]
        "tautulli" => Some(&lab_apis::tautulli::META),
        #[cfg(feature = "sabnzbd")]
        "sabnzbd" => Some(&lab_apis::sabnzbd::META),
        #[cfg(feature = "qbittorrent")]
        "qbittorrent" => Some(&lab_apis::qbittorrent::META),
        #[cfg(feature = "tailscale")]
        "tailscale" => Some(&lab_apis::tailscale::META),
        #[cfg(feature = "linkding")]
        "linkding" => Some(&lab_apis::linkding::META),
        #[cfg(feature = "memos")]
        "memos" => Some(&lab_apis::memos::META),
        #[cfg(feature = "bytestash")]
        "bytestash" => Some(&lab_apis::bytestash::META),
        #[cfg(feature = "beads")]
        "beads" => Some(&lab_apis::beads::META),
        #[cfg(feature = "paperless")]
        "paperless" => Some(&lab_apis::paperless::META),
        #[cfg(feature = "arcane")]
        "arcane" => Some(&lab_apis::arcane::META),
        #[cfg(feature = "unraid")]
        "unraid" => Some(&lab_apis::unraid::META),
        #[cfg(feature = "unifi")]
        "unifi" => Some(&lab_apis::unifi::META),
        #[cfg(feature = "overseerr")]
        "overseerr" => Some(&lab_apis::overseerr::META),
        #[cfg(feature = "gotify")]
        "gotify" => Some(&lab_apis::gotify::META),
        #[cfg(feature = "openacp")]
        "openacp" => Some(&lab_apis::openacp::META),
        #[cfg(feature = "openai")]
        "openai" => Some(&lab_apis::openai::META),
        #[cfg(feature = "notebooklm")]
        "notebooklm" => Some(&lab_apis::notebooklm::META),
        #[cfg(feature = "qdrant")]
        "qdrant" => Some(&lab_apis::qdrant::META),
        #[cfg(feature = "tei")]
        "tei" => Some(&lab_apis::tei::META),
        #[cfg(feature = "apprise")]
        "apprise" => Some(&lab_apis::apprise::META),
        #[cfg(feature = "deploy")]
        "deploy" => Some(&lab_apis::deploy::META),
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

const fn category_slug(cat: lab_apis::core::Category) -> &'static str {
    use lab_apis::core::Category;
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
    _category: lab_apis::core::Category,
) -> RegisteredServiceKind {
    match name {
        "extract" | "doctor" | "setup" | "marketplace" | "beads" | "deploy" | "acp" | "stash"
        | "loggifly" => RegisteredServiceKind::BootstrapOperator,
        _ => RegisteredServiceKind::BuiltInUpstreamApi,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RegisteredService, RegisteredServiceKind, ToolRegistry, build_default_registry,
        filter_built_in_upstream_apis, is_built_in_upstream_api_service, service_meta,
    };
    use lab_apis::core::action::ActionSpec;
    use serde_json::Value;
    use std::future::Future;
    use std::time::Duration;

    #[test]
    fn extract_is_always_registered() {
        let reg = build_default_registry();
        assert!(
            reg.services().iter().any(|s| s.name == "extract"),
            "extract must be in the default registry"
        );
    }

    #[test]
    fn all_features_registers_all_services() {
        let reg = build_default_registry();
        let names: Vec<&str> = reg.services().iter().map(|s| s.name).collect();
        // extract is always-on (no feature flag)
        assert!(names.contains(&"extract"), "extract missing");
        // feature-gated services — present only when the flag is enabled
        #[cfg(feature = "radarr")]
        assert!(names.contains(&"radarr"), "radarr missing");
        #[cfg(feature = "sonarr")]
        assert!(names.contains(&"sonarr"), "sonarr missing");
        #[cfg(feature = "prowlarr")]
        assert!(names.contains(&"prowlarr"), "prowlarr missing");
        #[cfg(feature = "plex")]
        assert!(names.contains(&"plex"), "plex missing");
        #[cfg(feature = "tautulli")]
        assert!(names.contains(&"tautulli"), "tautulli missing");
        #[cfg(feature = "sabnzbd")]
        assert!(names.contains(&"sabnzbd"), "sabnzbd missing");
        #[cfg(feature = "qbittorrent")]
        assert!(names.contains(&"qbittorrent"), "qbittorrent missing");
        #[cfg(feature = "tailscale")]
        assert!(names.contains(&"tailscale"), "tailscale missing");
        #[cfg(feature = "linkding")]
        assert!(names.contains(&"linkding"), "linkding missing");
        #[cfg(feature = "memos")]
        assert!(names.contains(&"memos"), "memos missing");
        #[cfg(feature = "bytestash")]
        assert!(names.contains(&"bytestash"), "bytestash missing");
        #[cfg(feature = "beads")]
        assert!(names.contains(&"beads"), "beads missing");
        #[cfg(feature = "paperless")]
        assert!(names.contains(&"paperless"), "paperless missing");
        #[cfg(feature = "arcane")]
        assert!(names.contains(&"arcane"), "arcane missing");
        #[cfg(feature = "unraid")]
        assert!(names.contains(&"unraid"), "unraid missing");
        #[cfg(feature = "unifi")]
        assert!(names.contains(&"unifi"), "unifi missing");
        #[cfg(feature = "overseerr")]
        assert!(names.contains(&"overseerr"), "overseerr missing");
        #[cfg(feature = "gotify")]
        assert!(names.contains(&"gotify"), "gotify missing");
        #[cfg(feature = "jellyfin")]
        assert!(names.contains(&"jellyfin"), "jellyfin missing");
        #[cfg(feature = "openacp")]
        assert!(names.contains(&"openacp"), "openacp missing");
        #[cfg(feature = "openai")]
        assert!(names.contains(&"openai"), "openai missing");
        #[cfg(feature = "notebooklm")]
        assert!(names.contains(&"notebooklm"), "notebooklm missing");
        #[cfg(feature = "qdrant")]
        assert!(names.contains(&"qdrant"), "qdrant missing");
        #[cfg(feature = "tei")]
        assert!(names.contains(&"tei"), "tei missing");
        #[cfg(feature = "apprise")]
        assert!(names.contains(&"apprise"), "apprise missing");
    }

    #[test]
    fn bootstrap_services_are_not_built_in_upstream_apis() {
        for service in [
            "gateway",
            "setup",
            "doctor",
            "extract",
            "logs",
            "device",
            "marketplace",
            "acp",
            "stash",
            "deploy",
            "fs",
            "lab_admin",
            "beads",
            "loggifly",
        ] {
            assert!(
                !is_built_in_upstream_api_service(service),
                "{service} must remain available when upstream APIs are disabled"
            );
        }
    }

    #[test]
    fn upstream_api_filter_removes_upstreams_and_keeps_bootstrap() {
        let unfiltered = build_default_registry();
        let unfiltered_names: std::collections::BTreeSet<&str> = unfiltered
            .services()
            .iter()
            .map(|service| service.name)
            .collect();
        let removed_services = [
            "radarr",
            "sonarr",
            "tailscale",
            "openai",
            "uptime-kuma",
            "dozzle",
        ];
        for removed in removed_services {
            assert!(
                unfiltered_names.contains(removed),
                "{removed} should exist in the unfiltered registry"
            );
        }

        let reg = filter_built_in_upstream_apis(unfiltered, false);
        let names: std::collections::BTreeSet<&str> =
            reg.services().iter().map(|service| service.name).collect();

        for removed in removed_services {
            assert!(!names.contains(removed), "{removed} should be disabled");
        }

        for kept in [
            "setup",
            "doctor",
            "extract",
            "gateway",
            "marketplace",
            "acp",
            "stash",
        ] {
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
        assert!(
            reg.services()
                .iter()
                .any(|service| service.kind == RegisteredServiceKind::BuiltInUpstreamApi),
            "registry should include upstream API services in all-features builds"
        );
        assert!(
            reg.services()
                .iter()
                .any(|service| service.kind == RegisteredServiceKind::BootstrapOperator),
            "registry should include bootstrap/operator services"
        );
    }

    #[test]
    fn service_meta_tracks_feature_enabled_services() {
        #[cfg(feature = "plex")]
        assert_eq!(service_meta("plex").map(|meta| meta.name), Some("plex"));
        assert!(service_meta("extract").is_none());
        assert!(service_meta("gateway").is_none());
    }

    /// Guard that the MCP registry and the HTTP router mount identical service sets.
    ///
    /// Both sides are derived from the same authoritative source — `lab_apis::<service>::META.name`
    /// — guarded by the same `#[cfg(feature)]` attributes used in `build_default_registry()` and
    /// `build_router()`. Adding a new service only requires touching those two sites;
    /// this test self-updates through the shared feature flag.
    ///
    /// If this test fails, a service was registered in the MCP registry but not mounted in the
    /// HTTP router (or vice versa). Both must be updated together.
    #[test]
    fn registry_and_router_service_sets_are_identical() {
        // Derive the expected HTTP router service set from lab_apis META constants.
        // These are the same names used by build_router(), so any rename
        // in lab_apis automatically propagates here without manual updates.
        //
        // Assumption: every HTTP route mount uses exactly `META.name` as its path prefix.
        // If a service is added to build_router() under a different name than
        // META.name, that divergence will NOT be caught here. The trade-off is accepted:
        // the router consistently derives its path prefix from META.name, and if that
        // ever changes the build itself would break on the feature-gated import.
        let http_router_services: std::collections::HashSet<&'static str> = {
            let mut s = std::collections::HashSet::new();
            s.insert(lab_apis::extract::META.name); // always-on
            s.insert(lab_apis::acp::META.name); // always-on
            s.insert("device");
            s.insert("gateway");
            s.insert("logs");
            s.insert(lab_apis::marketplace::META.name); // always-on
            s.insert(lab_apis::doctor::META.name); // always-on
            s.insert(lab_apis::setup::META.name); // always-on
            s.insert(lab_apis::stash::META.name); // always-on
            #[cfg(feature = "radarr")]
            s.insert(lab_apis::radarr::META.name);
            #[cfg(feature = "sonarr")]
            s.insert(lab_apis::sonarr::META.name);
            #[cfg(feature = "prowlarr")]
            s.insert(lab_apis::prowlarr::META.name);
            #[cfg(feature = "plex")]
            s.insert(lab_apis::plex::META.name);
            #[cfg(feature = "tautulli")]
            s.insert(lab_apis::tautulli::META.name);
            #[cfg(feature = "sabnzbd")]
            s.insert(lab_apis::sabnzbd::META.name);
            #[cfg(feature = "qbittorrent")]
            s.insert(lab_apis::qbittorrent::META.name);
            #[cfg(feature = "tailscale")]
            s.insert(lab_apis::tailscale::META.name);
            #[cfg(feature = "linkding")]
            s.insert(lab_apis::linkding::META.name);
            #[cfg(feature = "memos")]
            s.insert(lab_apis::memos::META.name);
            #[cfg(feature = "bytestash")]
            s.insert(lab_apis::bytestash::META.name);
            #[cfg(feature = "beads")]
            s.insert(lab_apis::beads::META.name);
            #[cfg(feature = "paperless")]
            s.insert(lab_apis::paperless::META.name);
            #[cfg(feature = "arcane")]
            s.insert(lab_apis::arcane::META.name);
            #[cfg(feature = "unraid")]
            s.insert(lab_apis::unraid::META.name);
            #[cfg(feature = "unifi")]
            s.insert(lab_apis::unifi::META.name);
            #[cfg(feature = "overseerr")]
            s.insert(lab_apis::overseerr::META.name);
            #[cfg(feature = "gotify")]
            s.insert(lab_apis::gotify::META.name);
            #[cfg(feature = "openacp")]
            s.insert(lab_apis::openacp::META.name);
            #[cfg(feature = "openai")]
            s.insert(lab_apis::openai::META.name);
            #[cfg(feature = "notebooklm")]
            s.insert(lab_apis::notebooklm::META.name);
            #[cfg(feature = "qdrant")]
            s.insert(lab_apis::qdrant::META.name);
            #[cfg(feature = "tei")]
            s.insert(lab_apis::tei::META.name);
            #[cfg(feature = "apprise")]
            s.insert(lab_apis::apprise::META.name);
            #[cfg(feature = "dozzle")]
            s.insert(lab_apis::dozzle::META.name);
            #[cfg(feature = "immich")]
            s.insert(lab_apis::immich::META.name);
            #[cfg(feature = "jellyfin")]
            s.insert(lab_apis::jellyfin::META.name);
            #[cfg(feature = "navidrome")]
            s.insert(lab_apis::navidrome::META.name);
            #[cfg(feature = "scrutiny")]
            s.insert(lab_apis::scrutiny::META.name);
            #[cfg(feature = "freshrss")]
            s.insert(lab_apis::freshrss::META.name);
            #[cfg(feature = "loggifly")]
            s.insert(lab_apis::loggifly::META.name);
            #[cfg(feature = "adguard")]
            s.insert(lab_apis::adguard::META.name);
            #[cfg(feature = "glances")]
            s.insert(lab_apis::glances::META.name);
            #[cfg(feature = "uptime_kuma")]
            s.insert(lab_apis::uptime_kuma::META.name);
            #[cfg(feature = "pihole")]
            s.insert(lab_apis::pihole::META.name);
            #[cfg(feature = "neo4j")]
            s.insert(lab_apis::neo4j::META.name);
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

    #[tokio::test]
    async fn dispatch_fn_round_trips() {
        let reg = build_default_registry();
        let extract = reg
            .services()
            .iter()
            .find(|s| s.name == "extract")
            .expect("extract must be registered");
        let result = (extract.dispatch)("help".to_string(), serde_json::json!({})).await;
        assert!(result.is_ok(), "extract help dispatch should succeed");
    }

    const ACTIONS_ONE: &[ActionSpec] = &[
        ActionSpec {
            name: "queue.list",
            description: "List queue",
            destructive: false,
            params: &[],
            returns: "object",
        },
        ActionSpec {
            name: "movie.search",
            description: "Search movies",
            destructive: false,
            params: &[],
            returns: "object",
        },
    ];

    const ACTIONS_TWO: &[ActionSpec] = &[
        ActionSpec {
            name: "movie.search",
            description: "Search movies again",
            destructive: false,
            params: &[],
            returns: "object",
        },
        ActionSpec {
            name: "calendar.list",
            description: "List calendar",
            destructive: false,
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
}
