use std::net::IpAddr;

use url::Host;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::require_str;

pub struct ServiceProbeParams<'a> {
    pub service: &'a str,
    pub instance: Option<&'a str>,
}

pub fn parse_service_probe(
    params: &serde_json::Value,
) -> Result<ServiceProbeParams<'_>, ToolError> {
    let service = require_str(params, "service")?;
    // Reject any URL in params — resolution must come from env (SSRF defense).
    if service.starts_with("http://") || service.starts_with("https://") {
        return Err(ToolError::InvalidParam {
            message: "service must be a service name, not a URL".to_string(),
            param: "service".to_string(),
        });
    }
    let instance = params
        .get("instance")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty());
    Ok(ServiceProbeParams { service, instance })
}

#[derive(Debug)]
pub struct ProxyCheckParams<'a> {
    pub app_url: &'a str,
    pub mcp_url: &'a str,
    pub route: &'a str,
    /// Optional private backend origin for the backend-leak probe.
    /// When present, the probe verifies this origin does not appear in
    /// public error response bodies.
    pub backend_url: Option<&'a str>,
}

pub fn parse_proxy_check(params: &serde_json::Value) -> Result<ProxyCheckParams<'_>, ToolError> {
    let app_url = require_str(params, "app_url")?;
    let mcp_url = require_str(params, "mcp_url")?;
    let route = require_str(params, "route")?;
    if !route.starts_with('/') {
        return Err(ToolError::InvalidParam {
            message: "route must start with /".to_string(),
            param: "route".to_string(),
        });
    }
    if route.len() > 1 && route.ends_with('/') {
        return Err(ToolError::InvalidParam {
            message: "route must not end with /".to_string(),
            param: "route".to_string(),
        });
    }
    if route.contains('?') || route.contains('#') {
        return Err(ToolError::InvalidParam {
            message: "route must be a path without query or fragment".to_string(),
            param: "route".to_string(),
        });
    }
    for (param, value) in [("app_url", app_url), ("mcp_url", mcp_url)] {
        let parsed = url::Url::parse(value).map_err(|error| ToolError::InvalidParam {
            message: format!("{param} must be a valid URL: {error}"),
            param: param.to_string(),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(ToolError::InvalidParam {
                message: format!("{param} must be an http(s) URL with a host"),
                param: param.to_string(),
            });
        }
        validate_public_proxy_url(param, &parsed)?;
    }
    let backend_url = params
        .get("backend_url")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty());
    if let Some(backend_url) = backend_url {
        let parsed = url::Url::parse(backend_url).map_err(|error| ToolError::InvalidParam {
            message: format!("backend_url must be a valid URL: {error}"),
            param: "backend_url".to_string(),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(ToolError::InvalidParam {
                message: "backend_url must be an http(s) URL with a host".to_string(),
                param: "backend_url".to_string(),
            });
        }
    }
    Ok(ProxyCheckParams {
        app_url,
        mcp_url,
        route,
        backend_url,
    })
}

fn validate_public_proxy_url(param: &str, parsed: &url::Url) -> Result<(), ToolError> {
    let Some(host) = parsed.host() else {
        return Err(ToolError::InvalidParam {
            message: format!("{param} must be an http(s) URL with a host"),
            param: param.to_string(),
        });
    };
    let is_localhost = matches!(host, Host::Domain(domain) if domain.eq_ignore_ascii_case("localhost") || domain.to_ascii_lowercase().ends_with(".localhost"));
    let is_blocked_ip = match host {
        Host::Domain(_) => false,
        Host::Ipv4(ip) => is_private_proxy_ip(IpAddr::V4(ip)),
        Host::Ipv6(ip) => is_private_proxy_ip(IpAddr::V6(ip)),
    };
    if is_localhost || is_blocked_ip {
        return Err(ToolError::InvalidParam {
            message: format!("{param} must be a public proxy URL, not a local or private address"),
            param: param.to_string(),
        });
    }
    Ok(())
}

fn is_private_proxy_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy_params(route: &str) -> serde_json::Value {
        serde_json::json!({
            "app_url": "https://lab.example.test",
            "mcp_url": "https://mcp.example.test",
            "route": route,
        })
    }

    #[test]
    fn parse_proxy_check_rejects_ambiguous_route_variants() {
        for route in [
            "syslog",
            "/syslog/",
            "/syslog?debug=true",
            "/syslog#fragment",
        ] {
            let err = parse_proxy_check(&proxy_params(route)).expect_err("route should fail");
            assert_eq!(err.kind(), "invalid_param");
        }
    }

    #[test]
    fn parse_proxy_check_rejects_private_ipv6_proxy_urls() {
        for value in ["https://[::1]", "https://[fc00::1]", "https://[fe80::1]"] {
            let params = serde_json::json!({
                "app_url": value,
                "mcp_url": "https://mcp.example.test",
                "route": "/syslog",
            });
            let err = parse_proxy_check(&params).expect_err("private IPv6 should fail");
            assert_eq!(err.kind(), "invalid_param", "{value}");
        }
    }
}
