use super::state::AppState;
use crate::config::NodeRole;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use labby_web::{AssetResponse, AssetSource, serve_asset};

/// Whether the embedded SPA bundle (`index.html`) shipped in this binary.
///
/// Delegates to `lab-gateway-web`, which owns the build-time embedded asset
/// table. Used by the serve path and router tests to decide whether the
/// embedded fallback is meaningful.
pub fn embedded_web_assets_available() -> bool {
    labby_web::embedded_assets_available()
}

/// Turn a resolved asset into an axum response, honoring `HEAD` (headers only).
fn web_asset_response(asset: AssetResponse, method: &Method) -> Response {
    let AssetResponse {
        bytes,
        content_type,
        cache_control,
    } = asset;
    let mut response = Response::new(if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(bytes)
    });
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(cache_control),
    );
    response
}

pub async fn serve_web_request(State(state): State<AppState>, request: Request) -> Response {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return StatusCode::NOT_FOUND.into_response();
    }

    if matches!(state.node_role, Some(NodeRole::NonMaster)) {
        tracing::warn!(
            path = %request.uri().path(),
            "rejected web ui request on non-master node"
        );
        return (
            StatusCode::FORBIDDEN,
            "web ui is disabled on non-master devices",
        )
            .into_response();
    }

    // Source precedence is product policy and stays here: a configured directory
    // always wins; the embedded bundle is the no-config fallback. When neither
    // is available there is nothing to serve. Asset lookup, symlink-escape
    // rejection, and header derivation live in `lab-gateway-web`.
    let source = if let Some(base_dir) = state.web_assets_dir.as_deref() {
        AssetSource::Directory(base_dir.to_path_buf())
    } else if state.embedded_web_assets {
        AssetSource::Embedded
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match serve_asset(request.uri().path(), &source).await {
        Ok(asset) => web_asset_response(asset, request.method()),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
