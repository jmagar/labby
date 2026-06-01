//! Upstream config validation.
//!
//! `validate_upstream_config` enforces the supported transport invariants:
//! non-empty name, mutually-exclusive `url`/`command`, supported URL scheme, and
//! rejection of bind-all hosts. `pub(super)` for the pool module and descendants.

use crate::config::UpstreamConfig;

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
            tool_search: crate::config::ToolSearchConfig::default(),
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
            tool_search: crate::config::ToolSearchConfig::default(),
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
                tool_search: crate::config::ToolSearchConfig::default(),
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
            tool_search: crate::config::ToolSearchConfig::default(),
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
                tool_search: crate::config::ToolSearchConfig::default(),
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
            tool_search: crate::config::ToolSearchConfig::default(),
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
            tool_search: crate::config::ToolSearchConfig::default(),
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
            tool_search: crate::config::ToolSearchConfig::default(),
        };
        assert!(validate_upstream_config(&config).is_err());
    }
}
