//! Claude Code plugin lifecycle helpers for the setup service.
//!
//! This module is intentionally lab-local: it shells out to the user's
//! `claude` CLI and reads local setup/env state. `lab-apis` stays pure.

use std::collections::HashSet;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command;

use crate::dispatch::error::ToolError;
use crate::dispatch::redact::{is_sensitive_key, redact_url};
use crate::registry::service_meta;

use super::client::{cached_registry, env_path};
use super::draft;

const DEFAULT_ORG: &str = match option_env!("LAB_PLUGIN_ORG") {
    Some(value) => value,
    None => "lab",
};
const DEFAULT_SCOPE: &str = "user";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
const CACHE_TTL: Duration = Duration::from_secs(8);

static INSTALLED_CACHE: OnceLock<Mutex<Option<(Instant, Vec<InstalledPlugin>)>>> = OnceLock::new();
static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledPlugin {
    pub id: String,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ServiceStatus {
    pub service: String,
    pub configured: bool,
    pub plugin_installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    pub draft_present: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PluginMutationResult {
    pub ok: bool,
    pub package_id: String,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

pub async fn installed_plugins(force: bool) -> Result<Vec<InstalledPlugin>, ToolError> {
    if !force
        && let Some((instant, plugins)) = cache().lock().expect("installed cache poisoned").as_ref()
        && instant.elapsed() < CACHE_TTL
    {
        return Ok(plugins.clone());
    }

    let output = run_claude(&["plugin", "list", "--json"], "plugin_list_failed").await?;
    let plugins = parse_installed_plugins(&output.stdout)?;
    *cache().lock().expect("installed cache poisoned") = Some((Instant::now(), plugins.clone()));
    Ok(plugins)
}

pub async fn install_plugin(service: &str) -> Result<PluginMutationResult, ToolError> {
    let package_id = package_for_service(service)?;
    ensure_service_configured(service)?;
    let _guard = InFlightGuard::acquire(package_id.clone())?;
    tracing::info!(
        surface = "dispatch",
        service = "setup",
        action = "install_plugin.intent",
        package_id,
        scope = DEFAULT_SCOPE,
        "installing Claude Code plugin"
    );
    let output = run_claude(
        &[
            "plugin",
            "install",
            "--scope",
            DEFAULT_SCOPE,
            "--",
            package_id.as_str(),
        ],
        "plugin_install_failed",
    )
    .await?;
    clear_cache();
    Ok(PluginMutationResult {
        ok: true,
        package_id,
        scope: DEFAULT_SCOPE.to_string(),
        stdout: non_empty(output.stdout),
        stderr: non_empty(output.stderr),
    })
}

pub async fn uninstall_plugin(service: &str) -> Result<PluginMutationResult, ToolError> {
    let package_id = package_for_service(service)?;
    let _guard = InFlightGuard::acquire(package_id.clone())?;
    tracing::info!(
        surface = "dispatch",
        service = "setup",
        action = "uninstall_plugin.intent",
        package_id,
        scope = DEFAULT_SCOPE,
        "uninstalling Claude Code plugin"
    );
    let output = run_claude(
        &[
            "plugin",
            "uninstall",
            "--scope",
            DEFAULT_SCOPE,
            "--",
            package_id.as_str(),
        ],
        "plugin_uninstall_failed",
    )
    .await?;
    clear_cache();
    Ok(PluginMutationResult {
        ok: true,
        package_id,
        scope: DEFAULT_SCOPE.to_string(),
        stdout: non_empty(output.stdout),
        stderr: non_empty(output.stderr),
    })
}

pub async fn services_status() -> Result<Vec<ServiceStatus>, ToolError> {
    let installed = installed_plugins(false).await.unwrap_or_default();
    let draft_entries = draft::read_entries(&super::client::draft_path());
    let draft_keys: HashSet<&str> = draft_entries
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    let env_entries = draft::read_entries(&env_path());
    let env_keys: HashSet<&str> = env_entries.iter().map(|entry| entry.key.as_str()).collect();

    let mut out = Vec::new();
    for entry in cached_registry().services() {
        let Some(meta) = service_meta(entry.name) else {
            continue;
        };
        let package_id = package_id_for_service(entry.name);
        let installed_plugin = installed.iter().find(|plugin| plugin.id == package_id);
        out.push(ServiceStatus {
            service: entry.name.to_string(),
            configured: meta
                .required_env
                .iter()
                .all(|var| env_keys.contains(var.name) || std::env::var(var.name).is_ok()),
            plugin_installed: installed_plugin.is_some(),
            plugin_version: installed_plugin.and_then(|plugin| plugin.version.clone()),
            draft_present: meta
                .required_env
                .iter()
                .any(|var| draft_keys.contains(var.name)),
        });
    }
    out.sort_by(|a, b| a.service.cmp(&b.service));
    Ok(out)
}

fn cache() -> &'static Mutex<Option<(Instant, Vec<InstalledPlugin>)>> {
    INSTALLED_CACHE.get_or_init(|| Mutex::new(None))
}

fn clear_cache() {
    *cache().lock().expect("installed cache poisoned") = None;
}

fn package_for_service(service: &str) -> Result<String, ToolError> {
    if service_meta(service).is_none() || cached_registry().service(service).is_none() {
        return Err(ToolError::Sdk {
            sdk_kind: "unknown_service".into(),
            message: format!("service `{service}` is not registered in this binary"),
        });
    }
    validate_package_segment(service)?;
    let package_id = package_id_for_service(service);
    ensure_package_allowed(&package_id)?;
    Ok(package_id)
}

fn package_id_for_service(service: &str) -> String {
    format!("lab-{service}@{DEFAULT_ORG}").to_ascii_lowercase()
}

fn validate_package_segment(segment: &str) -> Result<(), ToolError> {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return invalid_package(segment);
    };
    if !first.is_ascii_alphanumeric() {
        return invalid_package(segment);
    }
    if chars.any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')) {
        return invalid_package(segment);
    }
    Ok(())
}

fn invalid_package(segment: &str) -> Result<(), ToolError> {
    Err(ToolError::InvalidParam {
        param: "service".into(),
        message: format!("invalid service/package segment `{segment}`"),
    })
}

fn ensure_package_allowed(package_id: &str) -> Result<(), ToolError> {
    let Some((_name, org)) = package_id.split_once('@') else {
        return Err(ToolError::Sdk {
            sdk_kind: "package_not_allowlisted".into(),
            message: "package id must include an org suffix".into(),
        });
    };
    if org.eq_ignore_ascii_case(DEFAULT_ORG) {
        Ok(())
    } else {
        Err(ToolError::Sdk {
            sdk_kind: "package_not_allowlisted".into(),
            message: format!("package org `{org}` is not allowlisted"),
        })
    }
}

fn ensure_service_configured(service: &str) -> Result<(), ToolError> {
    let Some(meta) = service_meta(service) else {
        return Err(ToolError::Sdk {
            sdk_kind: "unknown_service".into(),
            message: format!("service `{service}` is not registered in this binary"),
        });
    };
    let env_entries = draft::read_entries(&env_path());
    let env_keys: HashSet<&str> = env_entries.iter().map(|entry| entry.key.as_str()).collect();
    let missing: Vec<&str> = meta
        .required_env
        .iter()
        .filter_map(|var| {
            let present = env_keys.contains(var.name)
                || std::env::var(var.name)
                    .ok()
                    .is_some_and(|value| !value.trim().is_empty());
            (!present).then_some(var.name)
        })
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "service_not_configured".into(),
        message: format!(
            "service `{service}` is missing required env vars: {}",
            missing.join(", ")
        ),
    })
}

#[derive(Debug)]
struct CommandOutput {
    stdout: String,
    stderr: String,
}

async fn run_claude(args: &[&str], failure_kind: &'static str) -> Result<CommandOutput, ToolError> {
    let claude_bin = std::env::var("LAB_CLAUDE_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "claude".to_string());
    let mut command = Command::new(&claude_bin);
    command.args(args).stdin(Stdio::null());
    let child = command.output();
    let output = match tokio::time::timeout(COMMAND_TIMEOUT, child).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ToolError::Sdk {
                sdk_kind: "claude_cli_unavailable".into(),
                message: format!("claude CLI `{claude_bin}` is not available"),
            });
        }
        Ok(Err(error)) => {
            return Err(ToolError::Sdk {
                sdk_kind: "claude_cli_unavailable".into(),
                message: format!("claude CLI `{claude_bin}` failed to start: {error}"),
            });
        }
        Err(_) => {
            let kind = if failure_kind == "plugin_install_failed" {
                "plugin_install_timeout"
            } else {
                "claude_cli_unavailable"
            };
            return Err(ToolError::Sdk {
                sdk_kind: kind.into(),
                message: format!(
                    "claude plugin command did not finish within {}s",
                    COMMAND_TIMEOUT.as_secs()
                ),
            });
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        return Ok(CommandOutput {
            stdout: stdout.trim().to_string(),
            stderr: redact_stderr(&stderr),
        });
    }
    let redacted = redact_stderr(&stderr);
    tracing::warn!(
        surface = "dispatch",
        service = "setup",
        action = "claude.plugin",
        args = ?args,
        stderr = %redacted,
        "claude plugin command failed"
    );
    Err(ToolError::Sdk {
        sdk_kind: failure_kind.into(),
        message: summarize_stderr(&redacted),
    })
}

fn parse_installed_plugins(stdout: &str) -> Result<Vec<InstalledPlugin>, ToolError> {
    let value: Value = serde_json::from_str(stdout).map_err(|error| ToolError::Sdk {
        sdk_kind: "decode_error".into(),
        message: format!("claude plugin list JSON parse failed: {error}"),
    })?;
    let array = match value {
        Value::Array(array) => array,
        Value::Object(mut object) => object
            .remove("plugins")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut plugins = Vec::new();
    for item in array {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        plugins.push(InstalledPlugin {
            id: id.to_ascii_lowercase(),
            scope: item
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_SCOPE)
                .to_string(),
            version: item
                .get("version")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            enabled: item.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        });
    }
    Ok(plugins)
}

fn redact_stderr(stderr: &str) -> String {
    stderr
        .lines()
        .map(redact_stderr_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_stderr_line(line: &str) -> String {
    let mut out = line
        .split_whitespace()
        .map(|token| {
            if token.contains("://") {
                redact_url(token)
            } else if let Some((key, _)) = token.split_once('=')
                && is_sensitive_key(key)
            {
                format!("{key}=[redacted]")
            } else if token.eq_ignore_ascii_case("bearer") {
                "Bearer".to_string()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    if let Some(idx) = out.to_ascii_lowercase().find("bearer ") {
        let start = idx + "bearer ".len();
        if out[start..].len() >= 20 {
            out.replace_range(start.., "[redacted]");
        }
    }
    out
}

fn summarize_stderr(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("claude plugin command failed")
        .to_string()
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

struct InFlightGuard {
    package_id: String,
}

impl InFlightGuard {
    fn acquire(package_id: String) -> Result<Self, ToolError> {
        let mut set = in_flight().lock().expect("in-flight set poisoned");
        if !set.insert(package_id.clone()) {
            return Err(ToolError::Conflict {
                message: format!("plugin operation already in flight for `{package_id}`"),
                existing_id: package_id,
            });
        }
        Ok(Self { package_id })
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        in_flight()
            .lock()
            .expect("in-flight set poisoned")
            .remove(&self.package_id);
    }
}

fn in_flight() -> &'static Mutex<HashSet<String>> {
    IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn installed_plugins_json(plugins: Vec<InstalledPlugin>) -> Value {
    json!({ "plugins": plugins })
}

pub fn services_status_json(statuses: Vec<ServiceStatus>) -> Value {
    json!({ "services": statuses })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_segment_validation_rejects_injection_shapes() {
        assert!(validate_package_segment("plex").is_ok());
        assert!(validate_package_segment("uptime_kuma").is_ok());
        assert!(validate_package_segment("--config").is_err());
        assert!(validate_package_segment("plex;rm").is_err());
        assert!(validate_package_segment("../plex").is_err());
    }

    #[test]
    fn allowlist_is_exact_org_match() {
        assert!(ensure_package_allowed("lab-plex@lab").is_ok());
        assert!(ensure_package_allowed("lab-plex@labxyz").is_err());
    }

    #[test]
    fn parses_claude_plugin_list_json() {
        let plugins = parse_installed_plugins(
            r#"[{"id":"lab-plex@lab","scope":"user","version":"abc","enabled":true}]"#,
        )
        .unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id, "lab-plex@lab");
        assert_eq!(plugins[0].version.as_deref(), Some("abc"));
    }

    #[test]
    fn stderr_redaction_masks_known_secret_shapes() {
        let stderr = "clone https://user:pass@example.com/repo?token=abc\nOPENAI_API_KEY=secret\nAuthorization: Bearer abcdefghijklmnopqrstuvwxyz";
        let redacted = redact_stderr(stderr);
        assert!(!redacted.contains("pass"));
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(redacted.contains("token=[redacted]"));
        assert!(redacted.contains("OPENAI_API_KEY=[redacted]"));
    }
}
