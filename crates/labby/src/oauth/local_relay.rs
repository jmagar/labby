//! Loopback-only OAuth callback forwarder.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::Response,
    routing::any,
};
use serde_json::json;
use tokio::net::TcpListener;
use url::form_urlencoded;

use crate::oauth::error::OauthRelayError;
use crate::oauth::target::{
    ResolvedTarget, build_forward_url, filter_hop_by_hop_request_headers,
    filter_hop_by_hop_response_headers,
};

#[derive(Debug, Clone)]
pub struct LocalRelayConfig {
    pub bind_addr: SocketAddr,
    pub resolved_target: ResolvedTarget,
    pub request_timeout: Duration,
}

#[derive(Clone)]
struct RelayState {
    resolved_target: ResolvedTarget,
    request_timeout: Duration,
    client: reqwest::Client,
}

pub async fn run_local_relay(config: LocalRelayConfig) -> Result<(), OauthRelayError> {
    let bind_addr = config.bind_addr;
    let listener = bind_local_relay_listener(bind_addr).await?;
    serve_local_relay(listener, config).await
}

pub async fn bind_local_relay_listener(
    bind_addr: SocketAddr,
) -> Result<TcpListener, OauthRelayError> {
    TcpListener::bind(bind_addr)
        .await
        .map_err(|source| OauthRelayError::Bind {
            bind_addr: bind_addr.to_string(),
            source,
        })
}

pub async fn serve_local_relay(
    listener: TcpListener,
    config: LocalRelayConfig,
) -> Result<(), OauthRelayError> {
    let bind_addr = config.bind_addr;

    tracing::info!(
        surface = "oauth_relay",
        bind_addr = %config.bind_addr,
        machine_id = ?config.resolved_target.machine_id,
        default_port = ?config.resolved_target.default_port,
        target = %redact_forward_target(&config.resolved_target.target_url),
        "oauth relay local listener ready"
    );

    let state = RelayState {
        resolved_target: config.resolved_target,
        request_timeout: config.request_timeout,
        client: reqwest::Client::new(),
    };

    let app = Router::new()
        .fallback(any(relay_callback))
        .with_state(state);
    axum::serve(listener, app)
        .await
        .map_err(|source| OauthRelayError::Bind {
            bind_addr: bind_addr.to_string(),
            source,
        })
}

async fn relay_callback(
    State(state): State<RelayState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !matches!(method, Method::GET | Method::POST) {
        return json_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "oauth relay only supports GET and POST callback requests",
        );
    }

    let suffix_path =
        match suffix_path_for_request(state.resolved_target.target_url.path(), uri.path()) {
            Ok(path) => path,
            Err(detail) => return json_error(StatusCode::NOT_FOUND, detail),
        };
    let query_items = parse_query_items(uri.query());
    let query_refs = query_items
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let forward_url =
        match build_forward_url(&state.resolved_target.target_url, &suffix_path, &query_refs) {
            Ok(url) => url,
            Err(error) => {
                return json_error(StatusCode::BAD_GATEWAY, error.to_string());
            }
        };
    let target_host = forward_url.host_str().unwrap_or("unknown").to_string();
    let redacted_target = redact_forward_target(&forward_url);

    let mut request = state
        .client
        .request(method.clone(), forward_url.clone())
        .timeout(state.request_timeout);
    for (name, value) in &filter_hop_by_hop_request_headers(&headers) {
        request = request.header(name, value);
    }
    if let Some(machine_id) = state.resolved_target.machine_id.as_deref() {
        request = request.header("x-lab-oauth-relay-machine-id", machine_id);
    }
    if !body.is_empty() {
        request = request.body(body.clone());
    }

    let start = Instant::now();
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) if error.is_timeout() => {
            let detail = OauthRelayError::UpstreamTimeout {
                target: redacted_target.clone(),
                timeout_ms: state.request_timeout.as_millis() as u64,
            }
            .to_string();
            tracing::warn!(
                surface = "oauth_relay",
                method = %method,
                path = %uri.path(),
                machine_id = ?state.resolved_target.machine_id,
                target_host,
                elapsed_ms = start.elapsed().as_millis(),
                "oauth relay target timed out"
            );
            return json_error(StatusCode::GATEWAY_TIMEOUT, detail);
        }
        Err(error) => {
            let detail = format_upstream_error(&forward_url, &redacted_target, &error);
            tracing::warn!(
                surface = "oauth_relay",
                method = %method,
                path = %uri.path(),
                machine_id = ?state.resolved_target.machine_id,
                target_host,
                elapsed_ms = start.elapsed().as_millis(),
                error = %detail,
                "oauth relay forward failed"
            );
            return json_error(StatusCode::BAD_GATEWAY, detail);
        }
    };

    let status = response.status();
    let response_headers = filter_hop_by_hop_response_headers(response.headers());
    let response_body = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) if error.is_timeout() => {
            return json_error(
                StatusCode::GATEWAY_TIMEOUT,
                OauthRelayError::UpstreamTimeout {
                    target: redacted_target,
                    timeout_ms: state.request_timeout.as_millis() as u64,
                }
                .to_string(),
            );
        }
        Err(error) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                format_upstream_error(&forward_url, &redact_forward_target(&forward_url), &error),
            );
        }
    };

    let elapsed = start.elapsed();
    tracing::info!(
        surface = "oauth_relay",
        method = %method,
        path = %uri.path(),
        machine_id = ?state.resolved_target.machine_id,
        target_host,
        status = %status,
        elapsed_ms = elapsed.as_millis(),
        "oauth relay forward complete"
    );

    build_response(status, &response_headers, response_body)
}

fn build_response(status: StatusCode, headers: &HeaderMap, body: Bytes) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().extend(headers.clone());
    response
}

fn json_error(status: StatusCode, detail: impl Into<String>) -> Response {
    let body = Body::from(json!({ "detail": detail.into() }).to_string());
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    response
}

fn suffix_path_for_request(
    target_base_path: &str,
    request_path: &str,
) -> Result<String, &'static str> {
    let normalized_base = target_base_path.trim_end_matches('/');
    let request_path = request_path.trim_end_matches('/');
    let suffix = if request_path == normalized_base {
        ""
    } else if normalized_base.is_empty() {
        request_path
    } else if let Some(rest) = request_path.strip_prefix(normalized_base) {
        match rest.strip_prefix('/') {
            Some(rest) => rest,
            None if rest.is_empty() => "",
            None => return Err("path not under relay target"),
        }
    } else {
        request_path
    };

    Ok(suffix.trim_matches('/').to_string())
}

fn parse_query_items(query: Option<&str>) -> Vec<(String, String)> {
    query
        .map(|query| {
            form_urlencoded::parse(query.as_bytes())
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn redact_forward_target(url: &reqwest::Url) -> String {
    let mut redacted = url.clone();
    redacted.set_query(None);
    redacted.set_fragment(None);
    redacted.to_string()
}

fn format_upstream_error(
    url: &reqwest::Url,
    redacted_target: &str,
    error: &reqwest::Error,
) -> String {
    let sanitized_source = error.to_string().replace(url.as_str(), redacted_target);
    format!(
        "failed to reach oauth relay target `{}`: {}",
        redacted_target, sanitized_source
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::oauth::target::resolve_explicit_target;
    use axum::{
        Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, HeaderValue, Method, StatusCode, header},
        response::IntoResponse,
        routing::any,
    };
    use tokio::task::JoinHandle;
    use tokio::time::sleep;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct SeenRequest {
        method: String,
        path_and_query: String,
        body: Vec<u8>,
        headers: Vec<(String, String)>,
    }

    #[derive(Clone)]
    struct UpstreamState {
        seen_requests: Arc<Mutex<Vec<SeenRequest>>>,
        response_status: StatusCode,
        response_body: &'static str,
        response_headers: Vec<(&'static str, &'static str)>,
        delay: Duration,
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oauth_local_relay_forwards_callback_requests_end_to_end() {
        let seen_requests = Arc::new(Mutex::new(Vec::new()));
        let upstream = spawn_upstream(UpstreamState {
            seen_requests: seen_requests.clone(),
            response_status: StatusCode::CREATED,
            response_body: "ok-from-upstream",
            response_headers: vec![
                (header::CONTENT_TYPE.as_str(), "text/plain; charset=utf-8"),
                (header::CONNECTION.as_str(), "close"),
                (header::SET_COOKIE.as_str(), "oauth=secret; HttpOnly"),
            ],
            delay: Duration::from_millis(0),
        })
        .await;

        let relay_addr = available_loopback_addr().await;
        let relay = spawn_relay(LocalRelayConfig {
            bind_addr: relay_addr,
            resolved_target: resolve_explicit_target(
                &format!("http://{}/callback/dookie", upstream.addr),
                Some(relay_addr.port()),
            )
            .unwrap(),
            request_timeout: Duration::from_millis(250),
        })
        .await;

        let response = reqwest::Client::new()
            .post(format!(
                "http://{}/callback/dookie/extra?code=abc&state=xyz",
                relay_addr
            ))
            .header(
                header::CONTENT_TYPE.as_str(),
                "application/x-www-form-urlencoded",
            )
            .header(header::AUTHORIZATION.as_str(), "Bearer secret-token")
            .header(header::COOKIE.as_str(), "lab_session=secret")
            .body("grant_type=authorization_code")
            .send()
            .await
            .expect("relay request should succeed");

        let response_headers = response.headers().clone();
        let response_status = response.status();
        let response_body = response.text().await.expect("body should decode");

        let seen = seen_requests.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(
            seen[0].path_and_query,
            "/callback/dookie/extra?code=abc&state=xyz"
        );
        assert_eq!(seen[0].body, b"grant_type=authorization_code");
        assert_eq!(response_status, StatusCode::CREATED);
        assert_eq!(response_body, "ok-from-upstream");
        assert_eq!(
            response_headers[header::CONTENT_TYPE],
            HeaderValue::from_static("text/plain; charset=utf-8")
        );
        assert!(response_headers.get(header::CONNECTION).is_none());
        assert!(response_headers.get(header::SET_COOKIE).is_none());
        assert!(
            seen[0]
                .headers
                .iter()
                .all(|(name, _)| name != header::AUTHORIZATION.as_str())
        );
        assert!(
            seen[0]
                .headers
                .iter()
                .all(|(name, _)| name != header::COOKIE.as_str())
        );

        relay.abort();
        upstream.handle.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oauth_local_relay_rejects_paths_outside_the_target_boundary() {
        let upstream = spawn_upstream(UpstreamState {
            seen_requests: Arc::new(Mutex::new(Vec::new())),
            response_status: StatusCode::OK,
            response_body: "ok",
            response_headers: vec![(header::CONTENT_TYPE.as_str(), "text/plain")],
            delay: Duration::from_millis(0),
        })
        .await;

        let relay_addr = available_loopback_addr().await;
        let relay = spawn_relay(LocalRelayConfig {
            bind_addr: relay_addr,
            resolved_target: resolve_explicit_target(
                &format!("http://{}/callback", upstream.addr),
                Some(relay_addr.port()),
            )
            .unwrap(),
            request_timeout: Duration::from_millis(250),
        })
        .await;

        let response = reqwest::Client::new()
            .get(format!("http://{}/callback2/extra?code=abc", relay_addr))
            .send()
            .await
            .expect("relay request should succeed");
        let status = response.status();
        let body = response.text().await.expect("body should decode");

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("path not under relay target"));

        relay.abort();
        upstream.handle.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oauth_local_relay_returns_bad_gateway_for_unreachable_target() {
        let upstream_addr = available_loopback_addr().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_addr = listener.local_addr().unwrap();
        let relay_config = LocalRelayConfig {
            bind_addr: relay_addr,
            resolved_target: resolve_explicit_target(
                &format!("http://{}/callback/dookie", upstream_addr),
                Some(relay_addr.port()),
            )
            .unwrap(),
            request_timeout: Duration::from_millis(100),
        };
        let relay = tokio::spawn(async move {
            serve_local_relay(listener, relay_config).await.unwrap();
        });
        sleep(Duration::from_millis(25)).await;

        let response = reqwest::Client::new()
            .get(format!("http://{}/callback/dookie?code=abc", relay_addr))
            .send()
            .await
            .expect("relay should return a response");
        let status = response.status();
        let body = response.text().await.expect("body should decode");

        // On Linux the OS returns ECONNREFUSED immediately → 502 BAD_GATEWAY.
        // On Windows a loopback connection to a closed port may time out instead
        // of getting ECONNREFUSED → 504 GATEWAY_TIMEOUT. Both are correct here.
        assert!(
            status == StatusCode::BAD_GATEWAY || status == StatusCode::GATEWAY_TIMEOUT,
            "expected 502 or 504, got {status}"
        );
        assert!(!body.contains("code=abc"));

        relay.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oauth_local_relay_returns_gateway_timeout_for_slow_target() {
        let upstream = spawn_upstream(UpstreamState {
            seen_requests: Arc::new(Mutex::new(Vec::new())),
            response_status: StatusCode::OK,
            response_body: "slow",
            response_headers: vec![(header::CONTENT_TYPE.as_str(), "text/plain")],
            delay: Duration::from_millis(150),
        })
        .await;

        let relay_addr = available_loopback_addr().await;
        let relay = spawn_relay(LocalRelayConfig {
            bind_addr: relay_addr,
            resolved_target: resolve_explicit_target(
                &format!("http://{}/callback/dookie", upstream.addr),
                Some(relay_addr.port()),
            )
            .unwrap(),
            request_timeout: Duration::from_millis(25),
        })
        .await;

        let response = reqwest::Client::new()
            .get(format!("http://{}/callback/dookie?code=abc", relay_addr))
            .send()
            .await
            .expect("relay should return a response");
        let status = response.status();
        let body = response.text().await.expect("body should decode");

        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert!(body.contains("timed out"));

        relay.abort();
        upstream.handle.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oauth_local_relay_returns_bind_error_on_port_collision() {
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bind_addr = occupied.local_addr().unwrap();

        let result = run_local_relay(LocalRelayConfig {
            bind_addr,
            resolved_target: resolve_explicit_target(
                "http://127.0.0.1:48081/callback/dookie",
                Some(bind_addr.port()),
            )
            .unwrap(),
            request_timeout: Duration::from_millis(100),
        })
        .await;

        assert!(matches!(result, Err(OauthRelayError::Bind { .. })));
    }

    #[test]
    fn redact_forward_target_strips_query_and_fragment() {
        // The relay logs `%redact_forward_target(...)` for the target field.
        // Verify the function strips query params and fragment so secrets
        // in OAuth callback URLs never appear in structured logs.
        let url = reqwest::Url::parse(
            "http://127.0.0.1:38935/callback/dookie?token=secret-value#fragment",
        )
        .unwrap();
        let redacted = redact_forward_target(&url);
        assert!(
            !redacted.contains("token=secret-value"),
            "query param leaked: {redacted}"
        );
        assert!(
            !redacted.contains("fragment"),
            "fragment leaked: {redacted}"
        );
        assert!(redacted.contains("127.0.0.1"), "host missing: {redacted}");
        assert!(
            redacted.contains("/callback/dookie"),
            "path missing: {redacted}"
        );
    }

    #[test]
    fn suffix_path_for_request_requires_segment_boundary() {
        assert_eq!(
            suffix_path_for_request("/callback", "/callback2/extra"),
            Err("path not under relay target")
        );
        assert_eq!(
            suffix_path_for_request("/callback/dookie", "/callback/dookie/extra"),
            Ok("extra".to_string())
        );
        assert_eq!(
            suffix_path_for_request("/callback", "/callback"),
            Ok(String::new())
        );
    }

    async fn spawn_upstream(state: UpstreamState) -> UpstreamHandle {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .fallback(any(upstream_handler))
            .with_state(state);
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        UpstreamHandle { addr, handle }
    }

    async fn upstream_handler(
        State(state): State<UpstreamState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> impl IntoResponse {
        state.seen_requests.lock().unwrap().push(SeenRequest {
            method: method.to_string(),
            path_and_query: uri
                .path_and_query()
                .map_or_else(|| uri.path().to_string(), ToString::to_string),
            body: body.to_vec(),
            headers: headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.as_str().to_string(), value.to_string()))
                })
                .collect(),
        });

        if !state.delay.is_zero() {
            sleep(state.delay).await;
        }

        let mut headers = HeaderMap::new();
        for (name, value) in &state.response_headers {
            headers.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        (state.response_status, headers, state.response_body)
    }

    async fn available_loopback_addr() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        addr
    }

    async fn spawn_relay(config: LocalRelayConfig) -> JoinHandle<()> {
        let handle = tokio::spawn(async move {
            run_local_relay(config).await.unwrap();
        });
        sleep(Duration::from_millis(25)).await;
        handle
    }

    struct UpstreamHandle {
        addr: SocketAddr,
        handle: JoinHandle<()>,
    }
}
