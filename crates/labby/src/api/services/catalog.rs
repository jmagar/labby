//! `GET /v1/catalog` — filtered service+action catalog for the ⌘K palette.
//!
//! Returns the aggregated catalog serialized as JSON, filtered to only the
//! services present in `state.enabled_services`. Disabled services (missing
//! required env vars at startup) are not leaked.
//!
//! The response shape matches `lib/types/command-catalog.ts` in gateway-admin:
//! ```json
//! { "services": [{ "name": "gateway-alpha", "description": "...", "actions": [...] }] }
//! ```

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use axum::{
    Json,
    body::Body,
    extract::{Extension, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use bytes::Bytes;
use serde_json::json;
use sha2::{Digest as _, Sha256};

use labby_primitives::product_credential::{BoundAccessGrant, ProductCredentialGrant};

use crate::api::state::AppState;

/// Startup nonce: nanoseconds since UNIX epoch, set once at first request.
static STARTUP_ID: OnceLock<String> = OnceLock::new();
const PROJECT_CATALOG_CACHE_CAPACITY: usize = 128;
const PROJECT_CATALOG_CACHE_TTL: Duration = Duration::from_mins(1);

#[derive(Clone)]
struct CatalogProjection {
    services: Vec<crate::catalog::ServiceCatalog>,
    body: Bytes,
    etag: String,
}

#[cfg(feature = "gateway")]
struct CachedProjectCatalog {
    manager: Weak<crate::dispatch::gateway::manager::GatewayManager>,
    key: String,
    expires_at: Instant,
    projection: CatalogProjection,
}

#[cfg(feature = "gateway")]
fn project_catalog_cache() -> &'static tokio::sync::Mutex<VecDeque<CachedProjectCatalog>> {
    static CACHE: OnceLock<tokio::sync::Mutex<VecDeque<CachedProjectCatalog>>> = OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::Mutex::new(VecDeque::new()))
}

#[cfg(feature = "gateway")]
fn project_catalog_build_lock() -> &'static tokio::sync::Mutex<()> {
    static BUILD_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    BUILD_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[cfg(feature = "gateway")]
async fn cached_project_catalog(
    manager: &Arc<crate::dispatch::gateway::manager::GatewayManager>,
    key: &str,
) -> Option<CatalogProjection> {
    let mut cache = project_catalog_cache().lock().await;
    let now = Instant::now();
    cache.retain(|entry| entry.expires_at > now && entry.manager.upgrade().is_some());
    let position = cache.iter().position(|entry| {
        entry.key == key
            && entry
                .manager
                .upgrade()
                .is_some_and(|cached| Arc::ptr_eq(&cached, manager))
    })?;
    let entry = cache.remove(position).expect("cache position exists");
    let projection = entry.projection.clone();
    cache.push_back(entry);
    Some(projection)
}

#[cfg(feature = "gateway")]
async fn store_project_catalog(
    manager: &Arc<crate::dispatch::gateway::manager::GatewayManager>,
    key: String,
    projection: CatalogProjection,
) {
    let mut cache = project_catalog_cache().lock().await;
    let now = Instant::now();
    cache.retain(|entry| {
        entry.expires_at > now
            && entry.manager.upgrade().is_some()
            && !(entry.key == key
                && entry
                    .manager
                    .upgrade()
                    .is_some_and(|cached| Arc::ptr_eq(&cached, manager)))
    });
    while cache.len() >= PROJECT_CATALOG_CACHE_CAPACITY {
        cache.pop_front();
    }
    cache.push_back(CachedProjectCatalog {
        manager: Arc::downgrade(manager),
        key,
        expires_at: now + PROJECT_CATALOG_CACHE_TTL,
        projection,
    });
}

fn startup_id() -> &'static str {
    STARTUP_ID.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_else(|_| "0".to_string())
    })
}

/// Returns true if `if_none_match` contains an entry matching `etag`.
///
/// Handles:
/// - Comma-separated ETag lists (e.g. `"abc", "def"`)
/// - Weak validators (`W/"abc"`)
/// - Quoted and unquoted forms
fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    // Strip outer quotes from our ETag for comparison.
    let bare_etag = etag.trim_matches('"');
    if_none_match.split(',').any(|candidate| {
        let stripped = candidate.trim().trim_start_matches("W/").trim_matches('"');
        stripped == bare_etag
    })
}

/// Register the catalog route: `GET /v1/catalog`.
pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    RouteGroup::empty().route(descriptors().remove(0), get(get_catalog))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![RouteDescriptor::new(
        "GET",
        "/",
        "get_catalog",
        "catalog",
        RouteAuth::V1,
    )]
}

/// `GET /v1/catalog` — serializes the enabled-service slice of the startup catalog.
///
/// Includes `Cache-Control` and `ETag` headers so browsers and SWR clients can
/// skip redundant fetches. The ETag binds the server startup and projected
/// response content, so equal-sized Loadouts cannot share stale validators.
/// Supports conditional `If-None-Match` requests; returns `304 Not Modified`
/// when the ETag matches.
async fn get_catalog(
    State(state): State<AppState>,
    bound: Option<Extension<BoundAccessGrant>>,
    source: Option<Extension<ProductCredentialGrant>>,
    req_headers: HeaderMap,
) -> impl IntoResponse {
    let start = Instant::now();

    tracing::info!(
        surface = "api",
        service = "catalog",
        action = "list",
        "dispatch start"
    );

    let projection = match bound {
        Some(Extension(bound)) => {
            let Some(Extension(source)) = source else {
                return catalog_error(StatusCode::UNAUTHORIZED, "catalog authorization denied");
            };
            match project_catalog(&state, &source, &bound).await {
                Ok(projection) => projection,
                Err(ProjectCatalogError::Denied) => {
                    return catalog_error(StatusCode::UNAUTHORIZED, "catalog authorization denied");
                }
                Err(ProjectCatalogError::Unavailable) => {
                    return catalog_error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "project catalog is unavailable",
                    );
                }
            }
        }
        None => catalog_projection(enabled_catalog(&state)),
    };
    let etag = projection.etag.clone();

    // Build shared response headers (used for both 200 and 304).
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        header::CACHE_CONTROL,
        "private, max-age=60, stale-while-revalidate=300"
            .parse()
            .expect("static Cache-Control value is always valid"),
    );
    resp_headers.insert(
        header::ETAG,
        etag.parse().expect("etag is always a valid header value"),
    );
    resp_headers.insert(
        header::VARY,
        "authorization, cookie"
            .parse()
            .expect("static Vary value is always valid"),
    );

    // Conditional GET: return 304 if the client already has this version.
    let client_etag = req_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !client_etag.is_empty() && etag_matches(client_etag, &etag) {
        return (StatusCode::NOT_MODIFIED, resp_headers).into_response();
    }

    tracing::info!(
        surface = "api",
        service = "catalog",
        action = "list",
        elapsed_ms = start.elapsed().as_millis(),
        count = projection.services.len(),
        "dispatch ok"
    );

    resp_headers.insert(
        header::CONTENT_TYPE,
        "application/json".parse().expect("static content type"),
    );
    (resp_headers, Body::from(projection.body)).into_response()
}

fn catalog_projection(services: Vec<crate::catalog::ServiceCatalog>) -> CatalogProjection {
    let body = serde_json::to_vec(&json!({ "services": services }))
        .expect("catalog types always serialize");
    let digest = hex::encode(Sha256::digest(&body));
    CatalogProjection {
        services,
        body: body.into(),
        etag: format!("\"{}-{}\"", startup_id(), &digest[..16]),
    }
}

fn enabled_catalog(state: &AppState) -> Vec<crate::catalog::ServiceCatalog> {
    state
        .catalog
        .services
        .iter()
        .filter(|service| state.enabled_services.contains(&service.name))
        .cloned()
        .collect()
}

fn catalog_error(status: StatusCode, message: &'static str) -> axum::response::Response {
    let kind = match status {
        StatusCode::SERVICE_UNAVAILABLE => "service_unavailable",
        StatusCode::INTERNAL_SERVER_ERROR => "internal",
        _ => "auth_failed",
    };
    (status, Json(json!({ "kind": kind, "message": message }))).into_response()
}

#[derive(Clone, Copy)]
enum ProjectCatalogError {
    Denied,
    Unavailable,
}

#[cfg(feature = "gateway")]
async fn project_catalog(
    state: &AppState,
    source: &ProductCredentialGrant,
    bound: &BoundAccessGrant,
) -> Result<CatalogProjection, ProjectCatalogError> {
    use labby_auth::{Authenticator, ProductAccessGrantResolver as _, VerifiedIdentity};

    use crate::access::{Permission, ProjectRuntimeMcpCatalogError};

    let Some(adapter) = state.access_credential_adapter.as_ref() else {
        return Err(ProjectCatalogError::Unavailable);
    };
    let Some(manager) = state.gateway_manager.as_ref() else {
        return Err(ProjectCatalogError::Unavailable);
    };
    let current = adapter.resolve(source).await.map_err(|error| match error {
        labby_primitives::product_credential::ProductCredentialVerificationError::Denied => {
            ProjectCatalogError::Denied
        }
        labby_primitives::product_credential::ProductCredentialVerificationError::Unavailable => {
            ProjectCatalogError::Unavailable
        }
    })?;
    if &current != bound {
        return Err(ProjectCatalogError::Denied);
    }
    let cache_key = project_catalog_cache_key(&current);
    if let Some(projection) = cached_project_catalog(manager, &cache_key).await {
        let after = adapter
            .resolve(source)
            .await
            .map_err(map_catalog_resolution_error)?;
        if after != current {
            return Err(ProjectCatalogError::Denied);
        }
        return Ok(projection);
    }
    // Serialize cache fills and recheck after waiting. This prevents a cold-key
    // request burst from duplicating the expensive runtime catalog projection.
    // Hits never wait on this lock.
    let _fill_guard = project_catalog_build_lock().lock().await;
    if let Some(projection) = cached_project_catalog(manager, &cache_key).await {
        let after = adapter
            .resolve(source)
            .await
            .map_err(map_catalog_resolution_error)?;
        if after != current {
            return Err(ProjectCatalogError::Denied);
        }
        return Ok(projection);
    }
    let identity = VerifiedIdentity::local_credential_with_issuer(
        Authenticator::ProductCredential,
        current.issuer.clone(),
        current.credential_id.clone(),
    )
    .map_err(|_| ProjectCatalogError::Denied)?;
    let context = crate::access::project_runtime_mcp_catalog_context(
        &state.access_runtime,
        manager,
        identity,
        current.project_id.clone(),
        Permission::AssetDiscover,
    )
    .await
    .map_err(|error| match error {
        ProjectRuntimeMcpCatalogError::ProjectAccessUnavailable => ProjectCatalogError::Denied,
        ProjectRuntimeMcpCatalogError::RuntimeUnavailable
        | ProjectRuntimeMcpCatalogError::AccessUnavailable
        | ProjectRuntimeMcpCatalogError::CatalogUnavailable
        | ProjectRuntimeMcpCatalogError::SnapshotUnstable => ProjectCatalogError::Unavailable,
    })?;
    let access = context.access();
    let same_principal = access.principal_id == current.principal_id;
    let same_organization = access.organization_id == current.organization_id;
    let same_project = access.project_id == current.project_id;
    let same_loadout = access.loadout_name == current.loadout_id;
    if !(same_principal && same_organization && same_project && same_loadout) {
        return Err(ProjectCatalogError::Denied);
    }
    let after = adapter
        .resolve(source)
        .await
        .map_err(map_catalog_resolution_error)?;
    if after != current {
        return Err(ProjectCatalogError::Denied);
    }
    let projection = catalog_projection(project_catalog_projection(
        state,
        &current.scopes,
        context
            .catalog()
            .services()
            .services()
            .iter()
            .map(|service| {
                (
                    service.name(),
                    service
                        .actions()
                        .iter()
                        .map(|action| action.name())
                        .collect::<HashSet<_>>(),
                )
            }),
    ));
    store_project_catalog(manager, cache_key, projection.clone()).await;
    Ok(projection)
}

#[cfg(feature = "gateway")]
fn map_catalog_resolution_error(
    error: labby_primitives::product_credential::ProductCredentialVerificationError,
) -> ProjectCatalogError {
    match error {
        labby_primitives::product_credential::ProductCredentialVerificationError::Denied => {
            ProjectCatalogError::Denied
        }
        labby_primitives::product_credential::ProductCredentialVerificationError::Unavailable => {
            ProjectCatalogError::Unavailable
        }
    }
}

#[cfg(feature = "gateway")]
fn project_catalog_cache_key(bound: &BoundAccessGrant) -> String {
    let mut scopes = bound.scopes.clone();
    scopes.sort_unstable();
    let material = json!({
        "installation": bound.installation_id,
        "issuer": bound.issuer,
        "subject": bound.subject,
        "principal": bound.principal_id,
        "organization": bound.organization_id,
        "project": bound.project_id,
        "loadout": bound.loadout_id,
        "loadout_generation": bound.loadout_generation,
        "assignment_generation": bound.assignment_generation,
        "catalog_generation": bound.catalog_generation,
        "route": bound.route_id,
        "route_generation": bound.route_generation,
        "membership_epoch": bound.membership_epoch,
        "organization_policy_epoch": bound.organization_policy_epoch,
        "project_policy_epoch": bound.project_policy_epoch,
        "credential": bound.credential_id,
        "credential_generation": bound.credential_generation,
        "scopes": scopes,
        "resource": bound.resource,
        "audience": bound.audience,
        "expires_at": bound.expires_at,
        "requires_admin": bound.requires_admin,
        "destructive": bound.destructive,
    });
    hex::encode(Sha256::digest(
        serde_json::to_vec(&material).expect("cache key material serializes"),
    ))
}

#[cfg(not(feature = "gateway"))]
async fn project_catalog(
    _state: &AppState,
    _source: &ProductCredentialGrant,
    _bound: &BoundAccessGrant,
) -> Result<CatalogProjection, ProjectCatalogError> {
    Err(ProjectCatalogError::Unavailable)
}

fn project_catalog_projection<'a>(
    state: &AppState,
    scopes: &[String],
    published: impl IntoIterator<Item = (&'a str, HashSet<&'a str>)>,
) -> Vec<crate::catalog::ServiceCatalog> {
    let published = published.into_iter().collect::<HashMap<_, _>>();
    let can_read = scopes
        .iter()
        .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin"));
    let is_admin = scopes.iter().any(|scope| scope == "lab:admin");
    state
        .catalog
        .services
        .iter()
        .filter_map(|service| {
            let actions = published.get(service.name.as_str())?;
            if !state.enabled_services.contains(&service.name) {
                return None;
            }
            let mut service = service.clone();
            service.actions.retain(|action| {
                actions.contains(action.name.as_str())
                    && can_read
                    && (!action.requires_admin || is_admin)
                    && action.required_scopes.iter().all(|required| {
                        scopes.iter().any(|scope| scope == required)
                            || (required == "lab:read"
                                && scopes
                                    .iter()
                                    .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin")))
                    })
            });
            Some(service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn concurrent_cache_fills_are_single_flight() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..32 {
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            tasks.spawn(async move {
                let _guard = super::project_catalog_build_lock().lock().await;
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(current, Ordering::SeqCst);
                tokio::task::yield_now().await;
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.expect("fill task");
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
    }

    use crate::api::router::build_router_with_bearer;
    use crate::api::state::AppState;
    use crate::catalog::{ActionEntry, Catalog, ParamEntry, ServiceCatalog};
    use crate::registry::ToolRegistry;

    /// Build a minimal `AppState` with a custom catalog and enabled-service set.
    fn test_state_with_catalog(
        services: Vec<ServiceCatalog>,
        enabled: HashSet<String>,
    ) -> AppState {
        let registry = ToolRegistry::new();
        let mut state = AppState::from_registry(registry);
        state.catalog = Arc::new(Catalog { services });
        state.enabled_services = Arc::new(enabled);
        state
    }

    fn make_service(name: &str) -> ServiceCatalog {
        ServiceCatalog {
            name: name.to_string(),
            description: format!("{name} service"),
            category: "Test".to_string(),
            status: "available".to_string(),
            caller_bound: false,
            requires_http_subject: false,
            actions: vec![ActionEntry {
                name: "health.list".to_string(),
                description: "List queue".to_string(),
                destructive: false,
                requires_admin: false,
                required_scopes: vec![],
                returns: "Queue[]".to_string(),
                params: vec![ParamEntry {
                    name: "page".to_string(),
                    ty: "integer".to_string(),
                    required: false,
                    description: "Page number".to_string(),
                }],
            }],
        }
    }

    fn catalog_router(state: AppState) -> axum::Router {
        // Route is registered as "/" inside `routes()` and is nested under
        // "/v1/catalog" in the full router (router.rs:985). Here we mount it
        // directly at "/" to keep the test helper simple; requests use "/".
        super::routes(state.clone()).router.with_state(state)
    }

    #[tokio::test]
    async fn returns_only_enabled_services() {
        let state = test_state_with_catalog(
            vec![
                make_service("gateway-alpha"),
                make_service("hidden-upstream"),
            ],
            HashSet::from(["gateway-alpha".to_string()]),
        );

        let response = catalog_router(state)
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::ACCEPT, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let services = value["services"].as_array().unwrap();
        assert_eq!(services.len(), 1, "only enabled services should appear");
        assert_eq!(services[0]["name"], "gateway-alpha");
    }

    #[tokio::test]
    async fn empty_catalog_returns_empty_array() {
        let state = test_state_with_catalog(vec![], HashSet::new());

        let response = catalog_router(state)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["services"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn response_shape_has_actions_with_params() {
        let state = test_state_with_catalog(
            vec![make_service("gateway-alpha")],
            HashSet::from(["gateway-alpha".to_string()]),
        );

        let response = catalog_router(state)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let svc = &value["services"][0];
        assert_eq!(svc["name"], "gateway-alpha");
        assert!(svc["actions"].is_array());

        let action = &svc["actions"][0];
        assert_eq!(action["name"], "health.list");
        assert_eq!(action["destructive"], false);
        assert!(action["params"].is_array());

        let param = &action["params"][0];
        assert_eq!(param["name"], "page");
        assert_eq!(param["ty"], "integer");
        assert_eq!(param["required"], false);
    }

    #[test]
    fn project_projection_intersects_enabled_published_services_and_actions() {
        let mut gateway = make_service("gateway");
        gateway.actions.push(ActionEntry {
            name: "admin.delete".to_string(),
            description: "admin".to_string(),
            destructive: true,
            requires_admin: true,
            required_scopes: vec!["lab:admin".to_string()],
            params: vec![],
            returns: "()".to_string(),
        });
        let state = test_state_with_catalog(
            vec![gateway, make_service("setup"), make_service("disabled")],
            HashSet::from(["gateway".to_string(), "setup".to_string()]),
        );

        let published = [
            ("gateway", HashSet::from(["health.list", "admin.delete"])),
            ("disabled", HashSet::from(["health.list"])),
        ];
        let projected =
            super::project_catalog_projection(&state, &["lab:read".to_string()], published.clone());

        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].name, "gateway");
        assert_eq!(projected[0].actions.len(), 1);
        assert_eq!(projected[0].actions[0].name, "health.list");
        assert!(!projected[0].actions[0].requires_admin);

        let execute =
            super::project_catalog_projection(&state, &["lab".to_string()], published.clone());
        assert_eq!(execute[0].actions.len(), 1);
        assert_eq!(execute[0].actions[0].name, "health.list");

        let admin =
            super::project_catalog_projection(&state, &["lab:admin".to_string()], published);
        assert_eq!(
            admin[0]
                .actions
                .iter()
                .map(|action| action.name.as_str())
                .collect::<Vec<_>>(),
            vec!["health.list", "admin.delete"]
        );
    }

    // ── Issue 5: auth gate ────────────────────────────────────────────────────
    //
    // `GET /v1/catalog` sits behind the bearer-token middleware added by
    // `build_router_with_bearer` (router.rs:985).  When a bearer token is
    // configured, unauthenticated requests must receive 401; authenticated
    // requests must reach the handler (200).
    //
    // These tests use the full router (via `build_router_with_bearer`) so the
    // middleware stack is exercised — the lightweight `catalog_router` helper
    // above bypasses auth intentionally.
    //
    // We drive the test via `GET /v1/catalog/actions`, which hits the shared
    // `/{service}/actions` route registered unconditionally in `build_v1_router`
    // (router.rs:984).  This route is inside the bearer-auth middleware and
    // exercises the same auth gate as `GET /v1/catalog/` — both are protected
    // by the same `route_layer(make_auth_layer(...))` that wraps the `/v1`
    // sub-router (router.rs:1442).

    #[tokio::test]
    async fn unauthenticated_request_returns_401() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/catalog/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "auth_failed");
    }

    #[tokio::test]
    async fn authenticated_request_reaches_catalog() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        // An authenticated request to any /v1/* endpoint must pass the auth layer.
        // We use /v1/catalog/actions (the shared service-actions route) so this
        // test is not affected by trailing-slash routing subtleties in axum nest.
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/catalog/actions")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // 200 (catalog actions listed) or 404 (no "catalog" in registry) — either
        // way, the auth layer must NOT block the request.
        assert_ne!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "authenticated request must not be blocked by auth layer"
        );
    }

    // ── Issue 10: Cache-Control and ETag ─────────────────────────────────────

    #[tokio::test]
    async fn response_includes_cache_control_header() {
        let state = test_state_with_catalog(vec![], HashSet::new());

        let response = catalog_router(state)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let cc = response
            .headers()
            .get(header::CACHE_CONTROL)
            .expect("Cache-Control header must be present")
            .to_str()
            .unwrap();
        assert!(
            cc.contains("private"),
            "Cache-Control should contain 'private'"
        );
        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "authorization, cookie"
        );
        assert!(
            cc.contains("max-age=60"),
            "Cache-Control should contain 'max-age=60'"
        );
        assert!(
            cc.contains("stale-while-revalidate=300"),
            "Cache-Control should contain 'stale-while-revalidate=300'"
        );
    }

    #[tokio::test]
    async fn response_includes_etag_header() {
        let state = test_state_with_catalog(
            vec![make_service("gateway-alpha")],
            HashSet::from(["gateway-alpha".to_string()]),
        );

        let response = catalog_router(state)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let etag = response
            .headers()
            .get(header::ETAG)
            .expect("ETag header must be present")
            .to_str()
            .unwrap();
        // ETag binds the server startup and projected response content without
        // embedding caller identity material.
        assert!(
            etag.starts_with('"'),
            "ETag must be a quoted string, got: {etag}"
        );
        let (_, fingerprint) = etag.trim_matches('"').rsplit_once('-').unwrap();
        assert_eq!(fingerprint.len(), 16);
        assert!(
            fingerprint
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
    }

    #[tokio::test]
    async fn if_none_match_matching_etag_returns_304() {
        let state = test_state_with_catalog(vec![], HashSet::new());

        // First request: obtain the ETag.
        let first = catalog_router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first
            .headers()
            .get(header::ETAG)
            .expect("ETag must be present on first response")
            .clone();

        // Second request: send the ETag back — expect 304.
        let second = catalog_router(state)
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::IF_NONE_MATCH, etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn if_none_match_stale_etag_returns_200() {
        let state = test_state_with_catalog(vec![], HashSet::new());

        let response = catalog_router(state)
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::IF_NONE_MATCH, "\"stale-etag-value\"")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
