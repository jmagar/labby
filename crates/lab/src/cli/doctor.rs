//! `labby doctor` — focused health checks and full audit.
//!
//! Subcommands:
//!   labby doctor              — full audit (system + auth + all service probes)
//!   labby doctor system       — local system checks only
//!   labby doctor auth         — auth/OAuth configuration checks
//!   labby doctor service NAME — probe a single service
//!   labby doctor services     — probe all configured services
//!
//! Exit codes: 0 = ok, 1 = warnings, 2 = failures.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::dispatch::clients::ServiceClients;
use crate::dispatch::doctor::{Finding, Report, Severity, run_auth_checks, run_system_checks};
use crate::output::OutputFormat;
use crate::output::theme::CliTheme;

#[cfg(test)]
pub use crate::dispatch::doctor::service_env_checks;

#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[command(subcommand)]
    pub check: Option<DoctorCheck>,
}

#[derive(Debug, Subcommand)]
pub enum DoctorCheck {
    /// Check auth/OAuth configuration (env vars, files, permissions)
    Auth,
    /// Check public Lab and protected MCP proxy endpoints from caller-visible URLs
    Proxy(DoctorProxyArgs),
    /// Run local system checks (env vars, Docker, disk, toolchain)
    System,
    /// Probe a single configured service
    Service {
        /// Service name (e.g. radarr, sonarr, plex)
        name: String,
    },
    /// Probe all configured services
    Services,
}

#[derive(Debug, Args)]
pub struct DoctorProxyArgs {
    /// Public Lab app URL, e.g. https://lab.example.com (default: LAB_PUBLIC_URL)
    #[arg(long)]
    pub app_url: Option<String>,
    /// Public MCP gateway URL, e.g. https://mcp.example.com (default: LAB_MCP_GATEWAY_URL)
    #[arg(long)]
    pub mcp_url: Option<String>,
    /// Protected MCP public route path, e.g. /syslog
    #[arg(long)]
    pub route: String,
    /// Optional private backend origin for backend-leak probe, e.g. http://mcp-backend:3100
    #[arg(long)]
    pub backend_url: Option<String>,
}

/// Run the doctor subcommand.
pub async fn run(args: DoctorArgs, format: OutputFormat) -> Result<ExitCode> {
    match args.check {
        None => run_full_audit(format).await,
        Some(DoctorCheck::Auth) => run_auth(format).await,
        Some(DoctorCheck::Proxy(args)) => run_proxy(args, format).await,
        Some(DoctorCheck::System) => run_system(format).await,
        Some(DoctorCheck::Service { name }) => run_service(name, format).await,
        Some(DoctorCheck::Services) => run_services(format).await,
    }
}

// ---------------------------------------------------------------------------
// Full audit (existing default behaviour)
// ---------------------------------------------------------------------------

async fn run_full_audit(format: OutputFormat) -> Result<ExitCode> {
    use tokio::sync::mpsc;
    let clients = Arc::new(ServiceClients::from_env());
    let (tx, mut rx) = mpsc::channel(64);

    tokio::spawn(async move {
        crate::dispatch::doctor::service::stream_audit_full(clients, tx).await;
    });

    let mut findings: Vec<Finding> = Vec::new();

    if format.is_json() {
        while let Some(f) = rx.recv().await {
            findings.push(f);
        }
        let report = Report { findings };
        println!("{}", serde_json::to_string_pretty(&report)?);
        Ok(exit_code(&report))
    } else {
        let theme = CliTheme::from_context(format.render_context());
        while let Some(f) = rx.recv().await {
            print_finding(theme, &f);
            findings.push(f);
        }
        Ok(exit_code(&Report { findings }))
    }
}

// ---------------------------------------------------------------------------
// auth subcommand
// ---------------------------------------------------------------------------

async fn run_auth(format: OutputFormat) -> Result<ExitCode> {
    let findings = tokio::task::spawn_blocking(run_auth_checks)
        .await
        .map_err(|e| anyhow::anyhow!("auth.check panicked: {e}"))?;

    let report = Report { findings };

    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(exit_code(&report));
    }

    let theme = CliTheme::from_context(format.render_context());
    print_section(theme, "Auth / OAuth configuration");

    // Group and label findings by check category
    let groups: &[(&str, &str)] = &[
        ("auth:mode", "Mode"),
        ("auth:web-ui-auth-disabled", "Safety gate"),
        ("auth:bearer-token", "Bearer token"),
        ("auth:public-url", "Public URL"),
        ("auth:google-client-id", "Google credentials"),
        ("auth:google-client-secret", "Google credentials"),
        ("auth:sqlite-path", "Auth store"),
        ("auth:key-path", "Auth store"),
        ("auth:sqlite-perms", "Auth store"),
        ("auth:key-perms", "Auth store"),
    ];

    let mut last_group = "";
    for f in &report.findings {
        // Print section header when the group label changes
        let group_label = groups
            .iter()
            .find(|(check, _)| f.check == *check)
            .map(|(_, label)| *label)
            .unwrap_or("Other");
        if group_label != last_group {
            if !last_group.is_empty() {
                println!();
            }
            println!("  {}", theme.primary(&format!("{group_label}:")));
            last_group = group_label;
        }
        print_finding_indented(theme, f);
    }
    println!();

    Ok(exit_code(&report))
}

async fn run_proxy(args: DoctorProxyArgs, format: OutputFormat) -> Result<ExitCode> {
    let app_url = args
        .app_url
        .or_else(|| {
            std::env::var("LAB_PUBLIC_URL")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .ok_or_else(|| anyhow::anyhow!("--app-url is required (or set LAB_PUBLIC_URL)"))?;
    let mcp_url = args
        .mcp_url
        .or_else(|| {
            std::env::var("LAB_MCP_GATEWAY_URL")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .ok_or_else(|| anyhow::anyhow!("--mcp-url is required (or set LAB_MCP_GATEWAY_URL)"))?;
    let mut params = serde_json::json!({
        "app_url": app_url,
        "mcp_url": mcp_url,
        "route": args.route,
    });
    if let Some(backend_url) = &args.backend_url {
        params["backend_url"] = serde_json::Value::String(backend_url.clone());
    }
    let value = crate::dispatch::doctor::dispatch("proxy.check", params)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let report: Report = serde_json::from_value(value)?;

    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(exit_code(&report));
    }

    let theme = CliTheme::from_context(format.render_context());
    print_section(theme, "Reverse proxy checks");
    for finding in &report.findings {
        print_finding_indented(theme, finding);
    }
    println!();

    Ok(exit_code(&report))
}

// ---------------------------------------------------------------------------
// system subcommand
// ---------------------------------------------------------------------------

async fn run_system(format: OutputFormat) -> Result<ExitCode> {
    let findings = tokio::task::spawn_blocking(run_system_checks)
        .await
        .map_err(|e| anyhow::anyhow!("system.checks panicked: {e}"))?;

    let report = Report { findings };

    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(exit_code(&report));
    }

    let theme = CliTheme::from_context(format.render_context());
    print_section(theme, "System checks");

    // Group by check prefix (before ':')
    let groups: &[(&str, &str)] = &[
        ("env:", "Environment variables"),
        ("config:", "Config files"),
        ("docker:", "Docker"),
        ("rust:", "Toolchain"),
        ("disk:", "Disk"),
    ];

    let mut last_group = "";
    for f in &report.findings {
        let prefix = f.check.split(':').next().unwrap_or("");
        let group_label = groups
            .iter()
            .find(|(pfx, _)| pfx.trim_end_matches(':') == prefix)
            .map(|(_, label)| *label)
            .unwrap_or("Other");
        if group_label != last_group {
            if !last_group.is_empty() {
                println!();
            }
            println!("  {}", theme.primary(&format!("{group_label}:")));
            last_group = group_label;
        }
        print_finding_indented(theme, f);
    }
    println!();

    Ok(exit_code(&report))
}

// ---------------------------------------------------------------------------
// service subcommand
// ---------------------------------------------------------------------------

async fn run_service(name: String, format: OutputFormat) -> Result<ExitCode> {
    let clients = Arc::new(ServiceClients::from_env());
    let finding = crate::dispatch::doctor::service::probe_service(&clients, &name, None)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&finding)?);
        let report = Report {
            findings: vec![finding],
        };
        return Ok(exit_code(&report));
    }

    let theme = CliTheme::from_context(format.render_context());
    print_section(theme, &format!("Service probe: {name}"));
    print_finding_indented(theme, &finding);
    println!();

    let report = Report {
        findings: vec![finding],
    };
    Ok(exit_code(&report))
}

// ---------------------------------------------------------------------------
// services subcommand
// ---------------------------------------------------------------------------

async fn run_services(format: OutputFormat) -> Result<ExitCode> {
    use tokio::sync::mpsc;
    let clients = Arc::new(ServiceClients::from_env());
    let (tx, mut rx) = mpsc::channel(64);

    // Stream only service probes (no system/auth checks)
    tokio::spawn(async move {
        crate::dispatch::doctor::service::stream_service_probes(clients, tx).await;
    });

    let mut findings: Vec<Finding> = Vec::new();

    if format.is_json() {
        while let Some(f) = rx.recv().await {
            findings.push(f);
        }
        let report = Report { findings };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(exit_code(&report));
    }

    let theme = CliTheme::from_context(format.render_context());
    print_section(theme, "Service probes");
    while let Some(f) = rx.recv().await {
        println!(
            "    {badge}  {service}: {msg}",
            badge = severity_badge(theme, f.severity),
            service = theme.section(&f.service),
            msg = theme.muted(&f.message),
        );
        findings.push(f);
    }
    println!();

    Ok(exit_code(&Report { findings }))
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn print_section(theme: CliTheme, title: &str) {
    // Aurora section style: bold-cyan title over a muted underline divider.
    println!("{}", theme.heading(title));
    println!();
}

fn print_finding(theme: CliTheme, f: &Finding) {
    println!(
        "{badge} {service} {check}: {msg}",
        badge = severity_badge(theme, f.severity),
        service = theme.muted(format!("[{}]", f.service)),
        check = theme.section(&f.check),
        msg = theme.muted(&f.message),
    );
}

fn print_finding_indented(theme: CliTheme, f: &Finding) {
    // Strip the category prefix (auth:, docker:, etc.) from the check name for cleaner display
    let check_label = f
        .check
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(&f.check);
    println!(
        "    {badge}  {check}: {msg}",
        badge = severity_badge(theme, f.severity),
        check = theme.section(check_label),
        msg = theme.muted(&f.message),
    );
}

/// Status glyph painted via the Aurora success/warn/error tokens, symbol-mode aware.
fn severity_badge(theme: CliTheme, s: Severity) -> String {
    match s {
        Severity::Ok => theme.ok_badge(),
        Severity::Warn => theme.warn_badge(),
        Severity::Fail => theme.error_badge(),
    }
}

fn exit_code(report: &Report) -> ExitCode {
    match report.worst() {
        Severity::Ok => ExitCode::SUCCESS,
        Severity::Warn => ExitCode::from(1),
        Severity::Fail => ExitCode::from(2),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn auth_checks_returns_findings() {
        let findings = crate::dispatch::doctor::run_auth_checks();
        assert!(!findings.is_empty());
        assert!(findings.iter().any(|f| f.check == "auth:mode"));
        assert!(findings.iter().any(|f| f.check == "auth:bearer-token"));
        assert!(findings.iter().any(|f| f.check == "auth:public-url"));
    }
}
