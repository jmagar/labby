//! Binary-owned setup checks for Claude plugin hooks.
//!
//! Hooks should call into `labby setup plugin-hook` instead of carrying their
//! own per-plugin shell bootstrap. This module only inspects and repairs local
//! filesystem prerequisites; it never shells out to Claude or external services.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::dispatch::error::ToolError;

use super::client::{env_path, lab_home};

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

pub fn run(mode: Mode) -> Result<SetupReport, ToolError> {
    run_for_paths(mode, lab_home(), env_path())
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
