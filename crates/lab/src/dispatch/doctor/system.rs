//! Local system probes for `system.checks`.
//!
//! All file and env I/O lives here, never in `lab-apis`.

use super::types::{Finding, Severity, service_env_checks};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn path_check(service: &str, label: &str, path: &str, severity_on_missing: Severity) -> Finding {
    let exists = std::path::Path::new(path).exists();
    Finding {
        service: service.to_string(),
        check: label.to_string(),
        severity: if exists {
            Severity::Ok
        } else {
            severity_on_missing
        },
        message: if exists {
            format!("{path} found")
        } else {
            format!("{path} not found")
        },
    }
}

fn writable_check(service: &str, label: &str, path: &str) -> Finding {
    let path_obj = std::path::Path::new(path);
    if !path_obj.exists() {
        return Finding {
            service: service.to_string(),
            check: label.to_string(),
            severity: Severity::Warn,
            message: format!("{path} not found; cannot check writability"),
        };
    }

    let result = if path_obj.is_dir() {
        let test_path = path_obj.join(".doctor_write_test");
        std::fs::write(&test_path, "test").inspect(|_| {
            drop(std::fs::remove_file(test_path));
        })
    } else {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path_obj)
            .map(|_| ())
    };

    match result {
        Ok(()) => Finding {
            service: service.to_string(),
            check: label.to_string(),
            severity: Severity::Ok,
            message: format!("{path} is writable"),
        },
        Err(e) => Finding {
            service: service.to_string(),
            check: label.to_string(),
            severity: Severity::Fail,
            message: format!("{path} is NOT writable: {e}"),
        },
    }
}

fn command_check(service: &str, label: &str, cmd: &str) -> Finding {
    let found = std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    Finding {
        service: service.to_string(),
        check: label.to_string(),
        severity: if found { Severity::Ok } else { Severity::Warn },
        message: if found {
            format!("`{cmd}` is available")
        } else {
            format!("`{cmd}` not found on PATH")
        },
    }
}

/// Verify `docker compose` (the v2 CLI plugin) is actually wired up,
/// not just that the `docker` binary exists.
///
/// Runs `docker compose version` and treats a non-zero exit (or missing
/// binary) as the plugin being unavailable.
fn compose_plugin_check() -> Finding {
    let found = std::process::Command::new("docker")
        .args(["compose", "version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    Finding {
        service: "system".to_string(),
        check: "docker:compose-plugin".to_string(),
        severity: if found { Severity::Ok } else { Severity::Warn },
        message: if found {
            "`docker compose` plugin is available".to_string()
        } else {
            "`docker compose` plugin not available".to_string()
        },
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Run all local system probes: env-var checks, config files, Docker, disk.
///
/// Order: env-var checks first (preserves current `labby doctor` output), then
/// system-level checks.
pub fn run_system_checks() -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();

    // --- Env var checks (current labby doctor behaviour; preserved for output parity) ---
    for (service_name, required_env) in service_env_checks() {
        for env in required_env {
            let present = std::env::var(env.name).is_ok_and(|v| !v.is_empty());
            findings.push(Finding {
                service: service_name.into(),
                check: format!("env:{}", env.name),
                severity: if present {
                    Severity::Ok
                } else {
                    Severity::Fail
                },
                message: if present {
                    format!("{} is set", env.name)
                } else {
                    format!("{} is missing ({})", env.name, env.description)
                },
            });
        }
    }

    // --- Lab config files ---
    let home = std::env::var("HOME").unwrap_or_default();
    let env_path = format!("{home}/.lab/.env");
    findings.push(path_check(
        "lab",
        "config:~/.lab/.env",
        &env_path,
        Severity::Warn,
    ));
    findings.push(writable_check(
        "lab",
        "config:~/.lab/.env:writable",
        &env_path,
    ));

    let lab_dir = format!("{home}/.lab");
    findings.push(writable_check("lab", "config:~/.lab:writable", &lab_dir));

    findings.push(path_check(
        "lab",
        "config:~/.lab/config.toml",
        &format!("{home}/.lab/config.toml"),
        Severity::Warn,
    ));

    // --- AI assistant configs (informational) ---
    for (name, rel_path) in [
        (".claude", "claude"),
        (".codex", "codex"),
        (".gemini", "gemini"),
    ] {
        let full = format!("{home}/{name}");
        let exists = std::path::Path::new(&full).exists();
        findings.push(Finding {
            service: "lab".into(),
            check: format!("config:~/{name}"),
            severity: Severity::Ok,
            message: if exists {
                format!("~/{name} present ({rel_path} detected)")
            } else {
                format!("~/{name} not present")
            },
        });
    }

    // --- Docker ---
    findings.push(path_check(
        "system",
        "docker:socket",
        "/var/run/docker.sock",
        Severity::Warn,
    ));
    findings.push(command_check("system", "docker:cli", "docker"));
    findings.push(compose_plugin_check());

    // --- Rust toolchain ---
    findings.push(command_check("system", "rust:cargo", "cargo"));

    // --- Disk space: warn when / exceeds 90 % used ---
    disk_check(&mut findings);

    findings
}

#[cfg(target_os = "linux")]
fn disk_check(findings: &mut Vec<Finding>) {
    let Ok(output) = std::process::Command::new("df")
        .args(["-h", "--output=pcent", "/"])
        .output()
    else {
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pct: Option<u64> = stdout
        .lines()
        .nth(1)
        .and_then(|l| l.trim().trim_end_matches('%').parse().ok());
    if let Some(used) = pct {
        findings.push(Finding {
            service: "system".into(),
            check: "disk:/".into(),
            severity: if used >= 90 {
                Severity::Warn
            } else {
                Severity::Ok
            },
            message: format!("/ is {used}% used"),
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn disk_check(_findings: &mut Vec<Finding>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_check_warns_when_target_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let finding = writable_check(
            "lab",
            "config:missing:writable",
            dir.path().join("missing.env").to_str().expect("utf8"),
        );
        assert!(matches!(finding.severity, Severity::Warn));
    }

    #[test]
    fn writable_check_accepts_writable_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let finding = writable_check("lab", "config:dir:writable", dir.path().to_str().unwrap());
        assert!(matches!(finding.severity, Severity::Ok));
        assert!(!dir.path().join(".doctor_write_test").exists());
    }

    #[cfg(unix)]
    #[test]
    fn writable_check_tests_actual_file_not_sibling() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".env");
        std::fs::write(&path, "LAB=value\n").expect("write");
        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o444);
        std::fs::set_permissions(&path, permissions).expect("readonly");

        let finding = writable_check("lab", "config:file:writable", path.to_str().unwrap());

        let mut restore = std::fs::metadata(&path).expect("metadata").permissions();
        restore.set_mode(0o644);
        std::fs::set_permissions(&path, restore).expect("restore");

        assert!(matches!(finding.severity, Severity::Fail));
    }
}

// ---------------------------------------------------------------------------
// Auth / OAuth checks
// ---------------------------------------------------------------------------

/// Build an auth-namespace `Finding`. All `auth:*` checks share `service = "auth"`.
fn auth_finding(check: &str, severity: Severity, message: impl Into<String>) -> Finding {
    Finding {
        service: "auth".into(),
        check: check.into(),
        severity,
        message: message.into(),
    }
}

/// Severity + message for an env var that is required when OAuth is enabled.
///
/// - set + valid → Ok
/// - set + invalid → Fail (caller validates and supplies `invalid_message`)
/// - missing + oauth → Fail
/// - missing + non-oauth → Warn
fn oauth_required_env(
    value: &str,
    is_oauth: bool,
    ok_message: impl Into<String>,
    fail_when_oauth: &str,
    warn_otherwise: &str,
) -> (Severity, String) {
    if !value.is_empty() {
        (Severity::Ok, ok_message.into())
    } else if is_oauth {
        (Severity::Fail, fail_when_oauth.to_string())
    } else {
        (Severity::Warn, warn_otherwise.to_string())
    }
}

/// Run auth/OAuth configuration probes.
///
/// Checks env vars, file presence, and Unix file permissions.
/// No network I/O — all checks are local and synchronous.
pub fn run_auth_checks() -> Vec<Finding> {
    let mut findings = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();

    let mode = std::env::var("LAB_AUTH_MODE")
        .unwrap_or_default()
        .to_lowercase();
    let is_oauth = mode == "oauth";

    let bearer_token = std::env::var("LAB_MCP_HTTP_TOKEN").unwrap_or_default();
    let google_id = std::env::var("LAB_GOOGLE_CLIENT_ID").unwrap_or_default();
    let google_secret = std::env::var("LAB_GOOGLE_CLIENT_SECRET").unwrap_or_default();
    let has_google = !google_id.is_empty() && !google_secret.is_empty();

    // --- Auth mode ---
    let mode_label = match mode.as_str() {
        "oauth" => "oauth",
        "bearer" => "bearer",
        _ => "auto (defaulting to bearer)",
    };
    findings.push(auth_finding(
        "auth:mode",
        Severity::Ok,
        format!("LAB_AUTH_MODE={mode_label}"),
    ));

    // --- Safety gate ---
    let web_ui_auth_disabled = match crate::config::resolve_web_ui_auth_disabled_env() {
        Ok(setting) => setting,
        Err(error) => {
            findings.push(auth_finding(
                "auth:web-ui-auth-disabled",
                Severity::Fail,
                format!("{error}"),
            ));
            None
        }
    };
    let web_ui_auth_disabled_source = web_ui_auth_disabled
        .map_or(crate::config::WEB_UI_AUTH_DISABLED_ENV, |setting| {
            setting.source
        });
    let web_ui_auth_disabled_value = web_ui_auth_disabled.is_some_and(|setting| setting.disabled);
    findings.push(auth_finding(
        "auth:web-ui-auth-disabled",
        if web_ui_auth_disabled_value {
            Severity::Fail
        } else {
            Severity::Ok
        },
        if web_ui_auth_disabled_value {
            format!(
                "{web_ui_auth_disabled_source}=true — /v1/* routes are unprotected (dev only, never in production)"
            )
        } else {
            format!(
                "{} not set (protected mode)",
                crate::config::WEB_UI_AUTH_DISABLED_ENV
            )
        },
    ));

    // --- Bearer token ---
    let (bearer_severity, bearer_message) = if !bearer_token.is_empty() {
        let len = bearer_token.len();
        if len < 32 {
            (
                Severity::Warn,
                format!(
                    "LAB_MCP_HTTP_TOKEN is set ({len} chars) — too short; regenerate: openssl rand -hex 32"
                ),
            )
        } else {
            (
                Severity::Ok,
                format!("LAB_MCP_HTTP_TOKEN is set ({len} chars)"),
            )
        }
    } else if is_oauth {
        (
            Severity::Ok,
            "LAB_MCP_HTTP_TOKEN not set — OAuth-only mode (MCP clients must use the OAuth flow)"
                .into(),
        )
    } else {
        (
            Severity::Fail,
            "LAB_MCP_HTTP_TOKEN not set — set it or enable OAuth: LAB_AUTH_MODE=oauth".into(),
        )
    };
    findings.push(auth_finding(
        "auth:bearer-token",
        bearer_severity,
        bearer_message,
    ));

    // --- LAB_PUBLIC_URL ---
    let public_url = std::env::var("LAB_PUBLIC_URL").unwrap_or_default();
    let (url_severity, url_message) = if !public_url.is_empty() {
        if public_url.starts_with("http://") || public_url.starts_with("https://") {
            (Severity::Ok, format!("LAB_PUBLIC_URL={public_url}"))
        } else {
            (
                Severity::Fail,
                format!(
                    "LAB_PUBLIC_URL={public_url} — not a valid URL (must start with http:// or https://)"
                ),
            )
        }
    } else if is_oauth {
        (
            Severity::Fail,
            "LAB_PUBLIC_URL not set — required for OAuth (JWT issuer, audience, metadata URLs)"
                .into(),
        )
    } else {
        (
            Severity::Warn,
            "LAB_PUBLIC_URL not set — required if using LAB_AUTH_MODE=oauth".into(),
        )
    };
    findings.push(auth_finding("auth:public-url", url_severity, url_message));

    // --- Google credentials ---
    let (gid_severity, gid_message) = oauth_required_env(
        &google_id,
        is_oauth,
        "LAB_GOOGLE_CLIENT_ID is set",
        "LAB_GOOGLE_CLIENT_ID not set — required for LAB_AUTH_MODE=oauth",
        "LAB_GOOGLE_CLIENT_ID not set — required if using LAB_AUTH_MODE=oauth",
    );
    findings.push(auth_finding(
        "auth:google-client-id",
        gid_severity,
        gid_message,
    ));

    let (gsec_severity, gsec_message) = oauth_required_env(
        &google_secret,
        is_oauth,
        "LAB_GOOGLE_CLIENT_SECRET is set",
        "LAB_GOOGLE_CLIENT_SECRET not set — required for LAB_AUTH_MODE=oauth",
        "LAB_GOOGLE_CLIENT_SECRET not set — required if using LAB_AUTH_MODE=oauth",
    );
    findings.push(auth_finding(
        "auth:google-client-secret",
        gsec_severity,
        gsec_message,
    ));

    // --- Auth store files (only meaningful when OAuth is configured) ---
    if is_oauth || has_google {
        let sqlite_path = std::env::var("LAB_AUTH_SQLITE_PATH")
            .unwrap_or_else(|_| format!("{home}/.lab/auth.db"));
        let key_path = std::env::var("LAB_AUTH_KEY_PATH")
            .unwrap_or_else(|_| format!("{home}/.lab/auth-jwt.pem"));

        let sqlite_exists = std::path::Path::new(&sqlite_path).exists();
        findings.push(auth_finding(
            "auth:sqlite-path",
            if sqlite_exists {
                Severity::Ok
            } else {
                Severity::Warn
            },
            if sqlite_exists {
                format!("{sqlite_path} found")
            } else {
                format!("{sqlite_path} not found — will be created at first login")
            },
        ));

        let key_exists = std::path::Path::new(&key_path).exists();
        findings.push(auth_finding(
            "auth:key-path",
            if key_exists {
                Severity::Ok
            } else {
                Severity::Warn
            },
            if key_exists {
                format!("{key_path} found")
            } else {
                format!("{key_path} not found — will be generated at first startup")
            },
        ));

        // File permission checks (Unix only)
        #[cfg(unix)]
        {
            if sqlite_exists {
                findings.push(file_perms_check("auth", "auth:sqlite-perms", &sqlite_path));
            }
            if key_exists {
                findings.push(file_perms_check("auth", "auth:key-perms", &key_path));
            }
        }
    }

    findings
}

#[cfg(unix)]
fn file_perms_check(service: &str, label: &str, path: &str) -> Finding {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(meta) => {
            let mode = meta.mode();
            let perms_ok = mode.trailing_zeros() >= 6;
            Finding {
                service: service.to_string(),
                check: label.to_string(),
                severity: if perms_ok {
                    Severity::Ok
                } else {
                    Severity::Fail
                },
                message: if perms_ok {
                    format!("{path}: permissions 0600 (owner-only)")
                } else {
                    format!(
                        "{path}: permissions {:04o} — must be 0600 (fix: chmod 600 {path})",
                        mode & 0o777
                    )
                },
            }
        }
        Err(e) => Finding {
            service: service.to_string(),
            check: label.to_string(),
            severity: Severity::Warn,
            message: format!("{path}: could not read permissions: {e}"),
        },
    }
}
