//! `lab` binary entry point.
//!
//! Initializes tracing, loads config, parses clap args, and dispatches
//! to the appropriate subcommand handler. All subsystems are sibling
//! modules declared here.

#![allow(clippy::multiple_crate_versions)]
#![allow(unreachable_pub)]
// binary crate — `pub` items are crate-internal by design

mod acp;
mod api;
mod catalog;
mod cli;
mod config;
mod dispatch;
mod docs;
mod log_fmt;
mod mcp;
mod net;
mod node;
mod oauth;
mod observability;
mod output;
mod process;
mod registry;
#[cfg(test)]
mod test_support;

use std::process::ExitCode;

use clap::Parser;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, filter::filter_fn, fmt, prelude::*};

use crate::cli::Cli;
use crate::dispatch::logs::ingest::LogIngestLayer;
use crate::log_fmt::formatter::PremiumEventFormatter;
use crate::output::{ColorPolicy, RenderEnv, human_output_styling_enabled};

fn human_console_target_enabled(target: &str) -> bool {
    target == "labby"
        || target.starts_with("labby::")
        || target == "lab_apis"
        || target.starts_with("lab_apis::")
        || target == "lab_auth"
        || target.starts_with("lab_auth::")
}

/// Initialize tracing.
///
/// Accepts config.toml log preferences; env vars `LAB_LOG` / `LAB_LOG_FORMAT`
/// override them when set.
fn init_tracing(
    log: &config::LogPreferences,
    color_policy: ColorPolicy,
    filter_override: Option<&str>,
) -> tracing_appender::non_blocking::WorkerGuard {
    // Priority: explicit CLI override > LAB_LOG env var > config.toml > default.
    let filter = if let Some(directive) = filter_override {
        EnvFilter::new(directive)
    } else {
        EnvFilter::try_from_env("LAB_LOG").unwrap_or_else(|_| {
            let directive = log
                .filter
                .as_deref()
                .unwrap_or("labby=info,lab_apis=warn,rmcp=warn");
            EnvFilter::new(directive)
        })
    };

    // ── Rolling file appender (survives OOM — guard must live as long as main) ──
    let log_dir = std::env::var("LAB_LOG_DIR").unwrap_or_else(|_| {
        format!(
            "{}/.local/share/lab/logs",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    std::fs::create_dir_all(&log_dir).ok();

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("lab")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&log_dir)
        .expect("failed to create lab log file appender");

    let (non_blocking_file, _log_guard) = tracing_appender::non_blocking(file_appender);

    let use_json = match std::env::var("LAB_LOG_FORMAT").ok() {
        Some(v) => v.eq_ignore_ascii_case("json"),
        None => log
            .format
            .as_deref()
            .is_some_and(|f| f.eq_ignore_ascii_case("json")),
    };

    if use_json {
        tracing_subscriber::registry()
            .with(filter)
            .with(LogIngestLayer)
            .with(fmt::layer().json().with_writer(std::io::stderr)) // console
            .with(fmt::layer().json().with_writer(non_blocking_file)) // file
            .init();
    } else {
        let fmt_layer = fmt::layer()
            .with_ansi(human_output_styling_enabled(
                color_policy,
                RenderEnv::stderr(),
            ))
            .with_target(false)
            .event_format(PremiumEventFormatter)
            .with_writer(std::io::stderr)
            .with_filter(filter_fn(|metadata| {
                human_console_target_enabled(metadata.target())
            }));
        tracing_subscriber::registry()
            .with(filter)
            .with(LogIngestLayer)
            .with(fmt_layer) // console (pretty)
            .with(fmt::layer().json().with_writer(non_blocking_file)) // file (JSON)
            .init();
    }

    _log_guard
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    if matches!(
        cli.command,
        cli::Command::Docs(_) | cli::Command::Internal(_)
    ) {
        return match cli::dispatch(cli, config::LabConfig::default()).await {
            Ok(code) => code,
            Err(err) => {
                #[allow(clippy::print_stderr)]
                {
                    eprintln!("{err:#}");
                }
                ExitCode::from(1)
            }
        };
    }

    // 1. Load config.toml first (lightweight, no tracing needed).
    //    eprintln is intentional — tracing isn't initialized yet.
    let config = match config::load_toml(&config::toml_candidates()) {
        Ok(cfg) => cfg,
        Err(err) => {
            #[allow(clippy::print_stderr)]
            {
                eprintln!("config.toml parse error: {err:#}");
            }
            return ExitCode::from(2);
        }
    };

    // 2. Init tracing. If a serve-path `--log-level <level>` was given, pass it
    //    directly to avoid mutating the environment (crate forbids unsafe_code).
    // For one-shot CLI commands (not Serve/Mcp) we silence labby's INFO chatter
    // by default — upstream connect/discovery events would otherwise flood
    // ordinary commands like `gateway list`. LAB_LOG still wins when set.
    let log_filter_override: Option<String> = match &cli.command {
        cli::Command::Serve(args) => args
            .log_level
            .as_ref()
            .map(|level| format!("labby={level},warn")),
        cli::Command::Mcp(args) => args
            .log_level
            .as_ref()
            .map(|level| format!("labby={level},warn")),
        _ if std::env::var_os("LAB_LOG").is_none() => {
            Some("labby=warn,lab_apis=warn,rmcp=warn".to_string())
        }
        _ => None,
    };

    // LAB_LOG_COLOR overrides the CLI default when running without a TTY (e.g.
    // inside Docker). The CLI --color flag wins when the user sets it explicitly,
    // but since clap cannot distinguish "user passed --color auto" from "defaulted
    // to auto", the env var only activates when the policy is Auto.
    let color_policy = if cli.color == ColorPolicy::Auto {
        match std::env::var("LAB_LOG_COLOR")
            .ok()
            .as_deref()
            .map(str::to_lowercase)
            .as_deref()
        {
            Some("force" | "always" | "1") => ColorPolicy::Color,
            Some("plain" | "never" | "0") => ColorPolicy::Plain,
            _ => ColorPolicy::Auto,
        }
    } else {
        cli.color
    };

    // _log_guard MUST live for the entire process — dropping it stops file logging.
    let _log_guard = init_tracing(&config.log, color_policy, log_filter_override.as_deref());

    // 3. Load .env files (secrets + URL env vars) for runtime paths.
    // Static docs generation is intentionally metadata-only and must not
    // depend on operator env/config secrets.
    if let Err(err) = config::load_dotenv() {
        tracing::error!("dotenv load error: {err:#}");
        return ExitCode::from(2);
    }

    match cli::dispatch(cli, config).await {
        Ok(code) => code,
        Err(err) => {
            tracing::error!("{err:#}");
            ExitCode::from(1)
        }
    }
}
