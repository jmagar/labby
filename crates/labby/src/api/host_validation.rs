//! Host header validation Layer.
//!
//! DNS rebinding mitigation: requests must use a loopback `Host` header or
//! an explicitly configured public/allowed host before they reach the
//! dispatch layer.
//!
//! Applied to every v1 route group that exposes setup or local state:
//! `/v1/setup`, `/v1/marketplace`, `/v1/doctor`. It is
//! intentionally conservative: a missing `Host` header is rejected too (no
//! browser-driven request omits it).
//!
//! Bypass for tests: set the `LAB_HOST_VALIDATION_DISABLED=1` env var.

use axum::{
    body::Body,
    http::{Request, Response, StatusCode, header::HOST},
    middleware::Next,
};

/// Loopback hostnames accepted by the validator.
const LOOPBACK_HOSTS: &[&str] = &["127.0.0.1", "localhost", "::1"];

fn normalize_host_value(host_value: &str) -> Option<String> {
    let host = host_without_port(host_value)?;
    Some(host.trim_end_matches('.').to_ascii_lowercase())
}

fn host_without_port(host_value: &str) -> Option<&str> {
    let trimmed = host_value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Strip optional port suffix.
    // Handle IPv6 with brackets first: `[::1]:8765` or `[::1]`.
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return Some(&rest[..end]);
        }
        return None;
    }
    if let Some((host, _port)) = trimmed.rsplit_once(':')
        && !host.contains(':')
    {
        return Some(host);
    }
    Some(trimmed)
}

/// Returns `true` if `host_value` (e.g. `"localhost:8765"` or `"[::1]"`)
/// resolves to a loopback hostname.
#[must_use]
pub fn is_loopback_host_value(host_value: &str) -> bool {
    normalize_host_value(host_value)
        .as_deref()
        .is_some_and(|host| LOOPBACK_HOSTS.contains(&host))
}

fn configured_allowed_hosts(public_url: Option<&str>, extra_hosts: Option<&str>) -> Vec<String> {
    let mut hosts = Vec::new();
    if let Some(public_url) = public_url
        && let Ok(url) = url::Url::parse(public_url)
        && let Some(host) = url.host_str()
    {
        hosts.push(host.to_ascii_lowercase());
    }
    if let Some(extra_hosts) = extra_hosts {
        for host in extra_hosts.split(',').map(str::trim) {
            if host.is_empty() || host == "*" {
                continue;
            }
            if let Some(normalized) = normalize_host_value(host)
                && !hosts.contains(&normalized)
            {
                hosts.push(normalized);
            }
        }
    }
    hosts
}

fn is_allowed_host_value(host_value: &str, allowed_hosts: &[String]) -> bool {
    if is_loopback_host_value(host_value) {
        return true;
    }
    let Some(host) = normalize_host_value(host_value) else {
        return false;
    };
    allowed_hosts.iter().any(|allowed| allowed == &host)
}

/// Axum middleware function. Use with
/// `Router::layer(axum::middleware::from_fn(host_validation_layer))`.
pub async fn host_validation_layer(
    req: Request<Body>,
    next: Next,
) -> Result<Response<Body>, StatusCode> {
    if std::env::var("LAB_HOST_VALIDATION_DISABLED").as_deref() == Ok("1") {
        // Loud per-request warn so accidental production use of the test
        // bypass cannot hide. If you see this in your logs and you're not
        // running tests, unset LAB_HOST_VALIDATION_DISABLED immediately.
        tracing::warn!(
            surface = "api",
            kind = "host_validation_bypassed",
            path = %req.uri().path(),
            "LAB_HOST_VALIDATION_DISABLED=1 — DNS-rebinding mitigation skipped"
        );
        return Ok(next.run(req).await);
    }
    let host = req
        .headers()
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let public_url = std::env::var("LAB_PUBLIC_URL").ok();
    let extra_hosts = std::env::var("LAB_MCP_ALLOWED_HOSTS").ok();
    let allowed_hosts = configured_allowed_hosts(public_url.as_deref(), extra_hosts.as_deref());
    if !is_allowed_host_value(host, &allowed_hosts) {
        tracing::warn!(
            surface = "api",
            kind = "host_validation_failed",
            host = %host,
            path = %req.uri().path(),
            "rejecting non-loopback Host header"
        );
        return Err(StatusCode::MISDIRECTED_REQUEST);
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_loopback_names() {
        assert!(is_loopback_host_value("localhost"));
        assert!(is_loopback_host_value("127.0.0.1"));
        assert!(is_loopback_host_value("[::1]"));
        assert!(is_loopback_host_value("localhost:8765"));
        assert!(is_loopback_host_value("127.0.0.1:8765"));
        assert!(is_loopback_host_value("[::1]:8765"));
        assert!(is_loopback_host_value("LOCALHOST:8765"));
    }

    #[test]
    fn rejects_non_loopback() {
        assert!(!is_loopback_host_value("evil.example.com"));
        assert!(!is_loopback_host_value("192.168.1.5:8765"));
        assert!(!is_loopback_host_value(""));
        assert!(!is_loopback_host_value("attacker.local"));
    }

    #[test]
    fn rejects_malformed_ipv6() {
        // No closing bracket — reject defensively.
        assert!(!is_loopback_host_value("[::1"));
    }

    #[test]
    fn allows_public_url_host() {
        let allowed = configured_allowed_hosts(Some("https://lab.example.com/app"), None);

        assert!(is_allowed_host_value("lab.example.com", &allowed));
        assert!(is_allowed_host_value("lab.example.com:8765", &allowed));
        assert!(is_allowed_host_value("LAB.EXAMPLE.COM", &allowed));
        assert!(!is_allowed_host_value("evil.example.com", &allowed));
    }

    #[test]
    fn allows_configured_extra_hosts_without_wildcard() {
        let allowed =
            configured_allowed_hosts(None, Some("lab.internal, dashboard.example.com:443, *"));

        assert!(is_allowed_host_value("lab.internal", &allowed));
        assert!(is_allowed_host_value("dashboard.example.com", &allowed));
        assert!(!is_allowed_host_value("attacker.local", &allowed));
        assert!(!allowed.contains(&"*".to_string()));
    }
}
