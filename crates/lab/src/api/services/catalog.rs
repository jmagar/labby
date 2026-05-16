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

use std::time::Instant;

use axum::{Json, Router, extract::State, routing::get};
use serde_json::{Value, json};

use crate::api::state::AppState;

/// Register the catalog route: `GET /v1/catalog`.
pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/", get(get_catalog))
}

/// `GET /v1/catalog` — serializes the enabled-service slice of the startup catalog.
async fn get_catalog(State(state): State<AppState>) -> Json<Value> {
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

    tracing::info!(
        surface = "api",
        service = "catalog",
        action = "list",
        elapsed_ms = start.elapsed().as_millis(),
        count = services.len(),
        "dispatch ok"
    );

    Json(json!({ "services": services }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

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
        use axum::Router;
        Router::new()
            .nest("/catalog", super::routes(state.clone()))
            .with_state(state)
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
                    .uri("/catalog/")
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
            .oneshot(
                Request::builder()
                    .uri("/catalog/")
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
        assert_eq!(value["services"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn response_shape_has_actions_with_params() {
        let state = test_state_with_catalog(
            vec![make_service("radarr")],
            HashSet::from(["radarr".to_string()]),
        );

        let response = catalog_router(state)
            .oneshot(
                Request::builder()
                    .uri("/catalog/")
                    .body(Body::empty())
                    .unwrap(),
            )
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
}
