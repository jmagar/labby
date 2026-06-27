//! Gateway pool health check for `gateway.upstreams`.
//!
//! Folds per-upstream health, circuit-breaker state, and OAuth token expiry
//! into the standard `Finding` / `Report` model so operators can see the
//! upstream pool state from `labby doctor gateway.upstreams` or as part of
//! `audit.full`.
//!
//! Data comes exclusively from read-only pool queries — no writes, no
//! reconnections.  Non-essential capability-discovery noise (prompts/resources
//! not implemented) is filtered out so findings stay signal-rich.

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::current_gateway_manager;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::types::UpstreamHealth;

use super::types::{Finding, Report, Severity};

/// Mirror of `projection::operator_visible_upstream_error` — suppresses
/// non-essential capability-discovery noise so doctor findings stay
/// signal-rich (only real connection/auth errors surface).
fn is_nonessential_capability_error(message: &str) -> bool {
    message.starts_with("failed to list prompts from upstream:")
        || message.starts_with("failed to list resources from upstream:")
        || message.starts_with("does not implement MCP prompts discovery")
        || message.starts_with("does not implement MCP resources discovery")
}

#[cfg(feature = "gateway")]
fn dependency_fix_hint(message: &str) -> Option<&'static str> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("ffmpeg: command not found")
        || lower.contains("ffmpeg.exe: command not found")
        || (lower.contains("no such file or directory") && lower.contains("ffmpeg"))
        || (lower.contains("enoent") && lower.contains("ffmpeg"))
    {
        Some("missing leaf dependency `ffmpeg`; suggested fix: sudo apt install ffmpeg")
    } else if lower.contains("failed to run `uvx`")
        || lower.contains("failed to spawn `uvx`")
        || (lower.contains("enoent") && lower.contains("uvx"))
    {
        Some("missing runtime floor `uv`; suggested fix: labby setup --provision --yes")
    } else if lower.contains("failed to run `npx`")
        || lower.contains("failed to spawn `npx`")
        || (lower.contains("enoent") && lower.contains("npx"))
    {
        Some("missing runtime floor `nodejs`; suggested fix: labby setup --provision --yes")
    } else if lower.contains("failed to run `python`")
        || lower.contains("failed to spawn `python`")
        || (lower.contains("enoent") && lower.contains("python"))
    {
        Some("missing runtime floor `python`; suggested fix: labby setup --provision --yes")
    } else {
        None
    }
}

/// Run the gateway upstream pool health check and return a `Report`.
///
/// Returns a single informational `Finding` when the gateway manager is not
/// wired (e.g. a standalone `labby` process without gateway config), rather
/// than an error, because that is a normal operating mode.
pub async fn check_gateway_upstreams() -> Report {
    #[cfg(not(feature = "gateway"))]
    {
        return Report {
            findings: vec![Finding {
                service: "gateway".to_string(),
                check: "pool".to_string(),
                severity: Severity::Warn,
                message: "gateway feature is not compiled into this labby build".to_string(),
            }],
        };
    }
    #[cfg(feature = "gateway")]
    {
        let Some(manager) = current_gateway_manager() else {
            return Report {
                findings: vec![Finding {
                    service: "gateway".to_string(),
                    check: "pool".to_string(),
                    severity: Severity::Warn,
                    message: "gateway manager not wired — no upstream pool to inspect".to_string(),
                }],
            };
        };

        let pool = manager.current_pool().await;
        let Some(pool) = pool else {
            return Report {
                findings: vec![Finding {
                    service: "gateway".to_string(),
                    check: "pool".to_string(),
                    severity: Severity::Warn,
                    message: "gateway pool not yet initialised (no reload or first boot)"
                        .to_string(),
                }],
            };
        };

        let statuses = pool.upstream_status().await;

        if statuses.is_empty() {
            return Report {
                findings: vec![Finding {
                    service: "gateway".to_string(),
                    check: "pool".to_string(),
                    severity: Severity::Ok,
                    message: "gateway pool is active but has no upstreams configured".to_string(),
                }],
            };
        }

        let mut findings = Vec::with_capacity(statuses.len());

        for (name, health) in &statuses {
            // Filter out non-essential capability-discovery noise (prompts/resources
            // not implemented) — mirrors `projection::operator_visible_upstream_error`.
            let last_error = pool
                .upstream_last_error(name)
                .await
                .filter(|msg| !is_nonessential_capability_error(msg));

            let dependency_hint = last_error.as_deref().and_then(dependency_fix_hint);
            let (severity, message) =
                upstream_finding(name, *health, last_error.as_deref(), dependency_hint);
            findings.push(Finding {
                service: "gateway".to_string(),
                check: format!("upstream:{name}"),
                severity,
                message,
            });
        }

        Report { findings }
    }
}

/// Derive severity + human message for one upstream from its health state and
/// last recorded error.
#[cfg(feature = "gateway")]
fn upstream_finding(
    name: &str,
    health: UpstreamHealth,
    last_error: Option<&str>,
    dependency_hint: Option<&str>,
) -> (Severity, String) {
    match health {
        UpstreamHealth::Healthy => {
            if let Some(hint) = dependency_hint {
                (
                    Severity::Warn,
                    format!("`{name}` is routable but has a dependency warning: {hint}"),
                )
            } else if let Some(err) = last_error {
                // Healthy but has a stale warning (e.g. partial capability failure).
                (
                    Severity::Warn,
                    format!("`{name}` is routable but has a recorded error: {err}"),
                )
            } else {
                (Severity::Ok, format!("`{name}` is healthy"))
            }
        }
        UpstreamHealth::Unhealthy {
            consecutive_failures,
        } => {
            use crate::dispatch::upstream::types::CIRCUIT_BREAKER_THRESHOLD;
            let circuit_open = consecutive_failures >= CIRCUIT_BREAKER_THRESHOLD;
            let state = if circuit_open {
                "circuit breaker OPEN"
            } else {
                "degraded"
            };
            let error_detail = last_error.map(|e| format!(": {e}")).unwrap_or_default();
            let error_detail = dependency_hint
                .map(|hint| format!(": {hint}"))
                .unwrap_or(error_detail);
            (
                Severity::Fail,
                format!(
                    "`{name}` is {state} ({consecutive_failures} consecutive failure(s)){error_detail}"
                ),
            )
        }
    }
}

#[cfg(test)]
#[cfg(feature = "gateway")]
mod tests {
    use super::*;
    use crate::dispatch::upstream::types::CIRCUIT_BREAKER_THRESHOLD;

    #[test]
    fn healthy_upstream_no_error_is_ok() {
        let (sev, msg) = upstream_finding("my-server", UpstreamHealth::Healthy, None, None);
        assert!(matches!(sev, Severity::Ok));
        assert!(msg.contains("my-server"));
    }

    #[test]
    fn healthy_upstream_with_stale_error_is_warn() {
        let (sev, _msg) = upstream_finding(
            "my-server",
            UpstreamHealth::Healthy,
            Some("prompts not supported"),
            None,
        );
        assert!(matches!(sev, Severity::Warn));
    }

    #[test]
    fn circuit_open_upstream_is_fail() {
        let (sev, msg) = upstream_finding(
            "my-server",
            UpstreamHealth::Unhealthy {
                consecutive_failures: CIRCUIT_BREAKER_THRESHOLD,
            },
            Some("connection refused"),
            None,
        );
        assert!(matches!(sev, Severity::Fail));
        assert!(msg.contains("circuit breaker OPEN"));
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn degraded_below_threshold_is_fail() {
        let (sev, msg) = upstream_finding(
            "my-server",
            UpstreamHealth::Unhealthy {
                consecutive_failures: CIRCUIT_BREAKER_THRESHOLD - 1,
            },
            None,
            None,
        );
        assert!(matches!(sev, Severity::Fail));
        assert!(msg.contains("degraded"));
        assert!(!msg.contains("circuit breaker OPEN"));
    }

    #[test]
    fn dependency_hint_rewrites_doctor_error_detail() {
        let hint = dependency_fix_hint("ffmpeg: command not found").expect("hint");
        let (sev, msg) = upstream_finding(
            "media-upstream",
            UpstreamHealth::Unhealthy {
                consecutive_failures: 1,
            },
            Some("ffmpeg: command not found"),
            Some(hint),
        );

        assert!(matches!(sev, Severity::Fail));
        assert!(msg.contains("sudo apt install ffmpeg"));
    }

    #[test]
    fn dependency_hint_does_not_match_auth_errors() {
        assert!(dependency_fix_hint("server exited because API_KEY is missing").is_none());
    }
}
