//! Upstream config validation.
//!
//! `validate_upstream_config` enforces the supported transport invariants:
//! non-empty name, mutually-exclusive `url`/`command`, supported URL scheme, and
//! rejection of bind-all hosts. `pub(super)` for the pool module and descendants.
//!
//! ## SSRF / DNS rebinding posture (S4 + T6)
//!
//! **Homelab context:** operator-configured upstreams at loopback and RFC1918
//! addresses (e.g. `http://localhost:3100`, `http://192.168.1.50/mcp`) are
//! **legitimate** and must keep working. `validate_upstream_config` therefore
//! does NOT reject private/loopback hosts — it only rejects bind-all
//! (`0.0.0.0` / `::`) which are never valid upstream targets.
//!
//! **OAuth-probe → connect gap (residual):** an upstream whose URL was
//! validated as a public hostname at probe time could later rebind to a
//! private address at connect time (DNS rebinding attack).  For the
//! non-OAuth pool path this is an accepted residual — the threat model
//! requires admin-level write access to add a hostname-based upstream, and
//! homelab operators intentionally point upstreams at LAN hosts.
//!
//! For the OAuth path (`connect_http_upstream` with `config.oauth.is_some()`),
//! the connect happens per-request under a subject token — a narrower window.
//! Full IP-pinning (resolve once, compare at connect) would require patching
//! the `reqwest::Client` DNS layer or wrapping the OAuth connect with a
//! pre-resolve step; that is tracked as follow-up work.  See
//! `dispatch/upstream/CLAUDE.md` for the documented residual.
//!
//! `validate_upstream_config` is the right place to add stricter checks
//! (e.g. a `--strict-ssrf` flag that rejects RFC1918 in the gateway's
//! public-registry import path) without touching legitimate homelab paths.

use lab_runtime::gateway_config::UpstreamConfig;

/// Validate an upstream config entry.
pub(super) fn validate_upstream_config(config: &UpstreamConfig) -> Result<(), String> {
    if config.name.is_empty() {
        return Err("upstream name cannot be empty".into());
    }

    if config.url.is_some() && config.command.is_some() {
        return Err("upstream must not set both 'url' and 'command'".into());
    }

    // Must have either a URL or a command
    if config.url.is_none() && config.command.is_none() {
        return Err("upstream must have either 'url' or 'command'".into());
    }

    if let Some(ref url_str) = config.url {
        // Reject schemes outside the supported HTTP and WebSocket transports.
        if !url_str.starts_with("http://")
            && !url_str.starts_with("https://")
            && !url_str.starts_with("ws://")
            && !url_str.starts_with("wss://")
        {
            return Err(format!(
                "upstream URL must use http://, https://, ws://, or wss:// scheme, got: {url_str}"
            ));
        }
        // Parse with url::Url to reliably check the host.
        let parsed = url::Url::parse(url_str)
            .map_err(|e| format!("invalid upstream URL `{url_str}`: {e}"))?;
        if let Some(host) = parsed.host_str() {
            // Reject bind-all addresses (0.0.0.0 or ::).
            let normalized = host.trim_start_matches('[').trim_end_matches(']');
            if normalized == "0.0.0.0" || normalized == "::" {
                return Err("upstream URL must not use 0.0.0.0 or :: (bind-all addresses)".into());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_empty_name() {
        let config = UpstreamConfig {
            enabled: true,
            name: String::new(),
            url: Some("http://localhost:8080".into()),
            bearer_token_env: None,
            command: None,
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        assert!(validate_upstream_config(&config).is_err());
    }

    #[test]
    fn validate_rejects_non_http_scheme() {
        let config = UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: Some("ftp://example.com".into()),
            bearer_token_env: None,
            command: None,
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        assert!(validate_upstream_config(&config).is_err());
    }

    #[test]
    fn validate_rejects_bind_all_addresses() {
        for url in &["http://0.0.0.0:8080", "http://[::]/mcp", "http://[::]:8080"] {
            let config = UpstreamConfig {
                enabled: true,
                name: "test".into(),
                url: Some((*url).into()),
                bearer_token_env: None,
                command: None,
                args: vec![],
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            };
            assert!(
                validate_upstream_config(&config).is_err(),
                "should reject {url}"
            );
        }
    }

    #[test]
    fn validate_accepts_valid_http_url() {
        let config = UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: Some("http://localhost:8080/mcp".into()),
            bearer_token_env: None,
            command: None,
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        assert!(validate_upstream_config(&config).is_ok());
    }

    #[test]
    fn validate_accepts_valid_websocket_urls() {
        for url in ["ws://localhost:8080/mcp", "wss://example.com/socket"] {
            let config = UpstreamConfig {
                enabled: true,
                name: "test".into(),
                url: Some(url.into()),
                bearer_token_env: None,
                command: None,
                args: vec![],
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            };
            assert!(
                validate_upstream_config(&config).is_ok(),
                "{url} should validate"
            );
        }
    }

    #[test]
    fn validate_accepts_stdio_command() {
        let config = UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: None,
            bearer_token_env: None,
            command: Some("my-mcp-server".into()),
            args: vec!["--port".into(), "8080".into()],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        assert!(validate_upstream_config(&config).is_ok());
    }

    #[test]
    fn validate_rejects_both_url_and_command() {
        let config = UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: Some("http://localhost:8080".into()),
            bearer_token_env: None,
            command: Some("my-mcp-server".into()),
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        assert!(validate_upstream_config(&config).is_err());
    }

    #[test]
    fn validate_rejects_no_url_or_command() {
        let config = UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: None,
            bearer_token_env: None,
            command: None,
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        assert!(validate_upstream_config(&config).is_err());
    }

    // ── S4 / T6: DNS rebinding posture documentation + bind-all regression ──

    /// T6: `validate_upstream_config` rejects the two canonical bind-all
    /// addresses (`0.0.0.0` and `::`).  These are never valid upstream targets
    /// and indicate a misconfigured or adversarial config.
    ///
    /// RFC1918 and loopback addresses (e.g. `http://localhost`, `http://192.168.1.1`)
    /// are intentionally accepted — operator-configured homelab upstreams
    /// legitimately point at private hosts.  See the module-level SSRF comment
    /// for the documented residual (DNS rebinding on the OAuth-probe→connect path).
    #[test]
    fn validate_rejects_bind_all_but_accepts_private_and_loopback() {
        // Bind-all must always be rejected.
        for url in &["http://0.0.0.0:8080/mcp", "http://[::]/mcp"] {
            let config = UpstreamConfig {
                enabled: true,
                name: "test".into(),
                url: Some((*url).into()),
                bearer_token_env: None,
                command: None,
                args: vec![],
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            };
            assert!(
                validate_upstream_config(&config).is_err(),
                "bind-all {url} must be rejected"
            );
        }

        // Loopback and RFC1918 are legitimate homelab upstream addresses — must
        // be accepted.  Rejecting them would break every operator who runs a
        // local MCP server (e.g. cortex at http://localhost:3100).
        for url in &[
            "http://localhost:3100/mcp",
            "http://127.0.0.1:8080/mcp",
            "http://192.168.1.50/mcp",
            "http://10.0.0.1:9000/mcp",
            "http://172.16.0.5/mcp",
        ] {
            let config = UpstreamConfig {
                enabled: true,
                name: "test".into(),
                url: Some((*url).into()),
                bearer_token_env: None,
                command: None,
                args: vec![],
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            };
            assert!(
                validate_upstream_config(&config).is_ok(),
                "private/loopback {url} must be accepted for homelab use"
            );
        }
    }

    /// T6 (residual documentation): demonstrates that the DNS rebinding gap
    /// exists at the validation layer — a hostname that DNS-resolves to a public
    /// IP at probe time could rebind to a private IP at connect time.
    ///
    /// `validate_upstream_config` operates on the *configured URL string* before
    /// any DNS resolution, so it cannot detect this class of attack.  The fix
    /// (IP pinning between probe and connect) is tracked as follow-up work; this
    /// test documents the residual so future reviewers understand the boundary.
    ///
    /// What we DO enforce: the validation layer rejects bind-all addresses and
    /// bad schemes.  The admin-write trust boundary is the primary SSRF defence
    /// for the homelab use case.
    #[test]
    fn validate_cannot_detect_dns_rebinding_residual_documented() {
        // A hostname-based URL that would validate successfully even though
        // it could theoretically rebind.  This is expected/acceptable for the
        // homelab trust model — the test exists to document the residual, not
        // to assert a failure.
        let config = UpstreamConfig {
            enabled: true,
            name: "rebind-risk-documented".into(),
            url: Some("http://mcp.example.com/mcp".into()),
            bearer_token_env: None,
            command: None,
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        // Hostname-based URLs pass validation.  DNS is NOT resolved here.
        // A bind-all address in the hostname position IS still rejected.
        assert!(
            validate_upstream_config(&config).is_ok(),
            "hostname-based URL must pass config validation (DNS rebinding is a residual gap, \
             not a config-layer concern for the homelab trust model)"
        );
    }
}
