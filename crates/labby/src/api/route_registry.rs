//! Runtime HTTP route inventory shared by router composition and generated docs.

use std::collections::BTreeSet;

use axum::{Router, routing::MethodRouter};
use serde::Serialize;

use super::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteAuth {
    Public,
    V1,
    BearerOnly,
    BrowserSession,
    BootstrapProof,
    OAuthProtocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutePresence {
    Static,
    RuntimeConditional,
    RuntimeCreated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteDescriptor {
    pub method: &'static str,
    pub path: String,
    pub handler: &'static str,
    pub mount: &'static str,
    pub auth: RouteAuth,
    pub feature: Option<&'static str>,
    pub runtime_condition: Option<&'static str>,
    pub presence: RoutePresence,
    pub aliases: Vec<String>,
    pub host_validation: bool,
    pub cache_posture: &'static str,
    pub failure_disclosure: &'static str,
    pub side_effects: &'static str,
}

impl RouteDescriptor {
    pub fn new(
        method: &'static str,
        path: impl Into<String>,
        handler: &'static str,
        mount: &'static str,
        auth: RouteAuth,
    ) -> Self {
        Self {
            method,
            path: path.into(),
            handler,
            mount,
            auth,
            feature: None,
            runtime_condition: None,
            presence: RoutePresence::Static,
            aliases: Vec::new(),
            host_validation: false,
            cache_posture: "route-defined",
            failure_disclosure: "standard",
            side_effects: if matches!(method, "GET" | "HEAD" | "OPTIONS") {
                "none_expected"
            } else {
                "possible"
            },
        }
    }

    #[must_use]
    pub fn feature(mut self, feature: &'static str) -> Self {
        self.feature = Some(feature);
        self
    }

    #[must_use]
    pub fn when(mut self, condition: &'static str) -> Self {
        self.runtime_condition = Some(condition);
        self.presence = RoutePresence::RuntimeConditional;
        self
    }

    #[must_use]
    pub fn runtime_created(mut self, condition: &'static str) -> Self {
        self.runtime_condition = Some(condition);
        self.presence = RoutePresence::RuntimeCreated;
        self
    }

    #[must_use]
    pub fn aliases(mut self, aliases: &[&str]) -> Self {
        self.aliases = aliases.iter().map(|alias| (*alias).to_string()).collect();
        self
    }

    #[must_use]
    pub fn host_validated(mut self) -> Self {
        self.host_validation = true;
        self
    }

    #[must_use]
    pub fn private_no_store(mut self) -> Self {
        self.cache_posture = "private, no-store";
        self
    }

    #[must_use]
    pub fn non_enumerating(mut self) -> Self {
        self.failure_disclosure = "uniform non-enumerating denial";
        self
    }

    #[must_use]
    pub fn side_effects(mut self, side_effects: &'static str) -> Self {
        self.side_effects = side_effects;
        self
    }
}

/// A locally cohesive router and the runtime-truth inventory for its routes.
pub struct RouteGroup {
    pub router: Router<AppState>,
    pub descriptors: Vec<RouteDescriptor>,
}

#[cfg(test)]
pub(crate) fn verify_auth_invariant(
    descriptor: &RouteDescriptor,
    status: axum::http::StatusCode,
) -> Result<(), String> {
    let protected = !matches!(
        descriptor.auth,
        RouteAuth::Public | RouteAuth::OAuthProtocol
    );
    if protected
        && !matches!(
            status,
            axum::http::StatusCode::UNAUTHORIZED
                | axum::http::StatusCode::FORBIDDEN
                | axum::http::StatusCode::NOT_FOUND
        )
    {
        return Err(format!(
            "protected route {} {} accepted an unauthenticated request with {status}",
            descriptor.method, descriptor.path
        ));
    }
    Ok(())
}

impl RouteGroup {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            router: Router::new(),
            descriptors: Vec::new(),
        }
    }

    #[must_use]
    pub fn route(
        mut self,
        descriptor: RouteDescriptor,
        method_router: MethodRouter<AppState>,
    ) -> Self {
        for alias in &descriptor.aliases {
            self.router = self.router.route(alias, method_router.clone());
        }
        self.router = self.router.route(&descriptor.path, method_router);
        self.descriptors.push(descriptor);
        validate_descriptors(&self.descriptors)
            .expect("route group contains conflicting descriptors");
        self
    }

    #[must_use]
    pub fn nest(mut self, prefix: &str, group: Self) -> Self {
        self.router = self.router.nest(prefix, group.router);
        self.descriptors.extend(prefixed(prefix, group.descriptors));
        validate_descriptors(&self.descriptors).expect("nested route group contains conflicts");
        self
    }

    #[must_use]
    pub fn merge(mut self, group: Self) -> Self {
        self.router = self.router.merge(group.router);
        self.descriptors.extend(group.descriptors);
        validate_descriptors(&self.descriptors).expect("merged route group contains conflicts");
        self
    }

    /// Merge a router supplied by another runtime subsystem. Static routes must
    /// use [`Self::route`], which couples every mount to one descriptor.
    #[must_use]
    pub fn merge_runtime_router(
        mut self,
        router: Router<AppState>,
        descriptors: impl IntoIterator<Item = RouteDescriptor>,
    ) -> Self {
        let descriptors = descriptors.into_iter().collect::<Vec<_>>();
        assert!(
            descriptors
                .iter()
                .all(|route| route.presence != RoutePresence::Static),
            "static routes cannot bypass RouteGroup::route"
        );
        self.router = self.router.merge(router);
        self.descriptors.extend(descriptors);
        validate_descriptors(&self.descriptors)
            .expect("runtime route group contains conflicting descriptors");
        self
    }

    #[must_use]
    pub fn map_router(self, map: impl FnOnce(Router<AppState>) -> Router<AppState>) -> Self {
        Self {
            router: map(self.router),
            descriptors: self.descriptors,
        }
    }
}

pub fn validate_mounted_inventory(
    mounted: &[RouteDescriptor],
    declared: &[RouteDescriptor],
) -> Result<(), String> {
    validate_descriptors(mounted)?;
    validate_descriptors(declared)?;
    let mounted_keys = expanded_keys(mounted);
    let declared_keys = expanded_keys(declared);
    if let Some(key) = mounted_keys.difference(&declared_keys).next() {
        return Err(format!(
            "mounted route missing from inventory: {} {}",
            key.0, key.1
        ));
    }
    let required = declared
        .iter()
        .filter(|route| route.presence == RoutePresence::Static)
        .flat_map(|route| {
            std::iter::once(&route.path)
                .chain(route.aliases.iter())
                .map(move |path| (route.method, path.clone()))
        })
        .collect::<BTreeSet<_>>();
    if let Some(key) = required.difference(&mounted_keys).next() {
        return Err(format!(
            "static inventory route is not mounted: {} {}",
            key.0, key.1
        ));
    }
    Ok(())
}

fn expanded_keys(descriptors: &[RouteDescriptor]) -> BTreeSet<(&'static str, String)> {
    descriptors
        .iter()
        .flat_map(|route| {
            std::iter::once(&route.path)
                .chain(route.aliases.iter())
                .map(move |path| (route.method, path.clone()))
        })
        .collect()
}

pub fn validate_descriptors(descriptors: &[RouteDescriptor]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for route in descriptors {
        for path in std::iter::once(&route.path).chain(route.aliases.iter()) {
            let key = (route.method, path.as_str());
            if !seen.insert(key) {
                return Err(format!(
                    "duplicate HTTP route descriptor: {} {path}",
                    route.method
                ));
            }
        }
    }
    Ok(())
}

pub fn prefixed(prefix: &str, descriptors: Vec<RouteDescriptor>) -> Vec<RouteDescriptor> {
    descriptors
        .into_iter()
        .map(|mut route| {
            route.path = join(prefix, &route.path);
            route.aliases = route
                .aliases
                .into_iter()
                .map(|alias| join(prefix, &alias))
                .collect();
            route
        })
        .collect()
}

fn join(prefix: &str, path: &str) -> String {
    if path == "/" {
        prefix.to_string()
    } else {
        format!("{}{path}", prefix.trim_end_matches('/'))
    }
}

/// Assemble the complete static route-pattern inventory from the same local
/// descriptor functions used by runtime route groups.
pub fn build_route_descriptors() -> Vec<RouteDescriptor> {
    use crate::app_manifest::{
        APPS_LAUNCHER_ROUTE, APPS_MANIFEST_API_ROUTE, LABBY_APP_HOST_JS_ROUTE,
        SERVER_LOGS_BROWSER_ROUTE,
    };

    let mut routes = vec![
        RouteDescriptor::new("GET", "/health", "health", "health", RouteAuth::Public),
        RouteDescriptor::new("GET", "/ready", "ready", "health", RouteAuth::Public),
        RouteDescriptor::new(
            "GET",
            "/.well-known/labby.json",
            "labby_discovery",
            "discovery",
            RouteAuth::Public,
        ),
        RouteDescriptor::new(
            "GET",
            concat!("/v1/", "{service}", "/actions"),
            "service_actions",
            "services",
            RouteAuth::V1,
        ),
        RouteDescriptor::new(
            "GET",
            APPS_MANIFEST_API_ROUTE,
            "apps_manifest",
            "apps",
            RouteAuth::V1,
        ),
        RouteDescriptor::new(
            "GET",
            LABBY_APP_HOST_JS_ROUTE,
            "labby_app_host_js",
            "apps",
            RouteAuth::Public,
        ),
        RouteDescriptor::new(
            "GET",
            "/auth/session",
            "auth_session",
            "oauth",
            RouteAuth::BrowserSession,
        ),
        RouteDescriptor::new(
            "POST",
            "/auth/logout",
            "auth_logout",
            "oauth",
            RouteAuth::BrowserSession,
        ),
        RouteDescriptor::new(
            "POST",
            "/auth/reauth",
            "reauth_start",
            "oauth",
            RouteAuth::BrowserSession,
        )
        .when("mounted only when credential authentication is configured"),
        RouteDescriptor::new(
            "GET",
            "/auth/reauth/{interaction}",
            "reauth_poll",
            "oauth",
            RouteAuth::BrowserSession,
        )
        .when("mounted only when credential authentication is configured"),
        RouteDescriptor::new(
            "DELETE",
            "/auth/reauth/{interaction}",
            "reauth_cancel",
            "oauth",
            RouteAuth::BrowserSession,
        )
        .when("mounted only when credential authentication is configured"),
        RouteDescriptor::new(
            "GET",
            "/auth/reauth/return",
            "reauth_return",
            "oauth",
            RouteAuth::Public,
        ),
        RouteDescriptor::new(
            "GET",
            APPS_LAUNCHER_ROUTE,
            "apps_launcher_page",
            "apps",
            RouteAuth::BrowserSession,
        )
        .aliases(&[&format!("{APPS_LAUNCHER_ROUTE}/")]),
        RouteDescriptor::new(
            "GET",
            SERVER_LOGS_BROWSER_ROUTE,
            "server_logs_app_page",
            "apps",
            RouteAuth::BrowserSession,
        )
        .aliases(&[&format!("{SERVER_LOGS_BROWSER_ROUTE}/")]),
        RouteDescriptor::new(
            "GET",
            "/dev/mockup",
            "dev_mockup",
            "dev",
            RouteAuth::BrowserSession,
        )
        .aliases(&["/dev/mockup/"])
        .when("development/mockup routes"),
        RouteDescriptor::new(
            "GET",
            "/dev/mockup/{name}",
            "dev_mockup_named",
            "dev",
            RouteAuth::BrowserSession,
        )
        .aliases(&["/dev/mockup/{name}/"])
        .when("development/mockup routes"),
        RouteDescriptor::new("POST", "/mcp", "mcp", "mcp", RouteAuth::BearerOnly)
            .when("mounted only when an MCP HTTP router is configured"),
        RouteDescriptor::new("GET", "/mcp", "mcp", "mcp", RouteAuth::BearerOnly)
            .when("mounted only when an MCP HTTP router is configured"),
    ];

    routes.extend(crate::api::services::oauth_relay::public_descriptors());
    routes.extend(crate::api::services::browser::public_descriptors());
    routes.extend(prefixed(
        "/v1/browser",
        crate::api::services::browser::descriptors(),
    ));
    routes.extend(prefixed(
        "/v1/integration",
        crate::api::services::integration_identity::descriptors(),
    ));
    routes.extend(prefixed(
        "/v1/oauth/relay",
        crate::api::services::oauth_relay::admin_descriptors(),
    ));
    routes.extend(prefixed(
        "/v1/catalog",
        crate::api::services::catalog::descriptors(),
    ));
    routes.extend(prefixed(
        "/v1/server_logs",
        crate::api::services::server_logs::descriptors(),
    ));
    routes.extend(prefixed(
        crate::app_manifest::SERVER_LOGS_DATA_API_PREFIX,
        crate::api::services::server_logs::data_descriptors(),
    ));
    routes.extend(prefixed(
        "/v1/doctor",
        crate::api::services::doctor::descriptors(),
    ));
    routes.extend(prefixed(
        "/v1/depot",
        crate::api::services::depot::descriptors(),
    ));
    routes.extend(prefixed(
        "/v1/setup",
        crate::api::services::setup::descriptors(),
    ));
    routes.extend(prefixed(
        "/v1/auth/allowed-emails",
        crate::api::services::auth_admin::descriptors(),
    ));
    routes.extend(prefixed(
        "/v1/access/bootstrap-owner",
        crate::api::services::access_bootstrap::descriptors(),
    ));
    routes.extend(prefixed(
        "/auth/bootstrap",
        crate::api::services::access_bootstrap_proof::descriptors(),
    ));
    routes.extend(crate::api::services::local_session::descriptors());
    routes.extend(prefixed(
        "/v1/access/credentials",
        crate::api::services::access_credentials::descriptors(),
    ));

    #[cfg(feature = "skills")]
    routes.extend(prefixed(
        "/v1/artifacts",
        crate::api::services::skills::descriptors(),
    ));
    #[cfg(feature = "skills")]
    for service in ["bundles", "jobs", "sources", "uploads"] {
        routes.extend(prefixed(
            &format!("/v1/{service}"),
            crate::api::services::remote_control::descriptors(service),
        ));
    }
    #[cfg(feature = "fs")]
    routes.extend(prefixed("/v1/fs", crate::api::services::fs::descriptors()));
    routes.extend(prefixed(
        "/v1/stash",
        crate::api::services::file_stash::descriptors(),
    ));
    #[cfg(feature = "gateway")]
    {
        routes.extend(prefixed(
            "/v1/gateway",
            crate::api::services::gateway::descriptors(),
        ));
        routes.extend(prefixed(
            "/v1/snippets",
            crate::api::services::snippets::descriptors(),
        ));
        routes.extend(prefixed(
            "/v1/palette",
            crate::api::services::palette::descriptors(),
        ));
        routes.extend(prefixed(
            "/v1/gateway/oauth",
            crate::api::upstream_oauth::gateway_descriptors(),
        ));
        routes.extend(crate::api::upstream_oauth::browser_descriptors());
        routes.extend(crate::api::upstream_oauth::well_known_descriptors());
        for method in ["GET", "POST", "DELETE"] {
            routes.push(
                RouteDescriptor::new(
                    method,
                    "/{runtime_protected_mcp_route}",
                    "protected_mcp_intercept",
                    "protected_mcp",
                    RouteAuth::BearerOnly,
                )
                .feature("gateway")
                .runtime_created(
                    "one concrete instance is created for each enabled protected MCP route",
                ),
            );
        }
    }

    #[cfg(feature = "api-docs")]
    routes.extend([
        RouteDescriptor::new(
            "GET",
            "/v1/openapi.json",
            "openapi",
            "openapi",
            RouteAuth::V1,
        )
        .feature("api-docs"),
        RouteDescriptor::new("GET", "/v1/docs", "openapi_docs", "openapi", RouteAuth::V1)
            .feature("api-docs"),
    ]);

    routes.extend(oauth_protocol_descriptors());
    routes.sort_by(|a, b| (a.path.as_str(), a.method).cmp(&(b.path.as_str(), b.method)));
    validate_descriptors(&routes).expect("global route inventory contains conflicts");
    routes
}

/// Inventory for the sealed Core-to-Labby Unix-socket profile.
///
/// Core owns browser navigation, sessions, OAuth hand-offs, and the native
/// Gateway admin UI in this deployment. Labby therefore exposes only its
/// service/API surface behind a fresh delegated-actor assertion; it must not
/// accidentally retain a second standalone web or OAuth entry point.
pub fn build_integrated_trusted_host_route_descriptors() -> Vec<RouteDescriptor> {
    build_route_descriptors()
        .into_iter()
        .filter(|route| {
            !matches!(
                route.mount,
                "browser"
                    | "oauth"
                    | "dev"
                    | "mcp"
                    | "oauth_relay"
                    | "protected_mcp"
                    | "upstream_oauth"
                    | "integration"
            ) && !matches!(
                route.handler,
                "labby_app_host_js"
                    | "apps_launcher_page"
                    | "server_logs_app_page"
                    | "local_session_create"
                    | "local_session_logout"
            )
        })
        .collect()
}

pub(crate) fn oauth_protocol_descriptors() -> Vec<RouteDescriptor> {
    [
        labby_auth::config::InboundProviderKind::Google,
        labby_auth::config::InboundProviderKind::Authelia,
    ]
    .into_iter()
    .flat_map(oauth_protocol_descriptors_for_provider)
    .fold(Vec::new(), |mut routes, route| {
        if !routes.iter().any(|existing: &RouteDescriptor| {
            existing.method == route.method && existing.path == route.path
        }) {
            routes.push(route);
        }
        routes
    })
    .into_iter()
    .chain({
        #[cfg(feature = "gateway")]
        {
            Some(
                RouteDescriptor::new(
                    "GET",
                    "/.well-known/oauth-protected-resource/{*route}",
                    "protected_route_resource_metadata",
                    "oauth",
                    RouteAuth::OAuthProtocol,
                )
                .feature("gateway")
                .when("mounted only when OAuth is configured"),
            )
        }
        #[cfg(not(feature = "gateway"))]
        {
            None
        }
    })
    .collect()
}

pub(crate) fn oauth_protocol_descriptors_for_provider(
    provider: labby_auth::config::InboundProviderKind,
) -> Vec<RouteDescriptor> {
    oauth_protocol_routes_for_provider(provider)
        .into_iter()
        .map(|(_, descriptor)| descriptor)
        .collect()
}

pub(crate) fn oauth_protocol_routes_for_provider(
    provider: labby_auth::config::InboundProviderKind,
) -> Vec<(labby_auth::routes::AuthRouteId, RouteDescriptor)> {
    use labby_auth::routes::AuthRouteId;
    labby_auth::routes::auth_route_specs(provider)
        .into_iter()
        .map(|spec| {
            let handler = match spec.id {
                AuthRouteId::AuthorizationServerMetadata
                | AuthRouteId::AuthorizationServerMetadataPath => {
                    "auth_authorization_server_metadata"
                }
                AuthRouteId::ProtectedResourceMetadata => "auth_protected_resource_metadata",
                AuthRouteId::Jwks => "auth_jwks",
                AuthRouteId::Register => "auth_register",
                AuthRouteId::Authorize => "auth_authorize",
                AuthRouteId::BrowserLogin => "auth_browser_login",
                AuthRouteId::ProviderCallback => "auth_callback",
                AuthRouteId::NativeCallback => "auth_native_callback",
                AuthRouteId::NativePoll => "auth_native_poll",
                AuthRouteId::Token => "auth_token",
                AuthRouteId::Revoke => "auth_revoke",
            };
            let condition = if spec.id == AuthRouteId::Register {
                "mounted only when OAuth is configured and dynamic client registration is enabled"
            } else if spec.id == AuthRouteId::ProviderCallback {
                match provider {
                    labby_auth::config::InboundProviderKind::Google => {
                        "mounted only when OAuth is configured with the Google provider"
                    }
                    labby_auth::config::InboundProviderKind::Authelia => {
                        "mounted only when OAuth is configured with the Authelia provider"
                    }
                }
            } else {
                "mounted only when OAuth is configured"
            };
            (
                spec.id,
                RouteDescriptor::new(
                    spec.method,
                    spec.path,
                    handler,
                    "oauth",
                    RouteAuth::OAuthProtocol,
                )
                .when(condition),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;

    #[test]
    fn route_builder_records_the_descriptor_for_the_route_it_mounts() {
        let descriptor = RouteDescriptor::new("GET", "/probe", "probe", "test", RouteAuth::Public);
        let group = RouteGroup::empty().route(descriptor.clone(), get(|| async {}));
        assert_eq!(group.descriptors, [descriptor]);
    }

    #[test]
    fn aliases_participate_in_duplicate_detection() {
        let routes = vec![
            RouteDescriptor::new("GET", "/one", "one", "test", RouteAuth::Public)
                .aliases(&["/alias"]),
            RouteDescriptor::new("GET", "/alias", "two", "test", RouteAuth::Public),
        ];
        assert!(validate_descriptors(&routes).is_err());
    }

    #[test]
    fn prefixes_canonical_paths_and_aliases() {
        let routes = prefixed(
            "/v1/example",
            vec![
                RouteDescriptor::new("GET", "/", "example", "test", RouteAuth::V1)
                    .aliases(&["/alternate"]),
            ],
        );
        assert_eq!(routes[0].path, "/v1/example");
        assert_eq!(routes[0].aliases, ["/v1/example/alternate"]);
    }

    #[test]
    fn trusted_host_inventory_excludes_standalone_identity_and_browser_surfaces() {
        let routes = build_integrated_trusted_host_route_descriptors();

        for path in [
            "/auth/session",
            "/auth/login",
            "/auth/local-session",
            "/auth/upstream/callback",
            "/callback/{machine_id}",
            "/mcp",
            "/dev/mockup",
            "/apps",
        ] {
            assert!(
                !routes.iter().any(|route| route.path == path),
                "sealed trusted-host inventory unexpectedly retained {path}"
            );
        }

        assert!(
            routes
                .iter()
                .any(|route| route.path == "/v1/gateway" && route.handler == "handle"),
            "Core must retain the authenticated management API surface"
        );
    }

    #[test]
    fn compiled_feature_shape_has_expected_conditional_groups() {
        let routes = build_route_descriptors();
        let paths = routes
            .iter()
            .map(|route| route.path.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            paths.contains("/v1/openapi.json"),
            cfg!(feature = "api-docs")
        );
        assert_eq!(paths.contains("/v1/fs/list"), cfg!(feature = "fs"));
        assert_eq!(paths.contains("/v1/artifacts"), cfg!(feature = "skills"));
        assert_eq!(paths.contains("/v1/gateway"), cfg!(feature = "gateway"));
        assert_eq!(
            paths.contains("/{runtime_protected_mcp_route}"),
            cfg!(feature = "gateway")
        );
    }

    #[test]
    fn mounted_inventory_detects_additions_and_removals() {
        let required =
            RouteDescriptor::new("GET", "/required", "required", "test", RouteAuth::Public);
        let extra = RouteDescriptor::new("POST", "/extra", "extra", "test", RouteAuth::Public);
        assert!(
            validate_mounted_inventory(&[], std::slice::from_ref(&required))
                .unwrap_err()
                .contains("not mounted")
        );
        assert!(
            validate_mounted_inventory(&[required, extra], &build_route_descriptors())
                .unwrap_err()
                .contains("missing from inventory")
        );
    }

    #[test]
    fn oauth_runtime_inventory_contains_only_selected_provider_callback() {
        for (provider, expected, absent) in [
            (
                labby_auth::config::InboundProviderKind::Google,
                "/auth/google/callback",
                "/auth/oidc/callback",
            ),
            (
                labby_auth::config::InboundProviderKind::Authelia,
                "/auth/oidc/callback",
                "/auth/google/callback",
            ),
        ] {
            let routes = oauth_protocol_descriptors_for_provider(provider);
            assert!(routes.iter().any(|route| route.path == expected));
            assert!(!routes.iter().any(|route| route.path == absent));
            assert_eq!(
                routes
                    .iter()
                    .filter(|route| route.handler == "auth_callback")
                    .count(),
                1
            );
            assert!(routes.iter().any(|route| route.path == "/native/callback"));
            assert!(routes.iter().any(|route| route.path == "/token"));
        }
    }
}
