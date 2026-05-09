use std::path::Path;

use crate::config::UpstreamConfig;

pub(crate) fn configured_bearer_token(env_name: &str) -> Option<String> {
    let dotenv_path = crate::config::dotenv_path();
    configured_bearer_token_with_dotenv(env_name, dotenv_path.as_deref())
}

fn configured_bearer_token_with_dotenv(
    env_name: &str,
    dotenv_path: Option<&Path>,
) -> Option<String> {
    let token = std::env::var(env_name).ok().or_else(|| {
        dotenv_path.and_then(|path| configured_bearer_token_from_dotenv_path(env_name, path))
    })?;
    normalize_bearer_token(&token)
}

fn configured_bearer_token_from_dotenv_path(env_name: &str, path: &Path) -> Option<String> {
    dotenvy::from_path_iter(path).ok().and_then(|iter| {
        iter.filter_map(Result::ok)
            .find_map(|(key, value)| (key == env_name).then_some(value))
            .and_then(|value| normalize_bearer_token(&value))
    })
}

fn normalize_bearer_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if token.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let raw = if token
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "))
    {
        token[7..].trim()
    } else {
        token
    };
    (!raw.is_empty()).then(|| raw.to_string())
}

fn configured_authorization_header_with_dotenv(
    env_name: &str,
    dotenv_path: Option<&Path>,
) -> Option<String> {
    configured_bearer_token_with_dotenv(env_name, dotenv_path)
        .map(|token| format!("Bearer {token}"))
}

pub(super) fn websocket_authorization_header(config: &UpstreamConfig) -> Option<String> {
    let dotenv_path = crate::config::dotenv_path();
    websocket_authorization_header_with_dotenv(config, dotenv_path.as_deref())
}

fn websocket_authorization_header_with_dotenv(
    config: &UpstreamConfig,
    dotenv_path: Option<&Path>,
) -> Option<String> {
    config
        .bearer_token_env
        .as_deref()
        .and_then(|env_name| configured_authorization_header_with_dotenv(env_name, dotenv_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_upstream_config() -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: None,
            bearer_token_env: None,
            command: None,
            args: vec![],
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            tool_search: crate::config::ToolSearchConfig::default(),
        }
    }

    #[test]
    fn configured_bearer_token_reads_and_normalizes_dotenv_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".env");
        std::fs::write(&path, "WS_TOKEN=\"Bearer dotenv-secret\"\nOTHER=ignored\n")
            .expect("write env");

        assert_eq!(
            configured_bearer_token_from_dotenv_path("WS_TOKEN", &path),
            Some("dotenv-secret".to_string())
        );
    }

    #[test]
    fn websocket_authorization_uses_dotenv_only_bearer_token() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".env");
        std::fs::write(&path, "WS_TOKEN=dotenv-secret\n").expect("write env");

        let mut config = test_upstream_config();
        config.url = Some("wss://upstream.example.com/mcp".into());
        config.bearer_token_env = Some("WS_TOKEN".into());

        assert_eq!(
            websocket_authorization_header_with_dotenv(&config, Some(&path)),
            Some("Bearer dotenv-secret".to_string())
        );
    }

    #[test]
    fn normalize_bearer_token_rejects_empty_values() {
        assert_eq!(normalize_bearer_token("   "), None);
        assert_eq!(normalize_bearer_token("Bearer   "), None);
        assert_eq!(
            normalize_bearer_token(" raw-token "),
            Some("raw-token".to_string())
        );
        assert_eq!(
            normalize_bearer_token("Bearer raw-token"),
            Some("raw-token".to_string())
        );
    }
}
