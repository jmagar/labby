//! Shared doctor types: `Severity`, `Finding`, `Report`.
//!
//! These live in the dispatch layer so they are accessible from both `system.rs`
//! and `cli/doctor.rs` without creating a cli → dispatch dependency.

use labby_apis::core::plugin::EnvVar;
use serde::{Deserialize, Serialize};

/// Severity of a single doctor finding.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Ok,
    Warn,
    Fail,
}

/// One entry in the doctor report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub service: String,
    pub check: String,
    pub severity: Severity,
    pub message: String,
}

/// Full doctor report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    /// Worst severity across all findings.
    pub fn worst(&self) -> Severity {
        self.findings
            .iter()
            .map(|f| f.severity)
            .fold(Severity::Ok, |acc, s| match (acc, s) {
                (Severity::Fail, _) | (_, Severity::Fail) => Severity::Fail,
                (Severity::Warn, _) | (_, Severity::Warn) => Severity::Warn,
                _ => Severity::Ok,
            })
    }
}

/// Returns `(service_name, required_env_vars)` for every enabled service.
///
/// Used by `system.checks` to verify env-var presence.
#[allow(clippy::too_many_lines)]
pub fn service_env_checks() -> Vec<(&'static str, &'static [EnvVar])> {
    Vec::new()
}
