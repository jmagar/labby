//! `labby health` — quick reachability ping for every configured service.

use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;

use crate::output::{OutputFormat, print};

/// One row of the health report.
#[derive(Debug, Clone, Serialize)]
pub struct HealthRow {
    pub service: String,
    pub reachable: bool,
    pub auth_ok: bool,
    pub version: Option<String>,
    pub latency_ms: u64,
    pub message: Option<String>,
}

/// Run the health subcommand.
pub async fn run(format: OutputFormat) -> Result<ExitCode> {
    let mut rows: Vec<HealthRow> = Vec::new();

    // Probe mcpregistry (uses configured or default public registry URL; no credentials required).
    #[cfg(feature = "marketplace")]
    {
        use lab_apis::core::ServiceClient;

        use crate::dispatch::marketplace::mcp_client;
        let row = match mcp_client::require_mcp_client() {
            Ok(client) => match client.health().await {
                Ok(status) => HealthRow {
                    service: "mcpregistry".into(),
                    reachable: status.reachable,
                    auth_ok: status.auth_ok,
                    version: status.version,
                    latency_ms: status.latency_ms,
                    message: status.message,
                },
                Err(_) => HealthRow {
                    service: "mcpregistry".into(),
                    reachable: false,
                    auth_ok: false,
                    version: None,
                    latency_ms: 0,
                    message: Some("health probe failed".into()),
                },
            },
            Err(_) => HealthRow {
                service: "mcpregistry".into(),
                reachable: false,
                auth_ok: false,
                version: None,
                latency_ms: 0,
                message: Some("not configured".into()),
            },
        };
        rows.push(row);
    }

    let any_unhealthy = rows.iter().any(|r| !r.reachable || !r.auth_ok);
    print(&rows, format)?;
    if any_unhealthy {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
