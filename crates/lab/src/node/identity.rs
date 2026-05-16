use std::fs;
use std::net::IpAddr;

use anyhow::Result;

use crate::config::{LabConfig, NodeRole, NodeRuntimeRole, ResolvedNodeRuntime};

const HOST_HOSTNAME_PATH: &str = "/run/host/hostname";

pub fn resolve_local_hostname() -> Result<String> {
    if let Some(value) = std::env::var("LAB_HOST_HOSTNAME")
        .ok()
        .and_then(|value| normalize_host_identifier(&value))
    {
        return Ok(value);
    }

    match fs::read_to_string(HOST_HOSTNAME_PATH) {
        Ok(value) => {
            if let Some(normalized) = normalize_host_identifier(&value) {
                return Ok(normalized);
            }
            tracing::debug!(
                surface = "node",
                service = "identity",
                action = "hostname.resolve",
                event = "identity.cache_miss",
                source = HOST_HOSTNAME_PATH,
                "host hostname file was empty after normalization",
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                surface = "node",
                service = "identity",
                action = "hostname.resolve",
                event = "identity.cache_miss",
                source = HOST_HOSTNAME_PATH,
                "host hostname file was not mounted",
            );
        }
        Err(error) => {
            tracing::warn!(
                surface = "node",
                service = "identity",
                action = "hostname.resolve",
                event = "identity.fetch_failure",
                kind = "identity_fetch_failed",
                source = HOST_HOSTNAME_PATH,
                error = %error,
                "host hostname fetch failed",
            );
        }
    }

    if let Some(value) = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .and_then(|value| normalize_host_identifier(&value))
    {
        return Ok(value);
    }

    tracing::debug!(
        surface = "node",
        service = "identity",
        action = "hostname.resolve",
        event = "identity.cache_miss",
        source = "environment",
        "hostname environment lookup missed",
    );

    for path in ["/etc/hostname", "/etc/HOSTNAME"] {
        match fs::read_to_string(path) {
            Ok(value) => {
                if let Some(normalized) = normalize_host_identifier(&value) {
                    return Ok(normalized);
                }
                tracing::debug!(
                    surface = "node",
                    service = "identity",
                    action = "hostname.resolve",
                    event = "identity.cache_miss",
                    source = %path,
                    "hostname file was empty after normalization",
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    surface = "node",
                    service = "identity",
                    action = "hostname.resolve",
                    event = "identity.cache_miss",
                    source = %path,
                    "hostname file was not present",
                );
            }
            Err(error) => {
                tracing::warn!(
                    surface = "node",
                    service = "identity",
                    action = "hostname.resolve",
                    event = "identity.fetch_failure",
                    kind = "identity_fetch_failed",
                    source = %path,
                    error = %error,
                    "hostname fetch failed",
                );
            }
        }
    }

    tracing::debug!(
        surface = "node",
        service = "identity",
        action = "hostname.resolve",
        event = "identity.cache_miss",
        source = "fallback",
        "using fallback hostname identity",
    );
    Ok("localhost".to_string())
}

pub fn resolve_runtime_role(
    local_host: &str,
    configured_master: Option<&str>,
) -> Result<ResolvedNodeRuntime> {
    let started = std::time::Instant::now();
    let local_host =
        normalize_host_identifier(local_host).unwrap_or_else(|| "localhost".to_string());
    let master_host = configured_master
        .and_then(normalize_host_identifier)
        .unwrap_or_else(|| local_host.clone());
    let role = if hosts_refer_to_same_device(&local_host, &master_host) {
        NodeRole::Master
    } else {
        NodeRole::NonMaster
    };

    tracing::info!(
        surface = "node", service = "identity", action = "role.resolved",
        local_host = %local_host,
        master_host = %master_host,
        role = ?role,
        is_master = matches!(role, NodeRole::Master),
        elapsed_ms = started.elapsed().as_millis(),
        "runtime role resolved",
    );
    Ok(ResolvedNodeRuntime {
        local_host,
        master_host,
        role,
    })
}

/// Resolve the node runtime role using the unified resolution order:
///
/// 1. CLI `--role` override (`override_role`)
/// 2. `[node].role` in config.toml
/// 3. Hostname comparison against `config.controller_host()` (legacy path)
///
/// If the resolved role is `Node`, a controller host **must** be configured;
/// the function returns an error before startup if one is absent.
///
/// Maps `NodeRuntimeRole::Controller → NodeRole::Master`
/// and `NodeRuntimeRole::Node → NodeRole::NonMaster`.
pub fn resolve_runtime_role_from_config(
    local_host: &str,
    config: &LabConfig,
    override_role: Option<NodeRuntimeRole>,
) -> Result<ResolvedNodeRuntime> {
    // Resolution order: CLI override → config [node].role → hostname inference.
    let explicit_role = override_role.or_else(|| config.node.as_ref().and_then(|n| n.role));

    match explicit_role {
        Some(role_hint @ NodeRuntimeRole::Node) => {
            // A node role requires a controller host to be known.
            let source = if override_role.is_some() {
                "cli_override"
            } else {
                "config_role"
            };
            tracing::info!(
                surface = "node",
                service = "identity",
                action = "role.resolved",
                source = source,
                role = ?role_hint,
                "runtime role resolution source",
            );
            let controller = config.controller_host().ok_or_else(|| {
                if override_role.is_some() {
                    anyhow::anyhow!(
                        "--role node requires a controller host; set [node].controller in config.toml"
                    )
                } else {
                    anyhow::anyhow!(
                        "[node].role = \"node\" requires a controller host; set [node].controller in config.toml"
                    )
                }
            })?;
            resolve_runtime_role(local_host, Some(controller))
        }
        Some(role_hint @ NodeRuntimeRole::Controller) => {
            // Explicit controller role: use local host as the master host.
            let source = if override_role.is_some() {
                "cli_override"
            } else {
                "config_role"
            };
            tracing::info!(
                surface = "node",
                service = "identity",
                action = "role.resolved",
                source = source,
                role = ?role_hint,
                "runtime role resolution source",
            );
            let normalized =
                normalize_host_identifier(local_host).unwrap_or_else(|| "localhost".to_string());
            resolve_runtime_role(local_host, Some(&normalized))
        }
        None => {
            // Legacy hostname-comparison path.
            tracing::info!(
                surface = "node",
                service = "identity",
                action = "role.resolved",
                source = "hostname_inference",
                role = ?Option::<NodeRuntimeRole>::None,
                "runtime role resolution source",
            );
            resolve_runtime_role(local_host, config.controller_host())
        }
    }
}

fn normalize_host_identifier(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

fn short_host_identifier(value: &str) -> &str {
    value.split('.').next().unwrap_or(value)
}

fn hosts_refer_to_same_device(local_host: &str, master_host: &str) -> bool {
    if local_host == master_host {
        return true;
    }

    match (local_host.parse::<IpAddr>(), master_host.parse::<IpAddr>()) {
        (Ok(local_ip), Ok(master_ip)) => return local_ip == master_ip,
        (Ok(_), Err(_)) | (Err(_), Ok(_)) => return false,
        (Err(_), Err(_)) => {}
    }

    let local_short = short_host_identifier(local_host);
    let master_short = short_host_identifier(master_host);
    local_short == master_short && (local_host == local_short || master_host == master_short)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hostname_resolution_logs_misses_and_fetch_failures() {
        let source = include_str!("identity.rs");
        for field in [
            "LAB_HOST_HOSTNAME",
            "HOST_HOSTNAME_PATH",
            "event = \"identity.cache_miss\"",
            "event = \"identity.fetch_failure\"",
            "kind = \"identity_fetch_failed\"",
            "action = \"hostname.resolve\"",
            "source = %path",
        ] {
            assert!(
                source.contains(field),
                "missing identity log field: {field}"
            );
        }
    }
}
