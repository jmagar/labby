//! Shared SSRF preflight guards for externally supplied HTTPS URLs.
//!
//! This is a preflight guard, not a complete DNS-rebinding defense. Any code
//! that later performs an outbound request must still avoid unsafe redirects and
//! must not claim that validation-time DNS pins the final connection target.
//!
//! The host/IP allow-deny policy itself is **not** defined here — it lives in
//! `labby_primitives::ssrf` (the canonical single source of truth, shared with
//! `labby-apis`). This module owns only the runtime concerns: DNS resolution,
//! the async `spawn_blocking` wrapper, the concurrency semaphore, and
//! conversion of `SsrfError` into `ToolError`. The private-TLD suffix
//! denylist, CGNAT/ULA ranges, and IPv4-mapped handling are all enforced by
//! the shared helpers.

use std::net::{IpAddr, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use labby_primitives::ssrf as shared;
use labby_runtime::error::ToolError;
use tokio::sync::Semaphore;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONCURRENT_DNS_VALIDATIONS: usize = 8;

/// Validate an externally supplied HTTPS URL using blocking DNS.
///
/// # Blocking
/// Call this only from blocking contexts. Async dispatch paths should call
/// [`validate_external_https_url`] instead.
pub fn validate_external_https_url_blocking(url: &str) -> Result<(), ToolError> {
    // Static checks (scheme/userinfo/query/fragment/host + IP-literal &
    // private-TLD denylist) are owned by the shared canonical guard. This
    // wrapper's stable contract is that *every* rejection surfaces as
    // `ssrf_blocked` (preserved from the pre-consolidation behavior and relied
    // on by the gateway/marketplace callers), so all `SsrfError` variants are
    // collapsed onto that kind here.
    let parsed = shared::parse_validated_https_url(url).map_err(as_ssrf_blocked)?;

    // An IP-literal host is fully validated by the static guard above.
    let host = parsed
        .host_str()
        .ok_or_else(|| ssrf_blocked("URL must include a host".to_string()))?;
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }

    // Resolve the name and validate every address via the shared IP guard.
    let redacted = shared::redact_url(url);
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| ssrf_blocked(format!("failed to resolve host `{host}`: {e}")))?;

    for sock_addr in addrs {
        shared::check_ip_not_private(sock_addr.ip(), &redacted).map_err(as_ssrf_blocked)?;
    }

    Ok(())
}

fn ssrf_blocked(message: String) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "ssrf_blocked".to_string(),
        message,
    }
}

/// Collapse any [`shared::SsrfError`] onto the `ssrf_blocked` kind, preserving
/// this wrapper's historic contract that all preflight rejections share one
/// stable kind regardless of whether the cause was a static URL defect or a
/// blocked address.
fn as_ssrf_blocked(e: shared::SsrfError) -> ToolError {
    ssrf_blocked(e.to_string())
}

/// Async wrapper for request/dispatch paths. Owns `spawn_blocking` and timeout
/// so async callers do not accidentally block runtime workers forever.
///
/// The timeout bounds the caller's wait, not the OS resolver call itself:
/// `getaddrinfo` cannot be cancelled once running. The semaphore is therefore
/// held by the blocking worker until DNS returns, preventing slow-host attempts
/// from growing the blocking pool without bound.
pub async fn validate_external_https_url(url: &str) -> Result<(), ToolError> {
    let url = url.to_string();
    let permit = tokio::time::timeout(
        DEFAULT_TIMEOUT,
        dns_validation_semaphore().clone().acquire_owned(),
    )
    .await
    .map_err(|_| ToolError::Sdk {
        sdk_kind: "ssrf_blocked".to_string(),
        message: "URL validation timed out waiting for DNS validation capacity".to_string(),
    })?
    .map_err(|_| ToolError::internal_message("DNS validation semaphore closed"))?;

    tokio::time::timeout(
        DEFAULT_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            validate_external_https_url_blocking(&url)
        }),
    )
    .await
    .map_err(|_| ToolError::Sdk {
        sdk_kind: "ssrf_blocked".to_string(),
        message: "URL validation timed out".to_string(),
    })?
    .map_err(|e| ToolError::internal_message(format!("SSRF validation task panicked: {e}")))?
}

fn dns_validation_semaphore() -> &'static Arc<Semaphore> {
    static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_DNS_VALIDATIONS)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_https_url() {
        let err = validate_external_https_url_blocking("http://example.com").unwrap_err();
        assert_eq!(err.kind(), "ssrf_blocked");
    }

    #[test]
    fn rejects_userinfo_query_and_fragment() {
        for url in [
            "https://user@example.com",
            "https://example.com/path?token=secret",
            "https://example.com/path#secret",
        ] {
            let err = validate_external_https_url_blocking(url).unwrap_err();
            assert_eq!(err.kind(), "ssrf_blocked");
            assert!(!err.user_message().contains("secret"));
        }
    }

    #[test]
    fn blocks_private_ranges_exactly() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.255",
            "::1",
            "fe80::1",
            "fc00::1",
            "fd00::1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
            "::ffff:100.64.0.1",
        ] {
            let parsed: IpAddr = ip.parse().expect(ip);
            let err = shared::check_ip_not_private(parsed, "https://example.com").unwrap_err();
            assert_eq!(err.kind(), "ssrf_blocked", "{ip}");
        }
    }
}
