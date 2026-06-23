//! Parameter coercion and validation for deploy actions.
//!
//! Every string that touches `Command::arg()` downstream goes through a
//! regex check here. Deploy never calls `sh -c` except for one well-documented
//! case in `runner::preflight` with an allowlist-validated path.

use labby_apis::deploy::{DeployError, DeployRequest};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

/// Allowed install path prefixes. Any `remote_path` must start with one of these.
const ALLOWED_PREFIXES: &[&str] = &["/usr/local/bin/", "/opt/lab/bin/", "/home/"];

fn alias_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").unwrap())
}

fn service_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[A-Za-z0-9@._-]{1,128}$").unwrap())
}

/// Parse a `run` / `rollback` / `plan` params object into a `DeployRequest`.
pub fn parse_run(params: &Value) -> Result<DeployRequest, DeployError> {
    let targets = params
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError::ValidationFailed {
            field: "targets".into(),
            reason: "required array".into(),
        })?;
    if targets.is_empty() {
        return Err(DeployError::ValidationFailed {
            field: "targets".into(),
            reason: "must contain at least one host".into(),
        });
    }
    let mut hosts = Vec::with_capacity(targets.len());
    for t in targets {
        let s = t.as_str().ok_or_else(|| DeployError::ValidationFailed {
            field: "targets".into(),
            reason: "entries must be strings".into(),
        })?;
        if !alias_re().is_match(s) {
            return Err(DeployError::ValidationFailed {
                field: "targets".into(),
                reason: format!("invalid alias: {s}"),
            });
        }
        hosts.push(s.to_string());
    }
    Ok(DeployRequest {
        targets: hosts,
        max_parallel: params
            .get("max_parallel")
            .and_then(Value::as_u64)
            .filter(|&n| n >= 1) // 0 would mean "no parallelism", treat as absent
            .map(|n| n.min(u64::from(u32::MAX)) as u32),
        fail_fast: params
            .get("fail_fast")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        confirm: params
            .get("confirm")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Validate a systemd unit name supplied via config.
pub fn validate_service_name(s: &str) -> Result<(), DeployError> {
    if service_re().is_match(s) {
        Ok(())
    } else {
        Err(DeployError::ValidationFailed {
            field: "service".into(),
            reason: format!("invalid: {s}"),
        })
    }
}

/// Validate a remote install path against the allowlist and reject path
/// traversal. Deploy refuses to write anywhere outside of
/// `/usr/local/bin/` or `/opt/lab/bin/` by default.
pub fn validate_remote_path(p: &str) -> Result<(), DeployError> {
    if !p.starts_with('/') || p.contains("..") {
        return Err(DeployError::ValidationFailed {
            field: "remote_path".into(),
            reason: "must be absolute and contain no `..`".into(),
        });
    }
    if !ALLOWED_PREFIXES.iter().any(|pref| p.starts_with(pref)) {
        return Err(DeployError::ValidationFailed {
            field: "remote_path".into(),
            reason: format!("not in allowlist: {ALLOWED_PREFIXES:?}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_run_requires_targets_array() {
        let err = parse_run(&json!({})).unwrap_err();
        assert_eq!(err.kind(), "validation_failed");
    }

    #[test]
    fn parse_run_rejects_empty_targets() {
        let err = parse_run(&json!({ "targets": [] })).unwrap_err();
        assert_eq!(err.kind(), "validation_failed");
    }

    #[test]
    fn parse_run_rejects_bad_alias_chars() {
        let err = parse_run(&json!({ "targets": ["bad alias!"] })).unwrap_err();
        assert_eq!(err.kind(), "validation_failed");
    }

    #[test]
    fn parse_run_accepts_valid_targets() {
        let r = parse_run(&json!({ "targets": ["mini1", "mini-2"], "fail_fast": true })).unwrap();
        assert_eq!(r.targets, vec!["mini1".to_string(), "mini-2".to_string()]);
        assert!(r.fail_fast);
    }

    #[test]
    fn remote_path_allowlist_enforced() {
        assert!(validate_remote_path("/etc/passwd").is_err());
        assert!(validate_remote_path("/usr/local/bin/labby").is_ok());
        assert!(validate_remote_path("/opt/lab/bin/labby").is_ok());
        assert!(validate_remote_path("/usr/local/bin/../../etc/passwd").is_err());
    }

    #[test]
    fn service_name_allowlist_enforced() {
        assert!(validate_service_name("lab").is_ok());
        assert!(validate_service_name("lab-worker@foo").is_ok());
        assert!(validate_service_name("; rm -rf /").is_err());
    }
}
