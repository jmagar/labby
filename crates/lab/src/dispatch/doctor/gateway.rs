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

use crate::dispatch::gateway::current_gateway_manager;
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

/// Run the gateway upstream pool health check and return a `Report`.
///
/// Returns a single informational `Finding` when the gateway manager is not
/// wired (e.g. a standalone `labby` process without gateway config), rather
/// than an error, because that is a normal operating mode.
pub async fn check_gateway_upstreams() -> Report {
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
                message: "gateway pool not yet initialised (no reload or first boot)".to_string(),
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

        let (severity, message) = upstream_finding(name, *health, last_error.as_deref());
        findings.push(Finding {
            service: "gateway".to_string(),
            check: format!("upstream:{name}"),
            severity,
            message,
        });
    }

    Report { findings }
}

/// Derive severity + human message for one upstream from its health state and
/// last recorded error.
fn upstream_finding(
    name: &str,
    health: UpstreamHealth,
    last_error: Option<&str>,
) -> (Severity, String) {
    match health {
        UpstreamHealth::Healthy => {
            if let Some(err) = last_error {
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
mod tests {
    use super::*;
    use crate::dispatch::upstream::types::CIRCUIT_BREAKER_THRESHOLD;

    #[test]
    fn healthy_upstream_no_error_is_ok() {
        let (sev, msg) = upstream_finding("my-server", UpstreamHealth::Healthy, None);
        assert!(matches!(sev, Severity::Ok));
        assert!(msg.contains("my-server"));
    }

    #[test]
    fn healthy_upstream_with_stale_error_is_warn() {
        let (sev, _msg) = upstream_finding(
            "my-server",
            UpstreamHealth::Healthy,
            Some("prompts not supported"),
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
        );
        assert!(matches!(sev, Severity::Fail));
        assert!(msg.contains("degraded"));
        assert!(!msg.contains("circuit breaker OPEN"));
    }
}
