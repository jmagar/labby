//! `GET /v1/catalog` — filtered service+action catalog for the ⌘K palette.
//!
//! Returns the aggregated catalog serialized as JSON, filtered to only the
//! services present in `state.enabled_services`. Disabled services (missing
//! required env vars at startup) are not leaked.
//!
//! The response shape matches `lib/types/command-catalog.ts` in gateway-admin:
//! ```json
//! { "services": [{ "name": "radarr", "description": "...", "actions": [...] }] }
//! ```

use std::sync::OnceLock;
use std::time::Instant;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use serde_json::json;

use crate::api::state::AppState;

/// Startup nonce: nanoseconds since UNIX epoch, set once at first request.
static STARTUP_ID: OnceLock<String> = OnceLock::new();

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
pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/", get(get_catalog))
}

/// `GET /v1/catalog` — serializes the enabled-service slice of the startup catalog.
///
/// Includes `Cache-Control` and `ETag` headers so browsers and SWR clients can
/// skip redundant fetches.  The ETag is `"<startup_id>-<service_count>"` — cheap
/// to compute and changes on every server restart or service-set change.
/// Supports conditional `If-None-Match` requests; returns `304 Not Modified`
/// when the ETag matches.
async fn get_catalog(State(state): State<AppState>, req_headers: HeaderMap) -> impl IntoResponse {
    let start = Instant::now();

    tracing::info!(
        surface = "api",
        service = "catalog",
        action = "list",
        "dispatch start"
    );

    // Filter to only services present in enabled_services (those whose
    // required env vars were set at startup).
    let services: Vec<&crate::catalog::ServiceCatalog> = state
        .catalog
        .services
        .iter()
        .filter(|svc| state.enabled_services.contains(&svc.name))
        .collect();

    let etag = format!("\"{}-{}\"", startup_id(), services.len());

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
        count = services.len(),
        "dispatch ok"
    );

    (resp_headers, Json(json!({ "services": services }))).into_response()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

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
            requires_http_subject: false,
            actions: vec![ActionEntry {
                name: "queue.list".to_string(),
                description: "List queue".to_string(),
                destructive: false,
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
        super::routes(state.clone()).with_state(state)
    }

    #[tokio::test]
    async fn returns_only_enabled_services() {
        let state = test_state_with_catalog(
            vec![make_service("radarr"), make_service("sonarr")],
            HashSet::from(["radarr".to_string()]),
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
        assert_eq!(services[0]["name"], "radarr");
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
            vec![make_service("radarr")],
            HashSet::from(["radarr".to_string()]),
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
        assert_eq!(svc["name"], "radarr");
        assert!(svc["actions"].is_array());

        let action = &svc["actions"][0];
        assert_eq!(action["name"], "queue.list");
        assert_eq!(action["destructive"], false);
        assert!(action["params"].is_array());

        let param = &action["params"][0];
        assert_eq!(param["name"], "page");
        assert_eq!(param["ty"], "integer");
        assert_eq!(param["required"], false);
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
            vec![make_service("radarr")],
            HashSet::from(["radarr".to_string()]),
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
        // ETag format: "<startup_id>-<count>" (quoted, count=1 for one enabled service)
        assert!(
            etag.starts_with('"'),
            "ETag must be a quoted string, got: {etag}"
        );
        assert!(
            etag.ends_with("-1\""),
            "ETag should end with the service count, got: {etag}"
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
