use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use anyhow::Context;
use fd_lock::RwLock;
use tempfile::NamedTempFile;

use crate::config::{LabConfig, ProtectedMcpRouteConfig, UpstreamConfig};
use crate::dispatch::error::ToolError;

use super::params::GatewayUpdatePatch;

pub fn load_gateway_config(path: &Path) -> Result<LabConfig, ToolError> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let mut cfg = toml::from_str::<LabConfig>(&raw).map_err(|e| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to parse {}: {e}", path.display()),
            })?;
            cfg.normalize_legacy_tool_search(crate::config::root_tool_search_present(&raw));
            cfg.normalize_protected_mcp_routes()
                .map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: format!("invalid config {}: {e}", path.display()),
                })?;
            normalize_config(&mut cfg)?;
            validate_config(&cfg)?;
            Ok(cfg)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LabConfig::default()),
        Err(e) => Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to read {}: {e}", path.display()),
        }),
    }
}

/// Serialize `cfg` to TOML and atomically replace the file at `path`.
///
/// **Limitation:** This serializes the full `LabConfig` struct via `toml::to_string`,
/// which means any unknown keys, TOML comments, or settings from newer schema
/// versions that are not represented in `LabConfig` will be dropped on write.
/// A future migration to `toml_edit` would preserve unknown keys and comments,
/// but that is deferred as a P2 change.
pub fn write_gateway_config(path: &Path, cfg: &LabConfig) -> Result<(), ToolError> {
    validate_config(cfg)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create {}: {e}", parent.display()),
        })?;
    }

    let lock_path = lock_path(path);
    let lock_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open {}", lock_path.display()))
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: e.to_string(),
        })?;
    let mut lock = RwLock::new(lock_file);
    let _guard = lock.try_write().map_err(|_| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("gateway config is locked: {}", lock_path.display()),
    })?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let raw = toml::to_string(cfg).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to serialize gateway config: {e}"),
    })?;

    let mut tmp = NamedTempFile::new_in(parent).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to create temp file in {}: {e}", parent.display()),
    })?;
    use std::io::Write as _;
    tmp.write_all(raw.as_bytes()).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to write temp gateway config: {e}"),
    })?;
    tmp.as_file().sync_all().map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to sync temp gateway config: {e}"),
    })?;
    tmp.persist(path).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to persist {}: {}", path.display(), e.error),
    })?;

    Ok(())
}

pub fn insert_upstream(cfg: &mut LabConfig, upstream: UpstreamConfig) -> Result<(), ToolError> {
    validate_upstream(&upstream)?;
    if cfg
        .upstream
        .iter()
        .any(|existing| existing.name == upstream.name)
    {
        return Err(ToolError::Conflict {
            message: format!("A gateway named {} already exists.", upstream.name),
            existing_id: upstream.name.clone(),
        });
    }
    cfg.upstream.push(upstream);
    Ok(())
}

pub fn update_upstream(
    cfg: &mut LabConfig,
    name: &str,
    patch: GatewayUpdatePatch,
) -> Result<(), ToolError> {
    let index = cfg
        .upstream
        .iter()
        .position(|existing| existing.name == name)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("gateway `{name}` not found"),
        })?;

    if let Some(new_name) = patch.name {
        if new_name != name
            && cfg
                .upstream
                .iter()
                .any(|existing| existing.name == new_name)
        {
            return Err(ToolError::InvalidParam {
                message: format!("gateway `{new_name}` already exists"),
                param: "name".to_string(),
            });
        }
        cfg.upstream[index].name = new_name;
    }
    if let Some(enabled) = patch.enabled {
        cfg.upstream[index].enabled = enabled;
    }
    if let Some(url) = patch.url {
        cfg.upstream[index].url = url;
    }
    if let Some(command) = patch.command {
        cfg.upstream[index].command = command;
    }
    if let Some(args) = patch.args {
        cfg.upstream[index].args = args;
    }
    if let Some(bearer_token_env) = patch.bearer_token_env {
        cfg.upstream[index].bearer_token_env = bearer_token_env;
    }
    if let Some(proxy_resources) = patch.proxy_resources {
        cfg.upstream[index].proxy_resources = proxy_resources;
    }
    if let Some(proxy_prompts) = patch.proxy_prompts {
        cfg.upstream[index].proxy_prompts = proxy_prompts;
    }
    if let Some(expose_tools) = patch.expose_tools {
        // Treat empty array as "clear filter" — an empty allowlist that blocks
        // all tools is never useful and is the natural way to say "remove filter".
        cfg.upstream[index].expose_tools = match expose_tools {
            Some(ref v) if v.is_empty() => None,
            other => other,
        };
    }
    if let Some(expose_resources) = patch.expose_resources {
        cfg.upstream[index].expose_resources = match expose_resources {
            Some(ref v) if v.is_empty() => None,
            other => other,
        };
    }
    if let Some(expose_prompts) = patch.expose_prompts {
        cfg.upstream[index].expose_prompts = match expose_prompts {
            Some(ref v) if v.is_empty() => None,
            other => other,
        };
    }
    if let Some(oauth) = patch.oauth {
        cfg.upstream[index].oauth = oauth;
    }
    if patch.tool_search.is_some() {
        return Err(ToolError::InvalidParam {
            message:
                "tool_search is gateway-wide; use gateway.tool_search.set instead of gateway.update"
                    .to_string(),
            param: "tool_search".to_string(),
        });
    }

    validate_upstream(&cfg.upstream[index])?;
    Ok(())
}

pub fn remove_upstream(cfg: &mut LabConfig, name: &str) -> Result<UpstreamConfig, ToolError> {
    let index = cfg
        .upstream
        .iter()
        .position(|existing| existing.name == name)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("gateway `{name}` not found"),
        })?;
    Ok(cfg.upstream.remove(index))
}

pub fn insert_protected_mcp_route(
    cfg: &mut LabConfig,
    mut route: ProtectedMcpRouteConfig,
) -> Result<ProtectedMcpRouteConfig, ToolError> {
    normalize_protected_mcp_route(&mut route)?;
    validate_protected_mcp_route(&route)?;
    if cfg
        .protected_mcp_routes
        .iter()
        .any(|existing| existing.name == route.name)
    {
        return Err(ToolError::Conflict {
            message: format!("protected MCP route `{}` already exists", route.name),
            existing_id: route.name.clone(),
        });
    }
    if cfg
        .protected_mcp_routes
        .iter()
        .filter(|existing| existing.enabled && route.enabled)
        .any(|existing| {
            existing
                .public_host
                .eq_ignore_ascii_case(&route.public_host)
                && existing.public_path == route.public_path
        })
    {
        return Err(ToolError::Conflict {
            message: format!(
                "protected MCP route for {}{} already exists",
                route.public_host, route.public_path
            ),
            existing_id: route.name.clone(),
        });
    }
    cfg.protected_mcp_routes.push(route.clone());
    Ok(route)
}

pub fn update_protected_mcp_route(
    cfg: &mut LabConfig,
    name: &str,
    mut route: ProtectedMcpRouteConfig,
) -> Result<ProtectedMcpRouteConfig, ToolError> {
    let index = cfg
        .protected_mcp_routes
        .iter()
        .position(|existing| existing.name == name)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("protected MCP route `{name}` not found"),
        })?;
    normalize_protected_mcp_route(&mut route)?;
    validate_protected_mcp_route(&route)?;
    if route.name != name
        && cfg
            .protected_mcp_routes
            .iter()
            .any(|existing| existing.name == route.name)
    {
        return Err(ToolError::Conflict {
            message: format!("protected MCP route `{}` already exists", route.name),
            existing_id: route.name.clone(),
        });
    }
    if cfg
        .protected_mcp_routes
        .iter()
        .enumerate()
        .filter(|(existing_index, existing)| {
            *existing_index != index && existing.enabled && route.enabled
        })
        .any(|(_, existing)| {
            existing
                .public_host
                .eq_ignore_ascii_case(&route.public_host)
                && existing.public_path == route.public_path
        })
    {
        return Err(ToolError::Conflict {
            message: format!(
                "protected MCP route for {}{} already exists",
                route.public_host, route.public_path
            ),
            existing_id: route.name.clone(),
        });
    }
    cfg.protected_mcp_routes[index] = route.clone();
    Ok(route)
}

pub fn remove_protected_mcp_route(
    cfg: &mut LabConfig,
    name: &str,
) -> Result<ProtectedMcpRouteConfig, ToolError> {
    let index = cfg
        .protected_mcp_routes
        .iter()
        .position(|existing| existing.name == name)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("protected MCP route `{name}` not found"),
        })?;
    Ok(cfg.protected_mcp_routes.remove(index))
}

pub fn validate_protected_mcp_routes(routes: &[ProtectedMcpRouteConfig]) -> Result<(), ToolError> {
    let mut names = std::collections::HashSet::new();
    let mut enabled_keys = std::collections::HashSet::new();
    for route in routes {
        validate_protected_mcp_route(route)?;
        if !names.insert(route.name.clone()) {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "protected MCP route `{}` appears more than once",
                    route.name
                ),
                param: "name".to_string(),
            });
        }
        if route.enabled {
            let key = (
                route.public_host.to_ascii_lowercase(),
                route.public_path.clone(),
            );
            if !enabled_keys.insert(key) {
                return Err(ToolError::InvalidParam {
                    message: format!(
                        "duplicate enabled protected MCP route for {}{}",
                        route.public_host, route.public_path
                    ),
                    param: "public_path".to_string(),
                });
            }
        }
    }
    Ok(())
}

fn validate_config(cfg: &LabConfig) -> Result<(), ToolError> {
    validate_tool_search(&cfg.tool_search)?;
    validate_upstreams(&cfg.upstream)?;
    validate_protected_mcp_routes(&cfg.protected_mcp_routes)
}

fn normalize_config(cfg: &mut LabConfig) -> Result<(), ToolError> {
    for route in &mut cfg.protected_mcp_routes {
        normalize_protected_mcp_route(route)?;
    }
    Ok(())
}

pub fn validate_tool_search(
    tool_search: &crate::config::ToolSearchConfig,
) -> Result<(), ToolError> {
    tool_search.validate().map_err(|e| match e {
        crate::config::ConfigError::InvalidToolSearchTopKDefault { .. } => {
            ToolError::InvalidParam {
                message: e.to_string(),
                param: "tool_search.top_k_default".to_string(),
            }
        }
        crate::config::ConfigError::InvalidToolSearchMaxTools { .. } => ToolError::InvalidParam {
            message: e.to_string(),
            param: "tool_search.max_tools".to_string(),
        },
        _ => ToolError::InvalidParam {
            message: e.to_string(),
            param: "tool_search".to_string(),
        },
    })
}

fn validate_upstreams(upstreams: &[UpstreamConfig]) -> Result<(), ToolError> {
    let mut names = std::collections::HashSet::new();
    for upstream in upstreams {
        validate_upstream(upstream)?;
        if !names.insert(upstream.name.clone()) {
            return Err(ToolError::InvalidParam {
                message: format!("gateway `{}` appears more than once", upstream.name),
                param: "name".to_string(),
            });
        }
    }
    Ok(())
}

fn normalize_protected_mcp_route(route: &mut ProtectedMcpRouteConfig) -> Result<(), ToolError> {
    route.name = route.name.trim().to_string();
    route.public_host = normalize_public_host(&route.public_host)?;
    route.public_path = normalize_route_path(&route.public_path, "public_path")?;
    route.upstream = route
        .upstream
        .take()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());
    let legacy_backend_path = normalize_route_path(&route.backend_mcp_path, "backend_mcp_path")?;
    route.backend_url = if route.backend_url.trim().is_empty() {
        String::new()
    } else {
        normalize_backend_url(&route.backend_url, &legacy_backend_path)?
    };
    route.backend_mcp_path = default_backend_mcp_path();
    route.scopes = normalize_scopes(&route.scopes)?;
    if let Some(path) = route.health_path.take() {
        let trimmed = path.trim();
        route.health_path = if trimmed.is_empty() {
            None
        } else {
            Some(normalize_route_path(trimmed, "health_path")?)
        };
    }
    Ok(())
}

fn validate_protected_mcp_route(route: &ProtectedMcpRouteConfig) -> Result<(), ToolError> {
    if route.name.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "protected MCP route name must not be empty".to_string(),
            param: "name".to_string(),
        });
    }
    validate_safe_public_path(&route.public_path)?;
    match (route.upstream.as_deref(), route.backend_url.is_empty()) {
        (Some(_), true) => {}
        (None, false) => validate_backend_target(&route.backend_url)?,
        (Some(_), false) => {
            return Err(ToolError::InvalidParam {
                message: "protected MCP route must set either upstream or backend_url, not both"
                    .to_string(),
                param: "upstream".to_string(),
            });
        }
        (None, true) => {
            return Err(ToolError::InvalidParam {
                message: "protected MCP route must set upstream or backend_url".to_string(),
                param: "backend_url".to_string(),
            });
        }
    }
    Ok(())
}

fn normalize_public_host(raw: &str) -> Result<String, ToolError> {
    let trimmed = raw.trim().trim_end_matches('.');
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains(':')
        || trimmed.contains('@')
    {
        return Err(ToolError::InvalidParam {
            message: "public_host must be a bare host without scheme, port, or path".to_string(),
            param: "public_host".to_string(),
        });
    }
    Ok(trimmed.to_ascii_lowercase())
}

fn normalize_route_path(raw: &str, param: &str) -> Result<String, ToolError> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('/') {
        return Err(ToolError::InvalidParam {
            message: format!("{param} must start with /"),
            param: param.to_string(),
        });
    }
    if trimmed.len() > 1 && trimmed.ends_with('/') {
        return Ok(trimmed.trim_end_matches('/').to_string());
    }
    Ok(trimmed.to_string())
}

fn normalize_backend_url(raw: &str, default_path: &str) -> Result<String, ToolError> {
    let parsed = url::Url::parse(raw.trim()).map_err(|e| ToolError::InvalidParam {
        message: format!("invalid backend_url: {e}"),
        param: "backend_url".to_string(),
    })?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(ToolError::InvalidParam {
                message: "backend_url must use http:// or https://".to_string(),
                param: "backend_url".to_string(),
            });
        }
    }
    if parsed.host_str().is_none() || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ToolError::InvalidParam {
            message: "backend_url must not include query or fragment".to_string(),
            param: "backend_url".to_string(),
        });
    }
    let path = if parsed.path() == "/" {
        default_path.to_string()
    } else {
        normalize_route_path(parsed.path(), "backend_url")?
    };
    let mut backend = format!(
        "{}://{}",
        parsed.scheme(),
        parsed.host_str().unwrap_or_default().to_ascii_lowercase()
    );
    if let Some(port) = parsed.port() {
        backend.push(':');
        backend.push_str(&port.to_string());
    }
    backend.push_str(&path);
    Ok(backend)
}

fn default_backend_mcp_path() -> String {
    "/mcp".to_string()
}

fn normalize_scopes(raw: &[String]) -> Result<Vec<String>, ToolError> {
    let scopes = if raw.is_empty() {
        vec!["mcp:read".to_string(), "mcp:write".to_string()]
    } else {
        raw.iter()
            .map(|scope| scope.trim().to_string())
            .collect::<Vec<_>>()
    };
    if scopes.iter().any(|scope| scope.is_empty()) {
        return Err(ToolError::InvalidParam {
            message: "route scopes must not contain empty values".to_string(),
            param: "scopes".to_string(),
        });
    }
    Ok(scopes)
}

fn validate_safe_public_path(path: &str) -> Result<(), ToolError> {
    if path == "/" {
        return Err(ToolError::InvalidParam {
            message: "public_path must include a service segment".to_string(),
            param: "public_path".to_string(),
        });
    }
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("/.well-known") || lower.starts_with("/v1") {
        return Err(ToolError::InvalidParam {
            message: "public_path conflicts with Lab reserved routes".to_string(),
            param: "public_path".to_string(),
        });
    }
    if lower.contains("%2f")
        || lower.contains("%5c")
        || lower.contains("%2e")
        || path.contains('\\')
        || path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        || path.contains("//")
    {
        return Err(ToolError::InvalidParam {
            message: "public_path contains unsafe or ambiguous path segments".to_string(),
            param: "public_path".to_string(),
        });
    }
    Ok(())
}

fn validate_backend_target(origin: &str) -> Result<(), ToolError> {
    let parsed = url::Url::parse(origin).map_err(|e| ToolError::InvalidParam {
        message: format!("invalid backend_url: {e}"),
        param: "backend_url".to_string(),
    })?;
    let host = parsed.host_str().unwrap_or_default();
    if host.eq_ignore_ascii_case("localhost") {
        return Err(ToolError::InvalidParam {
            message: "backend_url must not target localhost".to_string(),
            param: "backend_url".to_string(),
        });
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        let blocked = match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                    || v4.octets() == [169, 254, 169, 254]
                    || (v4.octets()[0] == 169 && v4.octets()[1] == 254)
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || ((v6.segments()[0] & 0xffc0) == 0xfe80)
            }
        };
        if blocked {
            return Err(ToolError::InvalidParam {
                message: "backend_url targets an unsafe local/link-local address".to_string(),
                param: "backend_url".to_string(),
            });
        }
    }
    Ok(())
}

fn validate_upstream(upstream: &UpstreamConfig) -> Result<(), ToolError> {
    // Validate bearer_token_env if present — reject raw token values.
    if let Some(env_name) = &upstream.bearer_token_env {
        validate_bearer_token_env_name(env_name)?;
    }

    // Reject invalid names, mutually-exclusive auth shapes, and invalid URLs.
    // Name validation lives in UpstreamConfig::validate() so it runs on the
    // TOML load path as well (lab-qxl8.2 / lab-wsed).
    upstream.validate().map_err(|e| match e {
        crate::config::ConfigError::InvalidName { .. } => ToolError::InvalidParam {
            message: e.to_string(),
            param: "name".to_string(),
        },
        crate::config::ConfigError::ConflictingAuth { .. } => ToolError::InvalidParam {
            message: e.to_string(),
            param: "bearer_token_env".to_string(),
        },
        crate::config::ConfigError::MissingOauthUrl { .. }
        | crate::config::ConfigError::InvalidUrl { .. } => ToolError::InvalidParam {
            message: e.to_string(),
            param: "url".to_string(),
        },
        other => ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: other.to_string(),
        },
    })?;

    match (&upstream.url, &upstream.command) {
        (Some(_), Some(_)) => Err(ToolError::InvalidParam {
            message: "gateway must not set both `url` and `command`".to_string(),
            param: "url".to_string(),
        }),
        (None, None) => Err(ToolError::InvalidParam {
            message: "gateway must set either `url` or `command`".to_string(),
            param: "url".to_string(),
        }),
        (Some(url), None) => validate_gateway_url(url),
        (None, Some(command)) => {
            if command.trim().is_empty() {
                Err(ToolError::InvalidParam {
                    message: "gateway command must not be empty".to_string(),
                    param: "command".to_string(),
                })
            } else {
                Ok(())
            }
        }
    }
}

pub(crate) fn validate_bearer_token_env_name(value: &str) -> Result<(), ToolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "bearer token env var must not be empty".to_string(),
            param: "bearer_token_env".to_string(),
        });
    }

    if looks_like_raw_bearer_token(trimmed) {
        return Err(ToolError::InvalidParam {
            message: "bearer_token_env must be an environment variable name, not the token value"
                .to_string(),
            param: "bearer_token_env".to_string(),
        });
    }

    if !is_valid_env_var_name(trimmed) {
        return Err(ToolError::InvalidParam {
            message: "bearer_token_env must be a valid environment variable name".to_string(),
            param: "bearer_token_env".to_string(),
        });
    }

    Ok(())
}

/// Derive a default bearer-token env var name from a gateway name.
///
/// Matches the TS `defaultGatewayBearerEnvName` helper in
/// `apps/gateway-admin/lib/gateway-env.ts`: always prefixes with `LAB_GW_`
/// so generated names are scoped and cannot collide with arbitrary system vars.
pub(crate) fn default_gateway_bearer_env_name(name: &str) -> String {
    let normalized = name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    let inner = if normalized.is_empty() {
        "GATEWAY".to_string()
    } else {
        normalized
    };
    format!("LAB_GW_{inner}_AUTH_HEADER")
}

fn looks_like_raw_bearer_token(value: &str) -> bool {
    // Common token prefixes: reject values that are clearly raw secrets rather
    // than env var names. is_valid_env_var_name catches most others (spaces,
    // hyphens, colons), but JWTs (eyJ...) and some API keys can pass that check.
    value.starts_with("Bearer ")
        || value.starts_with("Token ")
        || value.starts_with("Basic ")
        || value.starts_with("ghp_")
        || value.starts_with("github_pat_")
        || value.starts_with("ghu_")
        || value.starts_with("ghs_")
        || value.starts_with("ghr_")
        || value.starts_with("eyJ") // JWT header (base64url of {"alg":...})
        || value.starts_with("sk-") // OpenAI and similar
        || value.starts_with("xoxb-") // Slack bot token
        || value.starts_with("xoxp-") // Slack user token
        || value.starts_with("glpat-") // GitLab PAT
        || value.starts_with("AKIA") // AWS Access Key ID
}

fn is_valid_env_var_name(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(ch) if ch == '_' || ch.is_ascii_alphabetic() => {}
        _ => return false,
    }

    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn validate_gateway_url(url: &str) -> Result<(), ToolError> {
    if !url.starts_with("https://") {
        return Err(ToolError::InvalidParam {
            message: format!("gateway URL must use https:// scheme, got `{url}`"),
            param: "url".to_string(),
        });
    }

    let parsed = url::Url::parse(url).map_err(|e| ToolError::InvalidParam {
        message: format!("invalid gateway URL `{url}`: {e}"),
        param: "url".to_string(),
    })?;

    if let Some(host) = parsed.host_str() {
        // Check literal IP addresses for private/loopback ranges.
        // For hostnames we do NOT perform DNS resolution — blocking DNS is
        // forbidden in async dispatch contexts.
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
            check_ip_not_private_tool(ip, url)?;
        }
    }

    Ok(())
}

fn check_ip_not_private_tool(ip: std::net::IpAddr, url: &str) -> Result<(), ToolError> {
    let blocked = match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()                              // 127.0.0.0/8
                || o[0] == 10                             // 10.0.0.0/8
                || (o[0] == 172 && o[1] >= 16 && o[1] <= 31) // 172.16.0.0/12
                || (o[0] == 192 && o[1] == 168)          // 192.168.0.0/16
                || (o[0] == 169 && o[1] == 254) // 169.254.0.0/16 link-local
        }
        std::net::IpAddr::V6(v6) => {
            let s = v6.segments();
            let is_ipv4_mapped =
                s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0xffff;
            if is_ipv4_mapped {
                // Check the embedded IPv4 address for private ranges.
                let v4 = std::net::Ipv4Addr::new(
                    (s[6] >> 8) as u8,
                    s[6] as u8,
                    (s[7] >> 8) as u8,
                    s[7] as u8,
                );
                let o = v4.octets();
                v4.is_loopback()
                    || o[0] == 10
                    || (o[0] == 172 && o[1] >= 16 && o[1] <= 31)
                    || (o[0] == 192 && o[1] == 168)
                    || (o[0] == 169 && o[1] == 254)
            } else {
                v6.is_loopback()                           // ::1/128
                    || (s[0] & 0xfe00) == 0xfc00           // fc00::/7 ULA
                    || (s[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
            }
        }
    };
    if blocked {
        return Err(ToolError::InvalidParam {
            message: format!(
                "gateway URL resolves to a private/loopback address — blocked to prevent SSRF: {url}"
            ),
            param: "url".to_string(),
        });
    }
    Ok(())
}

fn lock_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{name}.lock"))
        .unwrap_or_else(|| "config.toml.lock".to_string());
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(file_name)
}

#[cfg(test)]
mod tests {
    use crate::config::{LabConfig, ProtectedMcpRouteConfig, UpstreamConfig};

    use super::*;

    fn sample_config() -> LabConfig {
        LabConfig {
            upstream: vec![
                UpstreamConfig {
                    enabled: true,
                    name: "a".to_string(),
                    url: Some("http://127.0.0.1:9001".to_string()),
                    bearer_token_env: None,
                    command: None,
                    args: Vec::new(),
                    env: std::collections::BTreeMap::new(),
                    proxy_resources: false,
                    proxy_prompts: false,
                    expose_tools: None,
                    expose_resources: None,
                    expose_prompts: None,
                    oauth: None,
                    imported_from: None,
                    tool_search: crate::config::ToolSearchConfig::default(),
                },
                UpstreamConfig {
                    enabled: true,
                    name: "b".to_string(),
                    url: None,
                    bearer_token_env: None,
                    command: Some("node".to_string()),
                    args: vec!["server.js".to_string()],
                    env: std::collections::BTreeMap::new(),
                    proxy_resources: false,
                    proxy_prompts: false,
                    expose_tools: None,
                    expose_resources: None,
                    expose_prompts: None,
                    oauth: None,
                    imported_from: None,
                    tool_search: crate::config::ToolSearchConfig::default(),
                },
            ],
            ..LabConfig::default()
        }
    }

    fn sample_protected_route(name: &str) -> ProtectedMcpRouteConfig {
        ProtectedMcpRouteConfig {
            name: name.to_string(),
            enabled: true,
            public_host: "MCP.Tootie.TV".to_string(),
            public_path: "/syslog/".to_string(),
            upstream: None,
            backend_url: "http://100.88.16.79:3100".to_string(),
            backend_mcp_path: "/mcp".to_string(),
            scopes: vec![],
            health_path: None,
        }
    }

    #[test]
    fn load_gateway_config_reads_existing_upstreams() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[[upstream]]
name = "a"
url = "https://example.com/mcp"

[[upstream]]
name = "b"
command = "node"
args = ["server.js"]
"#,
        )
        .expect("write config");

        let cfg = load_gateway_config(&path).expect("load");
        assert_eq!(cfg.upstream.len(), 2);
        assert_eq!(cfg.upstream[0].name, "a");
        assert_eq!(cfg.upstream[1].name, "b");
        assert_eq!(cfg.upstream[1].command.as_deref(), Some("node"));
    }

    #[test]
    fn insert_upstream_adds_new_gateway_entry() {
        let mut cfg = sample_config();
        insert_upstream(
            &mut cfg,
            UpstreamConfig {
                enabled: true,
                name: "c".to_string(),
                url: Some("https://example.com/mcp".to_string()),
                bearer_token_env: Some("C_TOKEN".to_string()),
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: true,
                proxy_prompts: true,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            },
        )
        .expect("insert");

        assert_eq!(cfg.upstream.len(), 3);
        assert!(cfg.upstream.iter().any(|u| u.name == "c"));
    }

    #[test]
    fn update_upstream_replaces_named_upstream_only() {
        let mut cfg = sample_config();

        update_upstream(
            &mut cfg,
            "b",
            GatewayUpdatePatch {
                proxy_resources: Some(true),
                ..GatewayUpdatePatch::default()
            },
        )
        .expect("update should succeed");

        let a = cfg
            .upstream
            .iter()
            .find(|u| u.name == "a")
            .expect("a upstream");
        let b = cfg
            .upstream
            .iter()
            .find(|u| u.name == "b")
            .expect("b upstream");

        assert_eq!(a.url.as_deref(), Some("http://127.0.0.1:9001"));
        assert_eq!(b.command.as_deref(), Some("node"));
        assert!(b.proxy_resources);
    }

    #[test]
    fn update_upstream_applies_expose_tools_patch() {
        let mut cfg = sample_config();

        update_upstream(
            &mut cfg,
            "b",
            GatewayUpdatePatch {
                expose_tools: Some(Some(vec!["search_*".to_string(), "read_file".to_string()])),
                ..GatewayUpdatePatch::default()
            },
        )
        .expect("update should succeed");

        let b = cfg
            .upstream
            .iter()
            .find(|u| u.name == "b")
            .expect("b upstream");

        assert_eq!(
            b.expose_tools.as_deref(),
            Some(&["search_*".to_string(), "read_file".to_string()][..])
        );
    }

    #[test]
    fn expose_tools_patch_distinguishes_absent_null_empty_and_values() {
        let absent: GatewayUpdatePatch = serde_json::from_str(r"{}").unwrap();
        let null: GatewayUpdatePatch = serde_json::from_str(r#"{"expose_tools": null}"#).unwrap();
        let empty: GatewayUpdatePatch = serde_json::from_str(r#"{"expose_tools": []}"#).unwrap();
        let with_values: GatewayUpdatePatch =
            serde_json::from_str(r#"{"expose_tools": ["foo"]}"#).unwrap();

        // absent → None (skip in patch)
        assert!(absent.expose_tools.is_none());
        // null → Some(None) (clear the filter)
        assert_eq!(null.expose_tools, Some(None));
        // empty array → Some(Some([])) (will be normalized to clear)
        assert_eq!(empty.expose_tools, Some(Some(vec![])));
        // values → Some(Some([...]))
        assert_eq!(
            with_values.expose_tools,
            Some(Some(vec!["foo".to_string()]))
        );
    }

    #[test]
    fn update_upstream_clears_expose_tools_with_null() {
        let mut cfg = sample_config();

        // First set a filter
        update_upstream(
            &mut cfg,
            "b",
            GatewayUpdatePatch {
                expose_tools: Some(Some(vec!["read_*".to_string()])),
                ..GatewayUpdatePatch::default()
            },
        )
        .expect("set filter");
        assert!(cfg.upstream[1].expose_tools.is_some());

        // Clear with null (Some(None))
        update_upstream(
            &mut cfg,
            "b",
            GatewayUpdatePatch {
                expose_tools: Some(None),
                ..GatewayUpdatePatch::default()
            },
        )
        .expect("clear filter");
        assert!(
            cfg.upstream[1].expose_tools.is_none(),
            "expose_tools should be cleared"
        );
    }

    #[test]
    fn update_upstream_clears_expose_tools_with_empty_array() {
        let mut cfg = sample_config();

        // First set a filter
        update_upstream(
            &mut cfg,
            "b",
            GatewayUpdatePatch {
                expose_tools: Some(Some(vec!["read_*".to_string()])),
                ..GatewayUpdatePatch::default()
            },
        )
        .expect("set filter");
        assert!(cfg.upstream[1].expose_tools.is_some());

        // Clear with empty array (normalized to None)
        update_upstream(
            &mut cfg,
            "b",
            GatewayUpdatePatch {
                expose_tools: Some(Some(vec![])),
                ..GatewayUpdatePatch::default()
            },
        )
        .expect("clear filter");
        assert!(
            cfg.upstream[1].expose_tools.is_none(),
            "empty array should clear expose_tools"
        );
    }

    #[test]
    fn remove_upstream_removes_named_gateway_entry() {
        let mut cfg = sample_config();
        let removed = remove_upstream(&mut cfg, "b").expect("remove");

        assert_eq!(removed.name, "b");
        assert_eq!(cfg.upstream.len(), 1);
        assert_eq!(cfg.upstream[0].name, "a");
    }

    #[test]
    fn insert_protected_route_normalizes_defaults_and_lan_backend() {
        let mut cfg = LabConfig::default();
        let route = insert_protected_mcp_route(&mut cfg, sample_protected_route("syslog"))
            .expect("insert route");

        assert_eq!(route.public_host, "mcp.tootie.tv");
        assert_eq!(route.public_path, "/syslog");
        assert_eq!(route.backend_url, "http://100.88.16.79:3100/mcp");
        assert_eq!(route.scopes, ["mcp:read", "mcp:write"]);
        assert_eq!(route.public_resource(), "https://mcp.tootie.tv/syslog");
        assert_eq!(cfg.protected_mcp_routes.len(), 1);
    }

    #[test]
    fn insert_protected_route_accepts_named_upstream_without_backend_url() {
        let mut cfg = LabConfig::default();
        let mut route = sample_protected_route("axon");
        route.public_path = "/axon".to_string();
        route.upstream = Some(" axon ".to_string());
        route.backend_url = String::new();

        let route = insert_protected_mcp_route(&mut cfg, route).expect("insert route");

        assert_eq!(route.upstream.as_deref(), Some("axon"));
        assert_eq!(route.backend_url, "");
        assert_eq!(route.public_resource(), "https://mcp.tootie.tv/axon");
    }

    #[test]
    fn insert_protected_route_rejects_ambiguous_backend_and_upstream() {
        let mut cfg = LabConfig::default();
        let mut route = sample_protected_route("axon");
        route.upstream = Some("axon".to_string());

        let err = insert_protected_mcp_route(&mut cfg, route)
            .expect_err("route should not set backend_url and upstream");

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn insert_protected_route_rejects_duplicate_enabled_host_path() {
        let mut cfg = LabConfig::default();
        insert_protected_mcp_route(&mut cfg, sample_protected_route("syslog")).expect("first");
        let err = insert_protected_mcp_route(&mut cfg, sample_protected_route("other"))
            .expect_err("duplicate route should fail");

        assert_eq!(err.kind(), "conflict");
    }

    #[test]
    fn insert_protected_route_rejects_reserved_or_ambiguous_public_paths() {
        for path in ["/", "/v1/proxy", "/.well-known/x", "/syslog/%2e%2e"] {
            let mut cfg = LabConfig::default();
            let mut route = sample_protected_route("bad");
            route.public_path = path.to_string();
            let err =
                insert_protected_mcp_route(&mut cfg, route).expect_err("path should be rejected");
            assert_eq!(err.kind(), "invalid_param", "{path}");
        }
    }

    #[test]
    fn insert_protected_route_rejects_unsafe_backend_targets() {
        for backend in [
            "file:///tmp/server",
            "http://localhost:3100",
            "http://127.0.0.1:3100",
            "http://169.254.169.254",
            "http://100.88.16.79:3100/mcp?token=secret",
        ] {
            let mut cfg = LabConfig::default();
            let mut route = sample_protected_route("bad");
            route.backend_url = backend.to_string();
            let err = insert_protected_mcp_route(&mut cfg, route)
                .expect_err("backend should be rejected");
            assert_eq!(err.kind(), "invalid_param", "{backend}");
        }
    }

    #[test]
    fn insert_upstream_rejects_duplicate_names() {
        let mut cfg = sample_config();
        let err = insert_upstream(
            &mut cfg,
            UpstreamConfig {
                enabled: true,
                name: "a".to_string(),
                url: Some("https://example.com/mcp".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            },
        )
        .expect_err("duplicate should fail");

        assert_eq!(err.kind(), "conflict");
    }

    #[test]
    fn write_gateway_config_rejects_both_url_and_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let cfg = LabConfig {
            upstream: vec![UpstreamConfig {
                enabled: true,
                name: "bad".to_string(),
                url: Some("http://127.0.0.1:9001".to_string()),
                bearer_token_env: None,
                command: Some("node".to_string()),
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            }],
            ..LabConfig::default()
        };

        let err = write_gateway_config(&path, &cfg).expect_err("invalid transport selectors");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn write_gateway_config_rejects_missing_transport_selector() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let cfg = LabConfig {
            upstream: vec![UpstreamConfig {
                enabled: true,
                name: "bad".to_string(),
                url: None,
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            }],
            ..LabConfig::default()
        };

        let err = write_gateway_config(&path, &cfg).expect_err("missing transport selectors");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn insert_upstream_rejects_non_http_scheme() {
        let err = insert_upstream(
            &mut LabConfig::default(),
            UpstreamConfig {
                enabled: true,
                name: "ftp".to_string(),
                url: Some("ftp://example.com".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            },
        )
        .expect_err("invalid scheme");

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn insert_upstream_rejects_bind_all_address() {
        let err = insert_upstream(
            &mut LabConfig::default(),
            UpstreamConfig {
                enabled: true,
                name: "bind-all".to_string(),
                url: Some("http://0.0.0.0:8790".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            },
        )
        .expect_err("bind-all should be rejected");

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn insert_upstream_rejects_raw_bearer_token_values_in_bearer_token_env() {
        let err = insert_upstream(
            &mut LabConfig::default(),
            UpstreamConfig {
                enabled: true,
                name: "github".to_string(),
                url: Some("https://api.githubcopilot.com/mcp/".to_string()),
                bearer_token_env: Some("Bearer ghp_secret".to_string()),
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            },
        )
        .expect_err("raw bearer token should be rejected");

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn default_gateway_bearer_env_name_normalizes_gateway_names() {
        assert_eq!(
            default_gateway_bearer_env_name("github"),
            "LAB_GW_GITHUB_AUTH_HEADER"
        );
        assert_eq!(
            default_gateway_bearer_env_name("github-copilot remote"),
            "LAB_GW_GITHUB_COPILOT_REMOTE_AUTH_HEADER"
        );
    }

    #[test]
    fn validate_gateway_url_blocks_rfc1918() {
        assert!(validate_gateway_url("https://192.168.1.1/mcp").is_err());
        assert!(validate_gateway_url("https://10.0.0.1/mcp").is_err());
        assert!(validate_gateway_url("https://172.16.0.1/mcp").is_err());
        assert!(validate_gateway_url("https://172.31.255.255/mcp").is_err());
        assert!(validate_gateway_url("https://169.254.0.1/mcp").is_err());
    }

    #[test]
    fn validate_gateway_url_blocks_loopback() {
        assert!(validate_gateway_url("https://127.0.0.1/mcp").is_err());
        assert!(validate_gateway_url("https://[::1]/mcp").is_err());
    }

    #[test]
    fn validate_gateway_url_requires_https() {
        assert!(validate_gateway_url("http://example.com/mcp").is_err());
        assert!(validate_gateway_url("ftp://example.com/mcp").is_err());
        assert!(validate_gateway_url("ws://example.com/mcp").is_err());
    }

    #[test]
    fn validate_gateway_url_allows_public_https() {
        assert!(validate_gateway_url("https://example.com/mcp").is_ok());
        assert!(validate_gateway_url("https://api.github.com/mcp").is_ok());
    }

    #[test]
    fn validate_gateway_url_blocks_ipv6_ula_and_link_local() {
        // fc00::/7 ULA
        assert!(validate_gateway_url("https://[fd00::1]/mcp").is_err());
        // fe80::/10 link-local
        assert!(validate_gateway_url("https://[fe80::1]/mcp").is_err());
    }

    #[test]
    fn validate_gateway_url_blocks_ipv4_mapped_private() {
        // ::ffff:192.168.1.1 — IPv4-mapped IPv6 with private address
        assert!(validate_gateway_url("https://[::ffff:192.168.1.1]/mcp").is_err());
        // ::ffff:127.0.0.1 — IPv4-mapped loopback
        assert!(validate_gateway_url("https://[::ffff:127.0.0.1]/mcp").is_err());
    }
}
