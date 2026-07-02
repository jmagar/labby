//! Binary-owned setup checks for Claude plugin hooks.
//!
//! Hooks should call into `labby setup plugin-hook` instead of carrying their
//! own per-plugin shell bootstrap. This module inspects and repairs local
//! filesystem prerequisites, syncs CLAUDE_PLUGIN_OPTION_* env vars into
//! ~/.labby/.env, exports current .env values as plugin field names, and
//! validates connectivity to the lab MCP server.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::config::env_merge::{self, EnvEntry, MergeRequest};
use crate::dispatch::error::ToolError;

use super::client::{env_path, key_matches_secret_suffix, lab_home};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Check,
    Repair,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SetupCheck {
    pub name: &'static str,
    pub ok: bool,
    pub severity: SetupSeverity,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repaired: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupSeverity {
    Blocking,
    Advisory,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SetupReport {
    pub exit_policy: &'static str,
    pub ran_repair: bool,
    pub no_repair: bool,
    pub blocking_failures: Vec<String>,
    pub advisory_failures: Vec<String>,
    pub ok: bool,
    pub changed: bool,
    pub mode: &'static str,
    pub checks: Vec<SetupCheck>,
}

/// Mapping from `CLAUDE_PLUGIN_OPTION_<OPTION>` to the LAB_* env var name.
///
/// Only options that have a direct LAB_* env var equivalent are listed.
/// `server_url` is intentionally absent — it's a client-side MCP connection
/// target, not a server env var. `mcp_host`/`mcp_port` are config.toml fields
/// with no env var override, so they're also absent.
const PLUGIN_OPTION_MAP: &[(&str, &str)] = &[
    ("CLAUDE_PLUGIN_OPTION_API_TOKEN", "LAB_MCP_HTTP_TOKEN"),
    ("CLAUDE_PLUGIN_OPTION_AUTH_MODE", "LAB_AUTH_MODE"),
    ("CLAUDE_PLUGIN_OPTION_PUBLIC_URL", "LAB_PUBLIC_URL"),
    (
        "CLAUDE_PLUGIN_OPTION_MCP_GATEWAY_URL",
        "LAB_MCP_GATEWAY_URL",
    ),
    ("CLAUDE_PLUGIN_OPTION_ADMIN_ENABLED", "LAB_ADMIN_ENABLED"),
    ("CLAUDE_PLUGIN_OPTION_LOG_FILTER", "LAB_LOG"),
    ("CLAUDE_PLUGIN_OPTION_LOG_FORMAT", "LAB_LOG_FORMAT"),
    ("CLAUDE_PLUGIN_OPTION_CORS_ORIGINS", "LAB_CORS_ORIGINS"),
    (
        "CLAUDE_PLUGIN_OPTION_GOOGLE_CLIENT_ID",
        "LAB_GOOGLE_CLIENT_ID",
    ),
    (
        "CLAUDE_PLUGIN_OPTION_GOOGLE_CLIENT_SECRET",
        "LAB_GOOGLE_CLIENT_SECRET",
    ),
    (
        "CLAUDE_PLUGIN_OPTION_AUTH_ADMIN_EMAIL",
        "LAB_AUTH_ADMIN_EMAIL",
    ),
];

/// Reverse map: LAB_* env var → plugin userConfig field name, for export.
const ENV_TO_FIELD_MAP: &[(&str, &str, bool)] = &[
    // (lab_env_var, userConfig_field_name, is_sensitive)
    ("LAB_MCP_HTTP_TOKEN", "api_token", true),
    ("LAB_AUTH_MODE", "auth_mode", false),
    ("LAB_PUBLIC_URL", "public_url", false),
    ("LAB_MCP_GATEWAY_URL", "mcp_gateway_url", false),
    ("LAB_ADMIN_ENABLED", "admin_enabled", false),
    ("LAB_LOG", "log_filter", false),
    ("LAB_LOG_FORMAT", "log_format", false),
    ("LAB_CORS_ORIGINS", "cors_origins", false),
    ("LAB_GOOGLE_CLIENT_ID", "google_client_id", false),
    ("LAB_GOOGLE_CLIENT_SECRET", "google_client_secret", true),
    ("LAB_AUTH_ADMIN_EMAIL", "auth_admin_email", false),
];

#[derive(Debug, Clone, Serialize)]
pub struct PluginSyncOutcome {
    pub written: usize,
    pub skipped: Vec<String>,
    pub options_found: usize,
    pub env_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginExportEntry {
    pub field: &'static str,
    pub env_var: &'static str,
    pub value: Option<String>,
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginExportOutcome {
    pub fields: Vec<PluginExportEntry>,
    pub env_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectivityOutcome {
    pub server_url: String,
    pub reachable: bool,
    pub latency_ms: Option<u64>,
    pub status: Option<u16>,
    pub message: String,
}

/// Composite result for the `plugin_hook` orchestration action.
///
/// `setup` is always present. `sync` is `None` in Check mode (non-mutating).
/// `connectivity` is always probed since it is read-only.
#[derive(Debug, Clone, Serialize)]
pub struct PluginHookReport {
    pub setup: SetupReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<PluginSyncOutcome>,
    pub connectivity: ConnectivityOutcome,
}

/// Run the full plugin-hook sequence: repair dirs → sync env vars → validate
/// connectivity. Runs repair phase by default (called from SessionStart hook).
pub fn run(mode: Mode) -> Result<SetupReport, ToolError> {
    run_for_paths(mode, lab_home(), env_path())
}

/// Sync CLAUDE_PLUGIN_OPTION_* env vars into ~/.labby/.env.
///
/// Only non-empty options are written; existing .env values are preserved
/// when the corresponding option var is absent or empty.
pub fn sync_plugin_env() -> Result<PluginSyncOutcome, ToolError> {
    sync_plugin_env_to(env_path())
}

pub fn sync_plugin_env_to(env: PathBuf) -> Result<PluginSyncOutcome, ToolError> {
    let entries: Vec<EnvEntry> = PLUGIN_OPTION_MAP
        .iter()
        .filter_map(|(option_var, lab_var)| {
            let value = std::env::var(option_var).ok().filter(|v| !v.is_empty())?;
            Some(EnvEntry::new(lab_var.to_string(), value))
        })
        .collect();

    let options_found = entries.len();
    if options_found == 0 {
        return Ok(PluginSyncOutcome {
            written: 0,
            skipped: vec![],
            options_found: 0,
            env_path: env.display().to_string(),
        });
    }

    // Ensure ~/.labby/ and ~/.labby/.env exist before merging.
    if let Some(parent) = env.parent() {
        fs::create_dir_all(parent).map_err(|e| ToolError::Sdk {
            sdk_kind: "setup_repair_failed".into(),
            message: format!("failed to create {}: {e}", parent.display()),
        })?;
    }
    if !env.exists() {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&env)
            .map_err(|e| ToolError::Sdk {
                sdk_kind: "setup_repair_failed".into(),
                message: format!("failed to create {}: {e}", env.display()),
            })?;
    }

    let outcome = env_merge::merge(
        &env,
        MergeRequest {
            entries,
            force: false,
            expected_mtime: None,
        },
    )
    .map_err(|e| ToolError::Sdk {
        sdk_kind: e.kind().to_string(),
        message: e.to_string(),
    })?;

    Ok(PluginSyncOutcome {
        written: outcome.written,
        skipped: outcome.skipped,
        options_found,
        env_path: env.display().to_string(),
    })
}

/// Read ~/.labby/.env and return current values keyed by userConfig field name.
/// Sensitive values are redacted to `"***"`.
pub fn export_plugin_env() -> Result<PluginExportOutcome, ToolError> {
    export_plugin_env_from(env_path())
}

pub fn export_plugin_env_from(env: PathBuf) -> Result<PluginExportOutcome, ToolError> {
    let raw = if env.exists() {
        fs::read_to_string(&env).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("failed to read {}: {e}", env.display()),
        })?
    } else {
        String::new()
    };

    // Parse key=value pairs from the env file.
    let mut env_map: HashMap<&str, String> = HashMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            env_map.insert(k.trim(), env_merge::strip_quotes(v.trim()));
        }
    }

    let fields = ENV_TO_FIELD_MAP
        .iter()
        .map(|(lab_var, field, sensitive)| {
            let raw_value = env_map.get(*lab_var).cloned();
            let value = raw_value.map(|v| {
                if *sensitive || key_matches_secret_suffix(lab_var) {
                    "***".to_string()
                } else {
                    v
                }
            });
            PluginExportEntry {
                field,
                env_var: lab_var,
                value,
                sensitive: *sensitive,
            }
        })
        .collect();

    Ok(PluginExportOutcome {
        fields,
        env_path: env.display().to_string(),
    })
}

/// Validate connectivity to the lab MCP server at `{server_url}/health`.
///
/// Uses `CLAUDE_PLUGIN_OPTION_SERVER_URL` if `server_url` is not provided.
/// Non-blocking: a failed probe is reported as `reachable: false`, not an error.
pub async fn validate_connectivity(server_url: Option<&str>) -> ConnectivityOutcome {
    let url_owned: String;
    let url = match server_url.filter(|s| !s.is_empty()) {
        Some(u) => u,
        None => {
            url_owned = std::env::var("CLAUDE_PLUGIN_OPTION_SERVER_URL").unwrap_or_default();
            if url_owned.is_empty() {
                "http://localhost:8765"
            } else {
                url_owned.as_str()
            }
        }
    };

    // Strip trailing /mcp if the user copied from .mcp.json.
    let base = url.trim_end_matches('/').trim_end_matches("/mcp");
    let health_url = format!("{base}/health");

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return ConnectivityOutcome {
                server_url: base.to_string(),
                reachable: false,
                latency_ms: None,
                status: None,
                message: format!("failed to build HTTP client: {e}"),
            };
        }
    };

    let start = std::time::Instant::now();
    match client.get(&health_url).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let latency = start.elapsed().as_millis() as u64;
            ConnectivityOutcome {
                server_url: base.to_string(),
                reachable: status < 400,
                latency_ms: Some(latency),
                status: Some(status),
                message: format!("connected ({status}) in {latency}ms"),
            }
        }
        Err(e) => ConnectivityOutcome {
            server_url: base.to_string(),
            reachable: false,
            latency_ms: None,
            status: None,
            message: format!("unreachable: {e}"),
        },
    }
}

fn run_for_paths(mode: Mode, lab_home: PathBuf, env: PathBuf) -> Result<SetupReport, ToolError> {
    let mut checks = Vec::with_capacity(2);
    let mut changed = false;

    checks.push(check_lab_home(mode, &lab_home, &mut changed)?);
    checks.push(check_env_file(mode, &env, &mut changed)?);

    let blocking_failures = checks
        .iter()
        .filter(|check| !check.ok && check.severity == SetupSeverity::Blocking)
        .map(|check| check.name.to_string())
        .collect::<Vec<_>>();
    let advisory_failures = checks
        .iter()
        .filter(|check| !check.ok && check.severity == SetupSeverity::Advisory)
        .map(|check| check.name.to_string())
        .collect::<Vec<_>>();
    let exit_policy = if !blocking_failures.is_empty() {
        "blocking_failure"
    } else if !advisory_failures.is_empty() {
        "advisory_failure"
    } else {
        "success"
    };

    Ok(SetupReport {
        exit_policy,
        ran_repair: mode == Mode::Repair,
        no_repair: mode == Mode::Check,
        ok: blocking_failures.is_empty(),
        changed,
        mode: match mode {
            Mode::Check => "check",
            Mode::Repair => "repair",
        },
        blocking_failures,
        advisory_failures,
        checks,
    })
}

fn check_lab_home(mode: Mode, path: &Path, changed: &mut bool) -> Result<SetupCheck, ToolError> {
    if path.is_dir() {
        return Ok(ok_check("lab_home", path, None));
    }
    if path.exists() {
        return Ok(failed_check(
            "lab_home",
            path,
            SetupSeverity::Blocking,
            "path exists but is not a directory",
        ));
    }
    if mode == Mode::Repair {
        fs::create_dir_all(path).map_err(|error| io_error("lab_home", path, error))?;
        *changed = true;
        return Ok(ok_check("lab_home", path, Some(true)));
    }
    Ok(failed_check(
        "lab_home",
        path,
        SetupSeverity::Blocking,
        "directory is missing",
    ))
}

fn check_env_file(mode: Mode, path: &Path, changed: &mut bool) -> Result<SetupCheck, ToolError> {
    if path.is_file() {
        return Ok(ok_check("env_file", path, None));
    }
    if path.exists() {
        return Ok(failed_check(
            "env_file",
            path,
            SetupSeverity::Blocking,
            "path exists but is not a regular file",
        ));
    }
    if mode == Mode::Repair {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error("env_file", parent, error))?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| io_error("env_file", path, error))?;
        *changed = true;
        return Ok(ok_check("env_file", path, Some(true)));
    }
    Ok(failed_check(
        "env_file",
        path,
        SetupSeverity::Advisory,
        "file is missing; process env can supply setup values",
    ))
}

fn ok_check(name: &'static str, path: &Path, repaired: Option<bool>) -> SetupCheck {
    SetupCheck {
        name,
        ok: true,
        severity: SetupSeverity::Advisory,
        path: path.display().to_string(),
        repaired,
        message: None,
    }
}

fn failed_check(
    name: &'static str,
    path: &Path,
    severity: SetupSeverity,
    message: &'static str,
) -> SetupCheck {
    SetupCheck {
        name,
        ok: false,
        severity,
        path: path.display().to_string(),
        repaired: None,
        message: Some(message.to_string()),
    }
}

fn io_error(check: &'static str, path: &Path, error: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "setup_repair_failed".into(),
        message: format!("failed to repair {check} at {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_reports_missing_paths_without_creating_them() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");

        let report = run_for_paths(Mode::Check, home.clone(), env.clone()).expect("check report");

        assert!(!report.ok);
        assert!(!report.changed);
        assert_eq!(report.exit_policy, "blocking_failure");
        assert!(report.no_repair);
        assert!(!report.ran_repair);
        assert_eq!(report.blocking_failures, ["lab_home"]);
        assert_eq!(report.advisory_failures, ["env_file"]);
        assert!(!home.exists());
        assert!(!env.exists());
        assert_eq!(report.checks.len(), 2);
        assert_eq!(report.checks[0].name, "lab_home");
        assert_eq!(report.checks[1].name, "env_file");
    }

    #[test]
    fn repair_creates_lab_home_and_env_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");

        let report = run_for_paths(Mode::Repair, home.clone(), env.clone()).expect("repair report");

        assert!(report.ok);
        assert!(report.changed);
        assert_eq!(report.exit_policy, "success");
        assert!(report.ran_repair);
        assert!(!report.no_repair);
        assert!(report.blocking_failures.is_empty());
        assert!(report.advisory_failures.is_empty());
        assert!(home.is_dir());
        assert!(env.is_file());
        assert!(report.checks.iter().all(|check| check.ok));
        assert_eq!(report.checks[0].repaired, Some(true));
        assert_eq!(report.checks[1].repaired, Some(true));
    }

    #[test]
    fn repair_is_idempotent_after_paths_exist() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");
        fs::create_dir_all(&home).expect("lab home");
        fs::write(&env, "RADARR_URL=http://localhost\n").expect("env file");

        let report = run_for_paths(Mode::Repair, home, env).expect("repair report");

        assert!(report.ok);
        assert!(!report.changed);
        assert!(report.checks.iter().all(|check| check.repaired.is_none()));
    }
}
