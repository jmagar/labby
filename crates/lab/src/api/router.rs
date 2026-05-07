//! Top-level axum router — mounts `POST /v1/<service>` for every enabled service
//! and the MCP streamable HTTP transport at `/mcp`.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderName, Request, StatusCode, header},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use tower_http::{
    compression::CompressionLayer,
    cors::{AllowOrigin, CorsLayer},
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use tracing::Level;

use lab_auth::AuthLayer;
use lab_auth::error::AuthError as LabAuthError;

/// Convert lab's strongly-typed [`crate::observability::activity::ActorKeyDeriver`]
/// into the closure-erased [`lab_auth::ActorKeyDeriver`] alias accepted by
/// [`AuthLayer::with_actor_key_deriver`]. Keeps the lab-specific HMAC actor-key
/// derivation while letting lab-auth stay agnostic about consumer-specific
/// observability hooks.
fn lab_auth_deriver(
    deriver: Arc<crate::observability::activity::ActorKeyDeriver>,
) -> Arc<lab_auth::ActorKeyDeriver> {
    Arc::new(move |subject: &str| {
        deriver
            .derive_subject(subject)
            .map(crate::observability::activity::ActorKey::into_arc)
    })
}

const DEV_MARKETPLACE_READ_ACTIONS: &[&str] = &[
    "help",
    "schema",
    "sources.list",
    "plugins.list",
    "plugin.get",
    "plugin.artifacts",
    "plugin.workspace",
    "plugin.components",
    "plugin.deploy.preview",
    "agent.list",
    "agent.get",
    "mcp.config",
    "mcp.list",
    "mcp.get",
    "mcp.versions",
    "mcp.meta.get",
];

use super::{health, services, state::AppState};
use crate::dispatch::error::ToolError;

fn app_auth_state(state: &AppState) -> Result<lab_auth::state::AuthState, LabAuthError> {
    state
        .oauth_state
        .as_ref()
        .map(|state| (**state).clone())
        .ok_or_else(|| LabAuthError::Config("oauth auth state is not configured".to_string()))
}

async fn auth_authorization_server_metadata(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::metadata::authorization_server_metadata(State(app_auth_state(&state)?)).await)
}

async fn auth_protected_resource_metadata(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::metadata::protected_resource_metadata(State(app_auth_state(&state)?)).await)
}

async fn auth_jwks(State(state): State<AppState>) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::metadata::jwks(State(app_auth_state(&state)?)).await)
}

async fn auth_register(
    State(state): State<AppState>,
    body: Json<lab_auth::types::ClientRegistrationRequest>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::authorize::register_client(State(app_auth_state(&state)?), body).await?)
}

async fn auth_authorize(
    State(state): State<AppState>,
    query: Query<lab_auth::types::AuthorizeQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::authorize::authorize(State(app_auth_state(&state)?), query).await?)
}

async fn auth_browser_login(
    State(state): State<AppState>,
    query: Query<lab_auth::types::BrowserLoginQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::authorize::browser_login(State(app_auth_state(&state)?), query).await?)
}

async fn auth_callback(
    State(state): State<AppState>,
    query: Query<lab_auth::types::CallbackQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::authorize::callback(State(app_auth_state(&state)?), query).await?)
}

async fn auth_token(
    State(state): State<AppState>,
    form: axum::extract::Form<lab_auth::types::TokenRequest>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::token::token(State(app_auth_state(&state)?), form).await?)
}

/// Build the `/v1` sub-router with all feature-gated service routes.
fn build_v1_router(state: &AppState, api_auth_configured: bool) -> Router<AppState> {
    let is_master = state.is_master();
    let openapi_spec: Arc<String> = super::openapi::build_openapi_spec(state.registry.services())
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to serialize OpenAPI spec");
            Arc::new(String::from(r#"{"error":"spec generation failed"}"#))
        });
    let spec_for_route = openapi_spec;

    let mut v1 = Router::new().nest("/nodes", super::nodes::routes(state.clone()));

    if is_master {
        v1 = v1.route("/{service}/actions", get(service_actions));

        // upstream oauth must be nested before /gateway so its more-specific prefix wins;
        // only mount when the gateway manager is present (oauth requires it).
        if state.gateway_manager.is_some() {
            v1 = v1.nest(
                "/gateway/oauth",
                crate::api::upstream_oauth::gateway_routes(state.clone()),
            );
        }

        v1 = v1
            .nest("/acp", services::acp::routes(state.clone()))
            .nest("/gateway", services::gateway::routes(state.clone()))
            .route(
                "/openapi.json",
                get(move || {
                    let spec = spec_for_route.clone();
                    async move {
                        (
                            [
                                (header::CONTENT_TYPE, "application/json"),
                                (header::CACHE_CONTROL, "private, no-store"),
                            ],
                            (*spec).clone(),
                        )
                    }
                }),
            )
            .route(
                "/docs",
                get(|| async { Html(include_str!("openapi_docs.html")) }),
            )
            // All v1 unauthenticated route groups (extract, marketplace,
            // doctor, setup) are gated by host_validation_layer — non-loopback
            // Host headers are rejected before reaching the dispatcher (DNS
            // rebinding mitigation for the v1 wizard, lab-bg3e.3.3).
            .nest(
                "/extract",
                services::extract::routes(state.clone()).layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                )),
            )
            .nest(
                "/marketplace",
                services::marketplace::routes(state.clone()).layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                )),
            )
            .nest(
                "/doctor",
                services::doctor::routes(state.clone()).layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                )),
            )
            .nest(
                "/setup",
                services::setup::routes(state.clone()).layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                )),
            )
            .nest("/stash", services::stash::routes(state.clone()))
            .nest(
                "/auth/allowed-emails",
                services::auth_admin::routes(state.clone()),
            );

        if state
            .registry
            .services()
            .iter()
            .any(|service| service.name == "logs")
        {
            v1 = v1.nest("/logs", services::logs::routes(state.clone()));
        }

        #[cfg(feature = "fs")]
        if state
            .registry
            .services()
            .iter()
            .any(|service| service.name == "fs")
        {
            // SECURITY: fs operations read workspace files, so refuse to
            // mount them on an unauthenticated API surface. Static web UI
            // auth settings do not bypass `/v1` auth when bearer/OAuth auth
            // is configured.
            if state.web_ui_auth_disabled && !api_auth_configured {
                tracing::warn!(
                    subsystem = "startup",
                    phase = "fs.mount.skipped",
                    reason = "web_ui_auth_disabled",
                    "fs service is configured but LAB_WEB_UI_AUTH_DISABLED=true would expose workspace files unauthenticated; refusing to mount /v1/fs"
                );
            } else {
                v1 = v1.nest("/fs", services::fs::routes(state.clone()));
            }
        }
    }

    macro_rules! mount_if_enabled {
        ($v1:ident, $state:ident, $feat:literal, $name:literal, $mod:ident) => {
            #[cfg(feature = $feat)]
            if $state.registry.services().iter().any(|s| s.name == $name) {
                $v1 = $v1.nest(concat!("/", $name), services::$mod::routes($state.clone()));
            }
        };
        ($v1:ident, $state:ident, $feat:literal, $registry_name:literal, $route_name:literal, $mod:ident) => {
            #[cfg(feature = $feat)]
            if $state
                .registry
                .services()
                .iter()
                .any(|s| s.name == $registry_name)
            {
                $v1 = $v1.nest(
                    concat!("/", $route_name),
                    services::$mod::routes($state.clone()),
                );
            }
        };
    }

    if is_master {
        mount_if_enabled!(v1, state, "radarr", "radarr", radarr);
        mount_if_enabled!(v1, state, "sonarr", "sonarr", sonarr);
        mount_if_enabled!(v1, state, "prowlarr", "prowlarr", prowlarr);
        mount_if_enabled!(v1, state, "plex", "plex", plex);
        mount_if_enabled!(v1, state, "tautulli", "tautulli", tautulli);
        mount_if_enabled!(v1, state, "sabnzbd", "sabnzbd", sabnzbd);
        mount_if_enabled!(v1, state, "qbittorrent", "qbittorrent", qbittorrent);
        mount_if_enabled!(v1, state, "tailscale", "tailscale", tailscale);
        mount_if_enabled!(v1, state, "linkding", "linkding", linkding);
        mount_if_enabled!(v1, state, "memos", "memos", memos);
        mount_if_enabled!(v1, state, "beads", "beads", beads);
        mount_if_enabled!(v1, state, "bytestash", "bytestash", bytestash);
        mount_if_enabled!(v1, state, "paperless", "paperless", paperless);
        mount_if_enabled!(v1, state, "arcane", "arcane", arcane);
        mount_if_enabled!(v1, state, "unraid", "unraid", unraid);
        mount_if_enabled!(v1, state, "unifi", "unifi", unifi);
        mount_if_enabled!(v1, state, "overseerr", "overseerr", overseerr);
        mount_if_enabled!(v1, state, "gotify", "gotify", gotify);
        mount_if_enabled!(v1, state, "openacp", "openacp", openacp);
        mount_if_enabled!(v1, state, "openai", "openai", openai);
        mount_if_enabled!(v1, state, "notebooklm", "notebooklm", notebooklm);
        mount_if_enabled!(v1, state, "qdrant", "qdrant", qdrant);
        mount_if_enabled!(v1, state, "tei", "tei", tei);
        mount_if_enabled!(v1, state, "apprise", "apprise", apprise);
        mount_if_enabled!(v1, state, "dozzle", "dozzle", dozzle);
        mount_if_enabled!(v1, state, "immich", "immich", immich);
        mount_if_enabled!(v1, state, "jellyfin", "jellyfin", jellyfin);
        mount_if_enabled!(v1, state, "navidrome", "navidrome", navidrome);
        mount_if_enabled!(v1, state, "scrutiny", "scrutiny", scrutiny);
        mount_if_enabled!(v1, state, "freshrss", "freshrss", freshrss);
        mount_if_enabled!(v1, state, "loggifly", "loggifly", loggifly);
        mount_if_enabled!(v1, state, "adguard", "adguard", adguard);
        mount_if_enabled!(v1, state, "glances", "glances", glances);
        mount_if_enabled!(
            v1,
            state,
            "uptime_kuma",
            "uptime-kuma",
            "uptime-kuma",
            uptime_kuma
        );
        mount_if_enabled!(v1, state, "pihole", "pihole", pihole);
        mount_if_enabled!(v1, state, "neo4j", "neo4j", neo4j);
        // [lab-scaffold: api-routes]
    }

    v1
}

/// Build the `/v0.1` sub-router with registry REST endpoints.
///
/// Auth middleware is applied via `route_layer()` — same pattern as `/v1`.
#[cfg(feature = "mcpregistry")]
fn build_v0_1_router() -> Router<AppState> {
    Router::new().nest("/v0.1", services::registry_v01::routes())
}

// ── Dev mockup file server ────────────────────────────────────────────────
// Implements the Tier 1 serving model from docs/design/component-development.md §5.
// Serves self-contained HTML from ~/.superpowers/brainstorm/content/ at:
//   GET /dev          → newest .html file
//   GET /dev/{name}   → newest .html whose stem contains {name}
//
// Rules (enforced by the doc — do not violate):
//   • These functions MUST live in router.rs alongside dev_marketplace_readonly.
//     The other Claude session strips dev-tooling code from web.rs.
//   • MUST NOT delegate to serve_web_request — that serves the Next.js SPA.
//   • Routes MUST be registered before the static-file fallback.

fn dev_mockup_dir() -> std::path::PathBuf {
    crate::config::home_dir()
        .map(|h| h.join(".superpowers/brainstorm/content"))
        .unwrap_or_else(|| std::path::PathBuf::from(".superpowers/brainstorm/content"))
}

fn dev_mockup_newest(fragment: Option<&str>) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dev_mockup_dir())
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("html"))
        .filter(|e| {
            fragment.is_none_or(|n| {
                e.path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.contains(n))
            })
        })
        .filter_map(|e| {
            e.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| (e.path(), t))
        })
        .max_by_key(|(_, t)| *t)
        .map(|(p, _)| p)
}

fn dev_mockup_response(fragment: Option<&str>) -> axum::response::Response {
    use axum::response::Html;
    match dev_mockup_newest(fragment) {
        None => {
            // Escape the fragment before embedding it in HTML to prevent XSS.
            // The name comes from a URL path segment and is user-controlled.
            let escaped = fragment
                .map(|n| {
                    format!(
                        " '{}'",
                        n.replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;")
                            .replace('"', "&quot;")
                    )
                })
                .unwrap_or_default();
            Html(format!(
                "<p style='font-family:sans-serif;padding:2rem'>No{escaped} mockup found in \
                 <code>~/.superpowers/brainstorm/content/</code></p>"
            ))
            .into_response()
        }
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(html) => Html(html).into_response(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read dev mockup");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
    }
}

async fn dev_mockup() -> axum::response::Response {
    dev_mockup_response(None)
}

async fn dev_mockup_named(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> axum::response::Response {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    dev_mockup_response(Some(&name))
}

// GET /dev/api/nodeinfo — unauthenticated, read-only.
// Returns config.toml values + ~/.lab/.env contents (secrets masked) so the
// setup wizard can pre-populate all fields without requiring a bearer token.
async fn dev_nodeinfo(State(state): State<AppState>) -> axum::response::Response {
    use axum::Json;

    let local_host =
        crate::node::identity::resolve_local_hostname().unwrap_or_else(|_| "local".into());
    let master_url = state
        .config
        .deploy
        .as_ref()
        .and_then(|d| d.defaults.as_ref())
        .and_then(|d| d.master_url.clone())
        .unwrap_or_default();
    let controller = state
        .config
        .node
        .as_ref()
        .and_then(|n| n.controller.clone())
        .unwrap_or_else(|| local_host.clone());

    // dotenvy already loaded ~/.lab/.env at startup, so everything is in std::env.
    // The UI treats MASKED_SECRET as "value already set — leave blank to keep current value".
    const MASKED_SECRET: &str = "***";
    let secret_suffixes = [
        // Deny-list for secret detection. Add new suffixes here when new secret
        // naming conventions are introduced (e.g. LAB_AUTH_SIGNING_KEY).
        // NOTE: `_KEY` intentionally covers `_API_KEY` and future signing-key vars.
        //       `_SECRET` covers `_CLIENT_SECRET` — the more-specific entry is omitted.
        "_KEY", // covers _API_KEY, _SIGNING_KEY, _HMAC_KEY, etc.
        "_TOKEN",
        "_PASSWORD",
        "_SECRET", // covers _CLIENT_SECRET
    ];
    let service_prefixes = [
        "RADARR_",
        "SONARR_",
        "PROWLARR_",
        "PLEX_",
        "TAUTULLI_",
        "OVERSEERR_",
        "SABNZBD_",
        "QBITTORRENT_",
        "UNRAID_",
        "UNIFI_",
        "TAILSCALE_",
        "ARCANE_",
        "LINKDING_",
        "MEMOS_",
        "PAPERLESS_",
        "BYTESTASH_",
        "GOTIFY_",
        "APPRISE_",
        "OPENAI_",
        "QDRANT_",
        "TEI_",
        "LAB_MCP_HTTP_",
        "LAB_LOG",
        "LAB_AUTH_",
        "LAB_PUBLIC_URL",
        "LAB_GOOGLE_",
    ];
    let mut env_values = serde_json::Map::new();
    for (key, val) in std::env::vars() {
        if val.is_empty() {
            continue;
        }
        if !service_prefixes.iter().any(|p| key.starts_with(p)) {
            continue;
        }
        let masked = secret_suffixes.iter().any(|s| key.ends_with(s));
        let display = if masked {
            MASKED_SECRET.to_string()
        } else {
            val
        };
        env_values.insert(key, serde_json::Value::String(display));
    }

    Json(serde_json::json!({
        "local_host": local_host,
        "controller": controller,
        "master_url": master_url,
        "env": env_values,
    }))
    .into_response()
}

async fn dev_marketplace_readonly(
    headers: axum::http::HeaderMap,
    Json(req): Json<crate::api::ActionRequest>,
) -> Result<Json<serde_json::Value>, ToolError> {
    let action = req.action.trim().to_string();
    if !DEV_MARKETPLACE_READ_ACTIONS.contains(&action.as_str()) {
        return Err(ToolError::Sdk {
            sdk_kind: "dev_preview_read_only".to_string(),
            message: format!("dev preview route blocked mutating marketplace action `{action}`"),
        });
    }

    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    services::helpers::handle_action(
        "marketplace",
        "api",
        request_id,
        req,
        crate::dispatch::marketplace::actions(),
        |action, params| async move {
            crate::dispatch::marketplace::dispatch_with_port(
                &action,
                params,
                &services::marketplace::WsNodeRpcPort,
            )
            .await
        },
    )
    .await
}

#[allow(clippy::too_many_lines)]
pub fn build_router(
    mut state: AppState,
    bearer_token: Option<String>,
    auth_state: Option<lab_auth::state::AuthState>,
    mcp_router: Option<Router<AppState>>,
    config_cors_origins: &[String],
) -> Router {
    if let Some(ref auth_state) = auth_state {
        state = state.with_oauth_state(auth_state.clone());
    }
    let static_token = bearer_token.map(Arc::<str>::from);
    state = state.with_bearer_token(static_token.clone());
    let auth_state = auth_state.map(Arc::new);
    let needs_auth = static_token.is_some() || auth_state.is_some();
    if !needs_auth {
        tracing::warn!(
            "HTTP API started without bearer token or OAuth auth state — all protected routes are unprotected"
        );
    }

    let v1 = build_v1_router(&state, needs_auth);

    let x_request_id = HeaderName::from_static("x-request-id");

    // Build separate protected sub-routers so `/v1/*` can accept browser
    // sessions while `/mcp` remains token-authenticated only.
    let v1_router = Router::new().nest("/v1", v1);
    let is_master = state.is_master();
    let resource_url: Option<Arc<str>> = auth_state
        .as_ref()
        .and_then(|state| state.config.public_url.as_ref().map(url::Url::as_str))
        .or_else(|| {
            state
                .auth_config
                .as_ref()
                .and_then(|cfg| cfg.public_url.as_ref().map(url::Url::as_str))
        })
        .map(Arc::from);
    let layer_deriver = state.actor_key_deriver.clone().map(lab_auth_deriver);
    // Build the shared AuthLayer once; per-route variants only differ in
    // whether the session-cookie path is enabled (true for browser-facing
    // /v1 + /dev + /v0.1; false for the bearer-only /mcp transport).
    let make_auth_layer = |allow_session_cookie: bool| -> AuthLayer {
        let mut layer = match auth_state.clone() {
            Some(state) => AuthLayer::from_state(state),
            // Bearer-only path (no OAuth state): grant the same legacy scopes
            // that the old middleware always issued for static-token requests.
            None => AuthLayer::new().with_static_token_scopes(vec![
                "lab:read".to_string(),
                "lab:admin".to_string(),
            ]),
        };
        layer = layer
            .with_static_token(static_token.clone())
            .with_actor_key_deriver(layer_deriver.clone())
            .with_resource_url(resource_url.clone())
            .with_allow_session_cookie(allow_session_cookie);
        layer
    };
    let v1_protected = if needs_auth {
        v1_router.route_layer(make_auth_layer(true))
    } else {
        v1_router
    };

    #[cfg(feature = "mcpregistry")]
    let v0_1_protected = {
        let v0_1_router = build_v0_1_router();
        if needs_auth {
            v0_1_router.route_layer(make_auth_layer(true))
        } else {
            v0_1_router
        }
    };

    let mcp_protected = mcp_router.map(|mcp| {
        if needs_auth {
            mcp.route_layer(make_auth_layer(false))
        } else {
            mcp
        }
    });

    // Build the outer router: health probes + discovery (no auth) + protected routes (auth).
    // Layers apply bottom-up: last .layer() call = outermost middleware.
    // Desired execution order (outermost → innermost → handler):
    //   SetRequestId → TraceLayer → PropagateRequestId → Timeout → Compression → CORS → handler
    let mut router = Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(health::ready))
        // POST /v1/nodes/hello is self-registration — exempt from bearer auth.
        .nest("/v1/nodes", super::nodes::public_routes(state.clone()))
        // Backward-compat alias for pre-rename self-registration clients.
        .nest("/v1/fleet", super::nodes::public_routes(state.clone()))
        // GET /v1/nodes/ws is outside bearer-auth middleware by design.
        // The `initialize` JSON-RPC method performs enrollment-token validation; all
        // subsequent node methods require an active session. See docs/runtime/FLEET_METHODS.md.
        .route(
            "/v1/nodes/ws",
            get(crate::api::nodes::fleet::websocket_upgrade),
        )
        // Backward-compat alias for pre-rename websocket clients.
        .route(
            "/v1/fleet/ws",
            get(crate::api::nodes::fleet::websocket_upgrade),
        )
        .merge(v1_protected);
    #[cfg(feature = "mcpregistry")]
    {
        router = router.merge(v0_1_protected);
    }
    if is_master {
        router = router
            .merge(crate::api::upstream_oauth::browser_routes(state.clone()))
            .merge(crate::api::upstream_oauth::well_known_routes(state.clone()));
    }
    if let Some(mcp) = mcp_protected.filter(|_| is_master) {
        router = router.merge(mcp);
    }
    if is_master && let Some(auth_state) = auth_state.as_ref() {
        let _ = auth_state;
        router = router
            .route(
                "/.well-known/oauth-authorization-server",
                get(auth_authorization_server_metadata),
            )
            .route(
                "/.well-known/oauth-protected-resource",
                get(auth_protected_resource_metadata),
            )
            .route("/jwks", get(auth_jwks))
            .route("/register", post(auth_register))
            .route("/authorize", get(auth_authorize))
            .route("/auth/login", get(auth_browser_login))
            .route(
                "/auth/session",
                get(crate::api::browser_session::auth_session),
            )
            .route(
                "/auth/logout",
                post(crate::api::browser_session::auth_logout),
            )
            .route("/auth/google/callback", get(auth_callback))
            .route("/token", post(auth_token));
    }

    // Dev routes — registered BEFORE the Next.js static fallback so they win
    // over the SPA. See docs/design/component-development.md §5 (two-tier
    // serving model) for the full rationale.
    //
    // /dev/api/*        → read-only dispatch endpoints (marketplace guard, nodeinfo)
    // /dev, /dev/{name} → Tier 1 mockup file server: serves HTML from
    //                     ~/.superpowers/brainstorm/content/{name}.html directly.
    //                     Once a feature graduates to a real Next.js page at
    //                     app/(admin)/dev/{name}/page.tsx, remove the corresponding
    //                     dev_mockup_named handler entry.
    let dev_routes = Router::new()
        .route("/dev/api/marketplace", post(dev_marketplace_readonly))
        .route("/dev/api/nodeinfo", get(dev_nodeinfo))
        // Mockup page routes — MUST stay before the static fallback (docs/design/component-development.md §5)
        .route("/dev", get(dev_mockup))
        .route("/dev/", get(dev_mockup))
        .route("/dev/{name}", get(dev_mockup_named))
        .route("/dev/{name}/", get(dev_mockup_named));
    let dev_routes = if needs_auth {
        dev_routes.route_layer(make_auth_layer(true))
    } else {
        dev_routes
    };
    router = router.merge(dev_routes);

    // Static-file fallback for the Next.js SPA.
    if state.web_assets_enabled() {
        router = router.fallback(crate::api::web::serve_web_request);
    }

    router
        .with_state(state)
        .layer(build_cors_layer(config_cors_origins))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            Duration::from_secs(30),
        ))
        // PropagateRequestId echoes the id back in the response header.
        .layer(PropagateRequestIdLayer::new(x_request_id.clone()))
        // TraceLayer reads x-request-id set by SetRequestId (outermost).
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &Request<_>| {
                let request_id = req
                    .headers()
                    .get("x-request-id")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("-");
                tracing::span!(
                    Level::INFO,
                    "request",
                    method = %req.method(),
                    path = %req.uri().path(),
                    request_id,
                    status = tracing::field::Empty,
                )
            }),
        )
        // SetRequestId generates a UUID for every request that lacks one.
        .layer(SetRequestIdLayer::new(x_request_id, MakeRequestUuid))
}

#[allow(clippy::too_many_lines)]
#[allow(dead_code)]
pub fn build_router_with_bearer(
    state: AppState,
    bearer_token: Option<String>,
    mcp_router: Option<Router<AppState>>,
) -> Router {
    build_router(state, bearer_token, None, mcp_router, &[])
}

/// Build a `CorsLayer` that allows only explicit trusted origins.
///
/// Sources (env var overrides config.toml):
/// - `LAB_CORS_ORIGINS` env var (comma-separated `scheme://host[:port]`)
/// - `api.cors_origins` in config.toml (array of strings)
///
/// Always includes `http://localhost`, `http://127.0.0.1`, and `http://[::1]`
/// as safe loopback defaults.
fn build_cors_layer(config_origins: &[String]) -> CorsLayer {
    use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
    use axum::http::{HeaderValue, Method};

    // Env var overrides config.toml when present.
    let raw_origins: Vec<String> = match std::env::var("LAB_CORS_ORIGINS") {
        Ok(val) if !val.trim().is_empty() => val
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        _ => config_origins.to_vec(),
    };

    let env_origins: Vec<HeaderValue> = raw_origins
        .iter()
        .filter_map(|s| match s.parse::<HeaderValue>() {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(
                    origin = s.as_str(),
                    error = %e,
                    "ignoring unparseable CORS origin"
                );
                None
            }
        })
        .collect();

    // Production loopback origins — always allowed.
    // 8765 is the default labby serve port; both `127.0.0.1` and `localhost`
    // are needed because some browsers resolve only one variant (lab-bg3e.3).
    let mut origins: Vec<HeaderValue> = vec![
        HeaderValue::from_static("http://localhost"),
        HeaderValue::from_static("http://localhost:8765"),
        HeaderValue::from_static("http://127.0.0.1"),
        HeaderValue::from_static("http://127.0.0.1:8765"),
        HeaderValue::from_static("http://[::1]"),
        HeaderValue::from_static("http://[::1]:8765"),
    ];
    // Dev ports (3000/5173/8080) are gated behind LAB_DEV_MODE=1 to prevent
    // a malicious npm postinstall HTTP server (or rogue browser extension on
    // those origins) from reading Setup API responses on a v1 unauthed lab
    // (lab-bg3e.3 security hardening).
    let dev_mode_enabled = std::env::var("LAB_DEV_MODE").as_deref() == Ok("1");
    if dev_mode_enabled {
        // One-shot WARN at startup so an operator who has LAB_DEV_MODE=1 in
        // their shell rc can see the wider CORS surface in production logs.
        tracing::warn!(
            subsystem = "api_server",
            phase = "cors.dev_mode_enabled",
            "LAB_DEV_MODE=1 — additional CORS origins enabled (3000/5173/8080); unset for production"
        );
        origins.extend([
            HeaderValue::from_static("http://localhost:3000"),
            HeaderValue::from_static("http://localhost:5173"),
            HeaderValue::from_static("http://localhost:8080"),
            HeaderValue::from_static("http://127.0.0.1:3000"),
            HeaderValue::from_static("http://127.0.0.1:5173"),
            HeaderValue::from_static("http://127.0.0.1:8080"),
        ]);
    }
    origins.extend(env_origins);

    // Explicit allowlist instead of Any — prevents arbitrary headers from
    // allowed origins reaching destructive endpoints (lab-3qn.7).
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            HeaderName::from_static("x-request-id"),
            HeaderName::from_static(lab_auth::session::BROWSER_CSRF_HEADER_NAME),
        ])
}

async fn service_actions(
    State(state): State<AppState>,
    axum::extract::Path(service): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ToolError> {
    let entry = state
        .catalog
        .services
        .iter()
        .find(|s| s.name == service)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("unknown service `{service}`"),
        })?;
    let actions = serde_json::to_value(&entry.actions).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("serialize actions: {e}"),
    })?;
    Ok(Json(actions))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use axum::Extension;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use super::*;

    async fn actor_key_probe(
        auth: Option<Extension<crate::api::oauth::AuthContext>>,
    ) -> Json<serde_json::Value> {
        let actor_key = auth
            .and_then(|Extension(ctx)| ctx.actor_key)
            .map(|key| key.to_string());
        Json(serde_json::json!({ "actor_key": actor_key }))
    }

    #[tokio::test]
    async fn actions_known_service_returns_200() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_array(), "body should be a JSON array of actions");
    }

    #[tokio::test]
    async fn actions_unknown_service_returns_404() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/doesnotexist/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "not_found");
    }

    #[tokio::test]
    async fn auth_layer_rejects_missing_bearer_token() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        // /v1/extract/actions is behind bearer auth; /health is NOT (lab-3qn.5).
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
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
    async fn auth_layer_accepts_valid_bearer_token() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        // Confirm that a valid token reaches the protected /v1 route.
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_layer_accepts_case_insensitive_bearer_token() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .header(header::AUTHORIZATION, "bearer   secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn web_ui_auth_disabled_does_not_bypass_v1_auth() {
        let state = AppState::new().with_web_ui_auth_disabled(true);
        let mcp_router: Router<AppState> =
            Router::new().route("/mcp", get(|| async { StatusCode::OK }));
        let app = build_router_with_bearer(state, Some("secret-token".into()), Some(mcp_router));

        let v1_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(v1_response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(v1_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "auth_failed");

        let mcp_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mcp_response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(mcp_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "auth_failed");
    }

    #[tokio::test]
    async fn health_endpoint_open_without_auth() {
        // /health must be reachable by monitoring probes without any token (lab-3qn.5).
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ready_endpoint_open_without_auth() {
        // /ready must be reachable by monitoring probes without any token (lab-3qn.5).
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn openapi_json_requires_bearer_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn openapi_json_returns_spec_with_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/openapi.json")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ct = response.headers().get(header::CONTENT_TYPE).unwrap();
        assert_eq!(ct, "application/json");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(spec["openapi"], "3.1.0");
        assert!(spec["info"]["title"].as_str().is_some());
        assert!(spec["paths"].as_object().is_some());
    }

    #[tokio::test]
    async fn docs_endpoint_returns_html_with_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/docs")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("scalar"), "HTML should reference Scalar");
        assert!(
            html.contains("openapi.json"),
            "HTML should reference spec URL"
        );
    }

    /// When a service is absent from the runtime registry (e.g. filtered out by
    /// `--services`), its `/v1/<service>` routes must NOT be mounted — even if
    /// the feature flag for that service is compiled in.
    ///
    /// This test uses an empty registry to simulate `labby serve --services <other>`
    /// excluding `radarr`, then verifies that `POST /v1/radarr` returns 404 rather
    /// than reaching the handler.
    #[cfg(feature = "radarr")]
    #[tokio::test]
    async fn service_filtered_from_registry_has_no_http_route() {
        use crate::registry::ToolRegistry;

        // An empty registry = no services enabled at runtime.
        let registry = ToolRegistry::new();
        let state = AppState::from_registry(registry);
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/radarr")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"help"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "radarr routes must not be mounted when radarr is absent from the runtime registry"
        );
    }

    #[cfg(feature = "uptime_kuma")]
    #[tokio::test]
    async fn uptime_kuma_route_uses_registry_service_name() {
        let mut state = AppState::new();
        state.clients = Arc::new(crate::dispatch::clients::ServiceClients {
            uptime_kuma: Some(Arc::new(
                lab_apis::uptime_kuma::UptimeKumaClient::new(
                    "http://127.0.0.1:3001",
                    lab_apis::core::Auth::None,
                )
                .unwrap(),
            )),
            ..Default::default()
        });
        let app = build_router_with_bearer(state, None, None);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/uptime-kuma")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"contract.status","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/uptime_kuma")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"contract.status","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bearer_mode_still_accepts_lab_mcp_http_token() {
        let state = AppState::new();
        let app = build_router(state, Some("secret-token".into()), None, None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn oauth_mode_accepts_lab_auth_jwt() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_lab_token(&auth_state);
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn static_bearer_bind_attaches_actor_key_when_deriver_is_configured() {
        let deriver =
            crate::observability::activity::ActorKeyDeriver::from_secret("test-secret").unwrap();
        let expected = deriver.derive_subject("static-bearer").unwrap();
        let deriver = Arc::new(deriver);
        let layer = AuthLayer::new()
            .with_static_token(Some(Arc::<str>::from("secret-token")))
            .with_actor_key_deriver(Some(lab_auth_deriver(Arc::clone(&deriver))));
        let app = Router::new()
            .route("/probe", get(actor_key_probe))
            .route_layer(layer);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/probe")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["actor_key"], expected.as_str());
    }

    #[tokio::test]
    async fn browser_session_bind_attaches_actor_key_when_deriver_is_configured() {
        let auth_state = Arc::new(test_lab_auth_state().await);
        let session = seed_browser_session(&auth_state).await;
        let deriver =
            crate::observability::activity::ActorKeyDeriver::from_secret("test-secret").unwrap();
        let expected = deriver.derive_subject(&session.subject).unwrap();
        let deriver = Arc::new(deriver);
        let layer = AuthLayer::from_state(Arc::clone(&auth_state))
            .with_actor_key_deriver(Some(lab_auth_deriver(Arc::clone(&deriver))))
            .with_allow_session_cookie(true);
        let app = Router::new()
            .route("/probe", get(actor_key_probe))
            .route_layer(layer);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/probe")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["actor_key"], expected.as_str());
    }

    #[tokio::test]
    async fn authenticated_bind_leaves_actor_key_null_without_deriver() {
        let layer = AuthLayer::new()
            .with_static_token(Some(Arc::<str>::from("secret-token")));
        let app = Router::new()
            .route("/probe", get(actor_key_probe))
            .route_layer(layer);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/probe")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["actor_key"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn auth_session_returns_internal_error_when_lookup_fails() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        auth_state
            .store
            .execute_test_statement("DROP TABLE browser_sessions;")
            .await
            .unwrap();
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/session")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn v1_accepts_browser_session_cookie() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mcp_rejects_browser_session_cookie_without_bearer() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let mcp_router = Router::new().route("/mcp", get(|| async { StatusCode::OK }));
        let app = build_router(state, None, Some(auth_state), Some(mcp_router), &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn v1_session_post_requires_csrf_header() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/gateway")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::from(r#"{"action":"gateway.list","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn auth_session_returns_browser_identity_and_csrf_token() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/session")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], true);
        assert_eq!(json["user"]["sub"], "browser-user");
        assert_eq!(json["csrf_token"], "csrf-123");
    }

    #[tokio::test]
    async fn auth_layer_accepts_valid_oauth_bearer_token() {
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_lab_token(&auth_state);
        let app = build_router(AppState::new(), None, Some(auth_state), None, &[]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_logout_revokes_session_and_clears_cookie() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let store = auth_state.store.clone();
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/logout")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .header(lab_auth::session::BROWSER_CSRF_HEADER_NAME, "csrf-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("Max-Age=0"));
        assert!(
            store
                .find_browser_session("sess-123")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn auth_logout_returns_internal_error_when_revocation_fails() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        auth_state
            .store
            .execute_test_statement("DROP TABLE browser_sessions;")
            .await
            .unwrap();
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/logout")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .header(lab_auth::session::BROWSER_CSRF_HEADER_NAME, "csrf-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
    }

    #[tokio::test]
    async fn oauth_mode_missing_token_returns_www_authenticate_metadata_hint() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let header = response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(header.contains("resource_metadata="));
    }

    #[tokio::test]
    async fn gateway_oauth_routes_require_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            tempdir.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ));
        let state = AppState::new().with_gateway_manager(manager);
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/gateway/oauth/status?upstream=test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn browser_oauth_callback_bypasses_bearer_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            tempdir.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ));
        let state = AppState::new().with_gateway_manager(manager);
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/upstream/callback?upstream=test&state=csrf&code=authcode")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn serves_web_assets_for_browser_routes_when_configured() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("index.html"),
            "<html><body>Labby</body></html>",
        )
        .unwrap();

        let state = AppState::new().with_web_assets_dir(dir.path().to_path_buf());
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/gateways/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("Labby"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlinked_assets_outside_configured_web_root() {
        use std::os::unix::fs as unix_fs;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("index.html"),
            "<html><body>Labby</body></html>",
        )
        .unwrap();
        fs::write(outside.path().join("secret.txt"), "top-secret").unwrap();
        unix_fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("secret.txt"),
        )
        .unwrap();

        let state = AppState::new().with_web_assets_dir(dir.path().to_path_buf());
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/secret.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn v1_routes_still_win_over_web_asset_fallback() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("index.html"),
            "<html><body>Labby</body></html>",
        )
        .unwrap();

        let state = AppState::new().with_web_assets_dir(dir.path().to_path_buf());
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("application/json"));
    }

    #[tokio::test]
    async fn serves_embedded_web_assets_without_configured_directory() {
        let state = AppState::new().with_embedded_web_assets();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("text/html"));
    }

    #[tokio::test]
    async fn v1_routes_still_win_over_embedded_web_asset_fallback() {
        let state = AppState::new().with_embedded_web_assets();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("application/json"));
    }

    async fn test_lab_auth_state() -> lab_auth::state::AuthState {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let config = lab_auth::config::AuthConfig {
            mode: lab_auth::config::AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: dir.path().join("auth.db"),
            key_path: dir.path().join("auth-jwt.pem"),
            bootstrap_secret: Some("bootstrap-secret".to_string()),
            google: lab_auth::config::GoogleConfig {
                client_id: "client-id".to_string(),
                client_secret: "client-secret".to_string(),
                callback_path: "/auth/google/callback".to_string(),
                scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
            },
            ..lab_auth::config::AuthConfig::default()
        };
        lab_auth::state::AuthState::new(config).await.unwrap()
    }

    fn issue_test_lab_token(auth_state: &lab_auth::state::AuthState) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        auth_state
            .signing_keys
            .issue_access_token(&lab_auth::jwt::AccessClaims {
                iss: "https://lab.example.com".to_string(),
                sub: "google-user".to_string(),
                aud: "https://lab.example.com/mcp".to_string(),
                exp: now + 3600,
                iat: now,
                jti: "test-jti".to_string(),
                scope: "lab".to_string(),
                azp: "client".to_string(),
            })
            .unwrap()
    }

    async fn seed_browser_session(
        auth_state: &lab_auth::state::AuthState,
    ) -> lab_auth::types::BrowserSessionRow {
        let session = lab_auth::types::BrowserSessionRow {
            session_id: "sess-123".to_string(),
            subject: "browser-user".to_string(),
            email: Some("browser@example.com".to_string()),
            csrf_token: "csrf-123".to_string(),
            created_at: 1,
            expires_at: i64::MAX,
        };
        auth_state
            .store
            .upsert_browser_session(session.clone())
            .await
            .unwrap();
        session
    }

    /// `/v0.1/servers` requires bearer auth — unauthenticated requests must get 401,
    /// authenticated requests must reach the handler (200 or 503 if store uninitialized).
    #[cfg(feature = "mcpregistry")]
    #[tokio::test]
    async fn v01_servers_requires_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        // Unauthenticated → 401
        let unauth_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v0.1/servers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            unauth_response.status(),
            StatusCode::UNAUTHORIZED,
            "/v0.1/servers must reject unauthenticated requests"
        );
        let body = axum::body::to_bytes(unauth_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "auth_failed");

        // Authenticated → reaches handler (200 OK or 503 if store not initialized)
        let auth_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v0.1/servers")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = auth_response.status();
        assert!(
            status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
            "/v0.1/servers with valid token must return 200 or 503 (store not initialized), got {status}"
        );
        if status == StatusCode::SERVICE_UNAVAILABLE {
            let body = axum::body::to_bytes(auth_response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["kind"], "service_unavailable");
        }
    }

    /// POST /dev/api/marketplace must accept whitelisted read-only actions after auth.
    #[tokio::test]
    async fn dev_marketplace_allows_whitelisted_read_actions() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        for action in DEV_MARKETPLACE_READ_ACTIONS {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/dev/api/marketplace")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::AUTHORIZATION, "Bearer secret-token")
                        .body(Body::from(
                            serde_json::to_string(&serde_json::json!({ "action": action }))
                                .unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();

            // 200 OK or 4xx from dispatch (action not implemented in test env) — never 403
            assert_ne!(
                response.status(),
                StatusCode::FORBIDDEN,
                "read-only action `{action}` must not be blocked by dev guard"
            );
        }
    }

    /// POST /dev/api/marketplace must block mutating actions after auth.
    #[tokio::test]
    async fn dev_marketplace_blocks_mutating_actions() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        for action in &[
            "plugin.install",
            "plugin.uninstall",
            "sources.add",
            "sources.remove",
            "plugin.workspace.save",
            "plugin.deploy",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/dev/api/marketplace")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::AUTHORIZATION, "Bearer secret-token")
                        .body(Body::from(
                            serde_json::to_string(&serde_json::json!({ "action": action }))
                                .unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "mutating action `{action}` must be blocked by dev guard"
            );
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["kind"], "dev_preview_read_only");
        }
    }

    /// POST /dev/api/marketplace must require auth when auth is configured.
    #[tokio::test]
    async fn dev_marketplace_requires_auth_when_configured() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dev/api/marketplace")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"plugin.install"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "/dev/api/marketplace must use auth middleware when auth is configured"
        );
    }

    #[tokio::test]
    async fn dev_mockup_routes_require_auth_when_configured() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/dev/example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "/dev mockup routes must use auth middleware when auth is configured"
        );
    }

    /// POST /dev/api/marketplace remains open in explicit no-auth local mode.
    #[tokio::test]
    async fn dev_marketplace_allows_no_auth_when_server_has_no_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dev/api/marketplace")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"plugin.install"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "no-auth local mode should still reach the read-only dev guard"
        );
    }
}
