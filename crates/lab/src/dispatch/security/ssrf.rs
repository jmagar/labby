//! Shared SSRF preflight guards for externally supplied HTTPS URLs.
//!
//! This is a preflight guard, not a complete DNS-rebinding defense. Any code
//! that later performs an outbound request must still avoid unsafe redirects and
//! must not claim that validation-time DNS pins the final connection target.

use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use crate::dispatch::error::ToolError;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Validate an externally supplied HTTPS URL using blocking DNS.
///
/// # Blocking
/// Call this only from blocking contexts. Async dispatch paths should call
/// [`validate_external_https_url`] instead.
pub fn validate_external_https_url_blocking(url: &str) -> Result<(), ToolError> {
    let redacted = redact_url_for_error(url);
    let ssrf_err = |msg: String| ToolError::Sdk {
        sdk_kind: "ssrf_blocked".to_string(),
        message: msg,
    };

    let parsed =
        url::Url::parse(url).map_err(|e| ssrf_err(format!("invalid URL `{redacted}`: {e}")))?;

    if parsed.scheme() != "https" {
        return Err(ssrf_err(format!(
            "URL `{redacted}` must use https to prevent SSRF"
        )));
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ssrf_err(format!(
            "URL `{redacted}` must not include userinfo"
        )));
    }

    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ssrf_err(format!(
            "URL `{redacted}` must not include query or fragment components"
        )));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| ssrf_err(format!("URL `{redacted}` must include a host")))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| ssrf_err(format!("URL `{redacted}` must include a resolvable port")))?;

    if let Ok(addr) = host.parse::<IpAddr>() {
        return check_ip_not_private(addr, &redacted);
    }

    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| ssrf_err(format!("failed to resolve host `{host}`: {e}")))?;

    for sock_addr in addrs {
        check_ip_not_private(sock_addr.ip(), &redacted)?;
    }

    Ok(())
}

/// Async wrapper for request/dispatch paths. Owns `spawn_blocking` and timeout
/// so async callers do not accidentally block runtime workers forever.
pub async fn validate_external_https_url(url: &str) -> Result<(), ToolError> {
    let url = url.to_string();
    tokio::time::timeout(
        DEFAULT_TIMEOUT,
        tokio::task::spawn_blocking(move || validate_external_https_url_blocking(&url)),
    )
    .await
    .map_err(|_| ToolError::Sdk {
        sdk_kind: "ssrf_blocked".to_string(),
        message: "URL validation timed out".to_string(),
    })?
    .map_err(|e| ToolError::internal_message(format!("SSRF validation task panicked: {e}")))?
}

fn check_ip_not_private(ip: IpAddr, redacted_url: &str) -> Result<(), ToolError> {
    let blocked = match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || is_cgnat(v4)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || is_ipv6_link_local(v6)
                || is_ipv6_ula(v6)
                || v6.to_ipv4_mapped().is_some_and(|v4| {
                    v4.is_private()
                        || v4.is_loopback()
                        || v4.is_link_local()
                        || v4.is_unspecified()
                        || is_cgnat(v4)
                })
        }
    };

    if blocked {
        return Err(ToolError::Sdk {
            sdk_kind: "ssrf_blocked".to_string(),
            message: format!(
                "URL `{redacted_url}` resolves to private, loopback, link-local, CGNAT, or ULA address {ip}; blocked to prevent SSRF"
            ),
        });
    }

    Ok(())
}

fn is_cgnat(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

fn is_ipv6_link_local(ip: std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn is_ipv6_ula(ip: std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn redact_url_for_error(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => "<invalid-url>".to_string(),
    }
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
            let err = check_ip_not_private(parsed, "https://example.com").unwrap_err();
            assert_eq!(err.kind(), "ssrf_blocked", "{ip}");
        }
    }
}
