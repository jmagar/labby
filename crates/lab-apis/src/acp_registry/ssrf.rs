//! Canonical SSRF preflight guards for externally supplied HTTPS URLs.
//!
//! This is the single source of truth for SSRF host/IP filtering across the
//! workspace. The `lab` binary's `dispatch::security::ssrf` module delegates
//! to these helpers so that all callers — the ACP archive installer here and
//! the gateway/marketplace dispatch paths in `lab` — share one allow/deny
//! policy. (Dependency direction only allows `lab -> lab-apis`, so the
//! canonical guard must live here, not in dispatch.)
//!
//! It is a *preflight* guard, not a complete DNS-rebinding defense. Any code
//! that performs an outbound request must still avoid unsafe redirects and
//! must re-validate the connected peer where it can (see
//! [`super::installer`], which pins a single validated address and re-checks
//! the peer IP post-connect).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Reason an externally supplied URL was rejected by the SSRF preflight.
///
/// Carries no caller secrets — messages are built from already-redacted URL
/// forms / bare host strings. Wrapped into surface error types (`ToolError`,
/// [`AcpInstallerError`](super::installer::AcpInstallerError)) by callers; the
/// stable error `kind` for all variants is `ssrf_blocked` except
/// [`SsrfError::InvalidUrl`] which is `invalid_param`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SsrfError {
    /// URL could not be parsed, used a non-https scheme, lacked a host/port,
    /// or carried forbidden userinfo/query/fragment components.
    #[error("{0}")]
    InvalidUrl(String),
    /// Host resolved (or parsed) to a private/loopback/link-local/CGNAT/ULA
    /// address, or matched a private-TLD suffix denylist.
    #[error("{0}")]
    Blocked(String),
}

impl SsrfError {
    /// Stable kind tag mirroring the dispatcher error vocabulary.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidUrl(_) => "invalid_param",
            Self::Blocked(_) => "ssrf_blocked",
        }
    }
}

/// Private-DNS suffix denylist applied to non-IP hosts.
///
/// These are common homelab/corporate internal TLDs that should never be the
/// target of an externally-supplied archive/registry URL. Belt-and-suspenders
/// on top of the resolved-IP checks (an internal name might resolve through
/// split-horizon DNS to a public-looking record at validation time).
pub const PRIVATE_TLD_SUFFIXES: &[&str] =
    &[".local", ".internal", ".lan", ".intranet", ".corp", ".home"];

/// Returns `true` for the IPv4 carrier-grade NAT range `100.64.0.0/10`.
#[must_use]
pub fn is_cgnat(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

fn is_ipv6_link_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn is_ipv6_ula(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// Reject an IP that targets private, loopback, link-local, CGNAT, ULA, or
/// IPv4-mapped-private space. `context` is a non-secret label (redacted URL or
/// bare host) used only to build the error message.
///
/// # Errors
/// Returns [`SsrfError::Blocked`] when `ip` falls in any blocked range.
pub fn check_ip_not_private(ip: IpAddr, context: &str) -> Result<(), SsrfError> {
    // Normalize IPv4-mapped IPv6 (`::ffff:a.b.c.d`) down to V4 so the V4
    // private/loopback/cgnat checks apply (Rust's IPv6 helpers don't cover the
    // mapped form).
    let normalized = match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        other => other,
    };

    let blocked = match normalized {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || is_cgnat(v4)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || is_ipv6_link_local(v6) || is_ipv6_ula(v6)
        }
    };

    if blocked {
        return Err(SsrfError::Blocked(format!(
            "`{context}` resolves to a private, loopback, link-local, CGNAT, or ULA address {ip}; blocked to prevent SSRF"
        )));
    }

    Ok(())
}

/// Reject a non-IP host whose name ends in one of [`PRIVATE_TLD_SUFFIXES`],
/// or which is a textual loopback/unspecified form. IP-literal hosts should be
/// routed through [`check_ip_not_private`] instead.
///
/// # Errors
/// Returns [`SsrfError::Blocked`] for a denylisted host name.
pub fn check_host_not_private(host: &str) -> Result<(), SsrfError> {
    let host_lower = host.to_ascii_lowercase();
    if host_lower == "localhost"
        || host_lower.starts_with("127.")
        || host_lower == "::1"
        || host_lower.contains("::ffff:")
        || host_lower == "0.0.0.0"
        || PRIVATE_TLD_SUFFIXES.iter().any(|s| host_lower.ends_with(s))
    {
        return Err(SsrfError::Blocked(format!(
            "host `{host}` is a local/loopback/private address"
        )));
    }
    Ok(())
}

/// Strip userinfo/query/fragment from a URL for safe inclusion in error text.
#[must_use]
pub fn redact_url(raw: &str) -> String {
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

/// Parse and statically validate an HTTPS URL: require https, forbid userinfo,
/// query, and fragment, require a host and resolvable port, and reject the
/// private-TLD/loopback host denylist. If the host is an IP literal it is
/// additionally run through [`check_ip_not_private`].
///
/// This performs **no DNS** — it is the synchronous, allocation-light portion
/// shared by both the blocking validator in `lab` and the installer here.
/// Callers that need resolved-address filtering must follow up with DNS +
/// [`check_ip_not_private`] on each resolved address.
///
/// # Errors
/// Returns [`SsrfError`] when any static rule is violated.
pub fn parse_validated_https_url(url: &str) -> Result<url::Url, SsrfError> {
    let redacted = redact_url(url);
    let parsed = url::Url::parse(url)
        .map_err(|e| SsrfError::InvalidUrl(format!("invalid URL `{redacted}`: {e}")))?;

    if parsed.scheme() != "https" {
        return Err(SsrfError::InvalidUrl(format!(
            "URL `{redacted}` must use https to prevent SSRF"
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(SsrfError::InvalidUrl(format!(
            "URL `{redacted}` must not include userinfo"
        )));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(SsrfError::InvalidUrl(format!(
            "URL `{redacted}` must not include query or fragment components"
        )));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| SsrfError::InvalidUrl(format!("URL `{redacted}` must include a host")))?;
    parsed
        .port_or_known_default()
        .ok_or_else(|| SsrfError::InvalidUrl(format!("URL `{redacted}` must include a port")))?;

    check_host_not_private(host)?;
    if let Ok(addr) = host.parse::<IpAddr>() {
        check_ip_not_private(addr, &redacted)?;
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "::ffff:169.254.169.254",
        ] {
            let parsed: IpAddr = ip.parse().expect(ip);
            let err = check_ip_not_private(parsed, "registry.example.com").unwrap_err();
            assert_eq!(err.kind(), "ssrf_blocked", "{ip}");
        }
    }

    #[test]
    fn allows_public_addresses() {
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            let parsed: IpAddr = ip.parse().expect(ip);
            check_ip_not_private(parsed, "cdn.example.com").expect(ip);
        }
    }

    #[test]
    fn rejects_non_https_as_invalid_param() {
        // Scheme defect is a static URL problem, not an address block.
        let err = parse_validated_https_url("http://example.com/agent.tar.gz").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn rejects_private_and_loopback_hosts_as_blocked() {
        // Private-TLD suffix, textual loopback, and private/loopback IP
        // literals are all address-policy rejections → `ssrf_blocked`.
        for url in [
            "https://agent.local/agent.tar.gz",
            "https://127.0.0.1/agent.tar.gz",
            "https://[::ffff:127.0.0.1]/agent.tar.gz",
            "https://192.168.1.20/agent.tar.gz",
        ] {
            let err = parse_validated_https_url(url).unwrap_err();
            assert_eq!(err.kind(), "ssrf_blocked", "{url}");
        }
    }

    #[test]
    fn rejects_userinfo_query_and_fragment_without_leaking_secret() {
        for url in [
            "https://user@example.com/a.tar.gz",
            "https://example.com/a.tar.gz?token=secret",
            "https://example.com/a.tar.gz#secret",
        ] {
            let err = parse_validated_https_url(url).unwrap_err();
            assert_eq!(err.kind(), "invalid_param");
            assert!(!err.to_string().contains("secret"), "{url}");
        }
    }

    #[test]
    fn private_tld_suffixes_are_blocked() {
        for host in [
            "box.local",
            "svc.internal",
            "host.lan",
            "x.intranet",
            "y.corp",
            "z.home",
        ] {
            let err = check_host_not_private(host).unwrap_err();
            assert_eq!(err.kind(), "ssrf_blocked", "{host}");
        }
    }
}
