use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use anyhow::Context;
use fd_lock::RwLock;
use tempfile::NamedTempFile;

use crate::upstream::spawn_guard;
use lab_runtime::error::ToolError;
use lab_runtime::gateway_config::{
    GatewayConfig, GatewayPreferences, ProtectedMcpRouteConfig, ProtectedMcpRouteTarget,
    UpstreamConfig, UpstreamImportTombstone,
};

use super::params::GatewayUpdatePatch;

pub fn load_gateway_config(path: &Path) -> Result<GatewayConfig, ToolError> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let mut cfg = toml::from_str::<GatewayConfig>(&raw).map_err(|e| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to parse {}: {e}", path.display()),
            })?;
            cfg.normalize_protected_mcp_routes()
                .map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: format!("invalid config {}: {e}", path.display()),
                })?;
            normalize_config(&mut cfg)?;
            validate_config(&cfg)?;
            Ok(cfg)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GatewayConfig::default()),
        Err(e) => Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to read {}: {e}", path.display()),
        }),
    }
}

// Gateway-owned top-level keys. This is the lab-gateway FS-store render path
// used by tests and any caller that persists a bare `GatewayConfig`. It strips
// and rewrites ONLY the gateway-owned sections, preserving every other key
// (including all non-gateway `LabConfig` sections) byte-for-byte.
//
// The PRODUCTION host store (`lab`'s `config_store`) keeps the full `LabConfig`
// key list and the verbatim `render_gateway_config` toml_edit logic, so the
// host preserves the exact foreign-key + non-gateway-section behavior.
const KNOWN_LAB_CONFIG_KEYS: &[&str] = &[
    "code_mode",
    "upstream_request_timeout_ms",
    "upstream_relay_timeout_ms",
    "upstream",
    "upstream_import_tombstones",
    "upstream_pending",
    "protected_mcp_routes",
    "virtual_servers",
    "quarantined_virtual_servers",
    "gateway",
];

/// Compile-time assertion that KNOWN_LAB_CONFIG_KEYS contains no duplicate entries.
///
/// This check runs at compile time via a const evaluation. If you add a key,
/// ensure it does not already appear in the list — the duplicate "code_mode"
/// entry (A-L9) was the original motivation for this guard.
const _: () = {
    let keys = KNOWN_LAB_CONFIG_KEYS;
    let mut i = 0;
    while i < keys.len() {
        let mut j = i + 1;
        while j < keys.len() {
            // const-safe byte-by-byte string comparison
            let a = keys[i].as_bytes();
            let b = keys[j].as_bytes();
            if a.len() == b.len() {
                let mut k = 0;
                let mut equal = true;
                while k < a.len() {
                    if a[k] != b[k] {
                        equal = false;
                        break;
                    }
                    k += 1;
                }
                assert!(!equal, "KNOWN_LAB_CONFIG_KEYS contains a duplicate entry");
            }
            j += 1;
        }
        i += 1;
    }
};

/// Serialize `cfg` to TOML and atomically replace the file at `path`.
pub fn write_gateway_config(path: &Path, cfg: &GatewayConfig) -> Result<(), ToolError> {
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
    let raw = render_gateway_config(path, cfg)?;

    let mut tmp = NamedTempFile::new_in(parent).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to create temp file in {}: {e}", parent.display()),
    })?;

    // O-M4: restrict temp file to 0o600 before writing; config.toml stores
    // upstream env refs and bearer token env names — treat it as a secret file.
    set_file_permissions_600(tmp.path()).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to restrict temp config permissions: {e}"),
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

    // Ensure the persisted file is 0o600 (rename preserves the temp file's
    // mode on Linux, but an explicit chmod guards against edge cases such as
    // cross-device rename or an existing file being replaced with different mode).
    set_file_permissions_600(path).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to restrict config permissions: {e}"),
    })?;

    Ok(())
}

fn render_gateway_config(path: &Path, cfg: &GatewayConfig) -> Result<String, ToolError> {
    let serialized = toml::to_string(cfg).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to serialize gateway config: {e}"),
    })?;
    let desired = serialized
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to parse serialized gateway config: {e}"),
        })?;

    let Ok(existing_raw) = std::fs::read_to_string(path) else {
        return Ok(serialized);
    };
    let Ok(mut existing) = existing_raw.parse::<toml_edit::DocumentMut>() else {
        return Ok(serialized);
    };

    for key in KNOWN_LAB_CONFIG_KEYS {
        existing.as_table_mut().remove(key);
    }
    for (key, item) in desired.as_table() {
        existing[key] = item.clone();
    }

    Ok(existing.to_string())
}

pub fn insert_upstream(cfg: &mut GatewayConfig, upstream: UpstreamConfig) -> Result<(), ToolError> {
    validate_upstream(&upstream, &cfg.gateway)?;
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
    cfg.upstream_import_tombstones
        .retain(|tombstone| !tombstone_matches_upstream(tombstone, &upstream));
    cfg.upstream.push(upstream);
    Ok(())
}

pub(crate) fn update_upstream(
    cfg: &mut GatewayConfig,
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
    if patch.code_mode.is_some() {
        return Err(ToolError::InvalidParam {
            message: "code_mode config is gateway-wide; use gateway.code_mode.set instead of gateway.update"
                .to_string(),
            param: "code_mode".to_string(),
        });
    }

    validate_upstream(&cfg.upstream[index], &cfg.gateway)?;
    Ok(())
}

pub fn remove_upstream(cfg: &mut GatewayConfig, name: &str) -> Result<UpstreamConfig, ToolError> {
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

pub fn tombstone_removed_import(cfg: &mut GatewayConfig, removed: &UpstreamConfig) {
    let Some(imported_from) = removed.imported_from.clone() else {
        return;
    };
    cfg.upstream_import_tombstones
        .retain(|tombstone| !tombstone_matches_upstream(tombstone, removed));
    cfg.upstream_import_tombstones
        .push(UpstreamImportTombstone::now(&removed.name, imported_from));
}

fn tombstone_matches_upstream(
    tombstone: &UpstreamImportTombstone,
    upstream: &UpstreamConfig,
) -> bool {
    if tombstone.name == upstream.name {
        let Some(source) = upstream.imported_from.as_ref() else {
            return true;
        };
        return tombstone_source_matches_upstream(tombstone, source)
            && tombstone
                .imported_from
                .server_name
                .as_deref()
                .is_none_or(|server_name| source.server_name.as_deref() == Some(server_name))
            && tombstone_transport_matches_upstream(tombstone, upstream);
    }

    let Some(source) = upstream.imported_from.as_ref() else {
        return false;
    };
    tombstone_source_matches_upstream(tombstone, source)
        && tombstone.imported_from.server_name.is_some()
        && tombstone.imported_from.server_name == source.server_name
        && tombstone_transport_matches_upstream(tombstone, upstream)
}

fn tombstone_source_matches_upstream(
    tombstone: &UpstreamImportTombstone,
    source: &lab_runtime::gateway_config::ImportSource,
) -> bool {
    tombstone.imported_from.client == source.client && tombstone.imported_from.path == source.path
}

fn tombstone_transport_matches_upstream(
    tombstone: &UpstreamImportTombstone,
    upstream: &UpstreamConfig,
) -> bool {
    let Some(tombstone_fingerprint) = tombstone.imported_from.transport_fingerprint.as_deref()
    else {
        return true;
    };
    upstream
        .imported_from
        .as_ref()
        .and_then(|source| source.transport_fingerprint.as_deref())
        .is_none_or(|fingerprint| fingerprint == tombstone_fingerprint)
}

pub fn insert_protected_mcp_route(
    cfg: &mut GatewayConfig,
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
    if route.enabled
        && route.is_gateway_subset()
        && cfg.protected_mcp_routes.iter().any(|existing| {
            existing.enabled
                && existing.is_gateway_subset()
                && existing.public_path == route.public_path
        })
    {
        return Err(ToolError::Conflict {
            message: format!(
                "gateway_subset protected MCP route for {} already exists",
                route.public_path
            ),
            existing_id: route.name.clone(),
        });
    }
    cfg.protected_mcp_routes.push(route.clone());
    Ok(route)
}

pub fn update_protected_mcp_route(
    cfg: &mut GatewayConfig,
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
    if route.enabled
        && route.is_gateway_subset()
        && cfg
            .protected_mcp_routes
            .iter()
            .enumerate()
            .any(|(existing_index, existing)| {
                existing_index != index
                    && existing.enabled
                    && existing.is_gateway_subset()
                    && existing.public_path == route.public_path
            })
    {
        return Err(ToolError::Conflict {
            message: format!(
                "gateway_subset protected MCP route for {} already exists",
                route.public_path
            ),
            existing_id: route.name.clone(),
        });
    }
    cfg.protected_mcp_routes[index] = route.clone();
    Ok(route)
}

pub fn remove_protected_mcp_route(
    cfg: &mut GatewayConfig,
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
    let mut enabled_gateway_subset_paths = std::collections::HashSet::new();
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
            if route.is_gateway_subset()
                && !enabled_gateway_subset_paths.insert(route.public_path.clone())
            {
                return Err(ToolError::InvalidParam {
                    message: format!(
                        "duplicate enabled gateway_subset protected MCP route for {}",
                        route.public_path
                    ),
                    param: "public_path".to_string(),
                });
            }
        }
    }
    Ok(())
}

fn validate_config(cfg: &GatewayConfig) -> Result<(), ToolError> {
    validate_code_mode(&cfg.code_mode)?;
    validate_upstreams(&cfg.upstream, &cfg.gateway)?;
    validate_protected_mcp_routes(&cfg.protected_mcp_routes)
}

fn normalize_config(cfg: &mut GatewayConfig) -> Result<(), ToolError> {
    for route in &mut cfg.protected_mcp_routes {
        normalize_protected_mcp_route(route)?;
    }
    Ok(())
}

pub fn validate_code_mode(
    code_mode: &lab_runtime::gateway_config::CodeModeConfig,
) -> Result<(), ToolError> {
    code_mode.validate().map_err(|e| match e {
        lab_runtime::gateway_config::ConfigError::InvalidCodeModeTimeout { .. } => {
            ToolError::InvalidParam {
                message: e.to_string(),
                param: "code_mode.timeout_ms".to_string(),
            }
        }
        lab_runtime::gateway_config::ConfigError::InvalidCodeModeMaxResponseBytes { .. } => {
            ToolError::InvalidParam {
                message: e.to_string(),
                param: "code_mode.max_response_bytes".to_string(),
            }
        }
        lab_runtime::gateway_config::ConfigError::InvalidCodeModeMaxResponseTokens { .. } => {
            ToolError::InvalidParam {
                message: e.to_string(),
                param: "code_mode.max_response_tokens".to_string(),
            }
        }
        _ => ToolError::InvalidParam {
            message: e.to_string(),
            param: "code_mode".to_string(),
        },
    })
}

fn validate_upstreams(
    upstreams: &[UpstreamConfig],
    prefs: &GatewayPreferences,
) -> Result<(), ToolError> {
    let mut names = std::collections::HashSet::new();
    for upstream in upstreams {
        validate_upstream(upstream, prefs)?;
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
    if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &mut route.target {
        target.upstreams =
            normalize_name_list(std::mem::take(&mut target.upstreams), "target.upstreams")?;
        target.services =
            normalize_name_list(std::mem::take(&mut target.services), "target.services")?;
    }
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
    if route.target.is_some() && (route.upstream.is_some() || !route.backend_url.is_empty()) {
        return Err(ToolError::InvalidParam {
            message: "protected MCP route target cannot be combined with upstream or backend_url"
                .to_string(),
            param: "target".to_string(),
        });
    }

    if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &route.target {
        if target.upstreams.is_empty() && target.services.is_empty() && !target.expose_code_mode {
            return Err(ToolError::InvalidParam {
                message:
                    "gateway_subset target must expose at least one upstream, service, or Code Mode"
                        .to_string(),
                param: "target".to_string(),
            });
        }
        return Ok(());
    }

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

fn normalize_name_list(values: Vec<String>, param: &str) -> Result<Vec<String>, ToolError> {
    let mut normalized = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ToolError::InvalidParam {
                message: format!("{param} entries must not be empty"),
                param: param.to_string(),
            });
        }
        let name = trimmed.to_string();
        if !normalized.contains(&name) {
            normalized.push(name);
        }
    }
    Ok(normalized)
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

fn validate_upstream(
    upstream: &UpstreamConfig,
    prefs: &GatewayPreferences,
) -> Result<(), ToolError> {
    // Validate bearer_token_env if present — reject raw token values.
    if let Some(env_name) = &upstream.bearer_token_env {
        validate_bearer_token_env_name(env_name)?;
    }

    // Reject invalid names, mutually-exclusive auth shapes, and invalid URLs.
    // Name validation lives in UpstreamConfig::validate() so it runs on the
    // TOML load path as well (lab-qxl8.2 / lab-wsed).
    upstream.validate().map_err(|e| match e {
        lab_runtime::gateway_config::ConfigError::InvalidName { .. } => ToolError::InvalidParam {
            message: e.to_string(),
            param: "name".to_string(),
        },
        lab_runtime::gateway_config::ConfigError::ConflictingAuth { .. } => {
            ToolError::InvalidParam {
                message: e.to_string(),
                param: "bearer_token_env".to_string(),
            }
        }
        lab_runtime::gateway_config::ConfigError::MissingOauthUrl { .. }
        | lab_runtime::gateway_config::ConfigError::InvalidUrl { .. } => ToolError::InvalidParam {
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
            validate_stdio_upstream(command, &upstream.args, &upstream.env, prefs)
        }
    }
}

/// S1/S6: validate a stdio upstream's command, argv, and env against the
/// shared spawn-guard allowlists. This is the **single chokepoint** — every
/// path that writes a stdio upstream spec (add, batch_add, update, import)
/// calls `validate_upstream`, which calls this function.
///
/// Only known runtimes (npx/uvx/docker/node/…) may be persisted as the
/// `command` of a stdio upstream. Dangerous argv flags and protected env names
/// are also rejected here so no write path bypasses the checks.
fn validate_stdio_upstream(
    command: &str,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
    prefs: &GatewayPreferences,
) -> Result<(), ToolError> {
    if command.trim().is_empty() {
        return Err(ToolError::InvalidParam {
            message: "gateway command must not be empty".to_string(),
            param: "command".to_string(),
        });
    }
    spawn_guard::validate_stdio_spec(
        command,
        args,
        env,
        &prefs.extra_stdio_commands,
        prefs.disable_spawn_guard,
    )
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
    // Accept both http:// and https:// — OAuth and other auth layers provide
    // security for servers that need it; homelab services routinely use http://.
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ToolError::InvalidParam {
            message: format!("gateway URL must use http:// or https:// scheme, got `{url}`"),
            param: "url".to_string(),
        });
    }

    let parsed = url::Url::parse(url).map_err(|e| ToolError::InvalidParam {
        message: format!("invalid gateway URL `{url}`: {e}"),
        param: "url".to_string(),
    })?;

    // Reject the bind-all / unspecified address — it is a listen address,
    // not a valid connection target.
    if let Some(host) = parsed.host_str() {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
            let is_unspecified = match ip {
                std::net::IpAddr::V4(v4) => v4.octets() == [0, 0, 0, 0],
                std::net::IpAddr::V6(v6) => v6 == std::net::Ipv6Addr::UNSPECIFIED,
            };
            if is_unspecified {
                return Err(ToolError::InvalidParam {
                    message: format!(
                        "gateway URL must not use the bind-all/unspecified address: {url}"
                    ),
                    param: "url".to_string(),
                });
            }
        }
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

/// Set a file's permissions to owner-read/write only (0o600).
/// No-op on non-Unix targets (homelab is Linux-only).
pub(crate) fn set_file_permissions_600(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic)]
#[path = "config_tests.rs"]
mod tests;
