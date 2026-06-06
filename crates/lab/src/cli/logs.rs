use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::cli::helpers::run_action_command;
use crate::config::LabConfig;
use crate::dispatch::helpers::env_non_empty;
use crate::dispatch::logs::client::{
    bootstrap_store_backed_log_system, resolve_retention, resolve_store_path,
};
use crate::dispatch::logs::dispatch::dispatch_with_system;
use crate::dispatch::logs::forward::{ForwardConfig, resolve_node_id};
use crate::dispatch::logs::types::{LogQuery, LogSystem, LogTailRequest};
use crate::output::{OutputFormat, print};

#[derive(Debug, Args)]
pub struct LogsArgs {
    #[command(subcommand)]
    pub command: LogsCommand,
}

#[derive(Debug, Subcommand)]
pub enum LogsCommand {
    /// Search fleet logs for a device from the master control plane.
    Search {
        /// Device (node) ID to search logs for.
        #[arg(value_name = "DEVICE")]
        device: String,
        /// Query string to search for.
        #[arg(value_name = "QUERY")]
        query: String,
    },
    /// Search or inspect the local-master runtime log store.
    Local(LocalLogsArgs),
    /// Forward this node's syslog to the master log store (peer mode).
    Forward(ForwardArgs),
}

#[derive(Debug, Args)]
pub struct ForwardArgs {
    /// Override the master base URL (default: LAB_MASTER_URL).
    #[arg(long, env = "LAB_MASTER_URL")]
    pub master_url: Option<String>,
    /// Override the bearer token (default: LAB_MASTER_TOKEN or LAB_MCP_HTTP_TOKEN).
    #[arg(long, env = "LAB_MASTER_TOKEN")]
    pub master_token: Option<String>,
    /// Node ID to stamp on every forwarded event (default: LAB_NODE_ID or hostname).
    #[arg(long, env = "LAB_NODE_ID")]
    pub node_id: Option<String>,
    /// How many events to batch per request (default 200).
    #[arg(long, default_value = "200")]
    pub batch_size: usize,
    /// Skip journald and read directly from /var/log/syslog.
    #[arg(long)]
    pub syslog_only: bool,
}

#[derive(Debug, Args)]
pub struct LocalLogsArgs {
    #[command(subcommand)]
    pub command: LocalLogsCommand,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum LocalLogsCommand {
    /// Search the persistent local log store.
    Search(LocalSearchArgs),
    /// Read a bounded follow-up window from the persistent local log store.
    Tail(LocalTailArgs),
    /// Inspect local retention and drop counters.
    Stats,
    /// Live push is HTTP SSE only in v1; this command fails with guidance.
    Stream,
}

#[derive(Debug, Args, Default)]
pub struct LocalSearchArgs {
    /// Text to search for in log messages (also accepted as a positional argument).
    #[arg(long, value_name = "QUERY")]
    pub text: Option<String>,
    /// Positional query shorthand — equivalent to `--text <QUERY>`.
    #[arg(value_name = "QUERY", conflicts_with = "text")]
    pub positional_query: Option<String>,
    /// Only include events after this Unix timestamp (milliseconds).
    #[arg(long)]
    pub after_ts: Option<i64>,
    /// Only include events before this Unix timestamp (milliseconds).
    #[arg(long)]
    pub before_ts: Option<i64>,
    #[arg(long = "level")]
    pub levels: Vec<crate::dispatch::logs::types::LogLevel>,
    #[arg(long = "subsystem")]
    pub subsystems: Vec<crate::dispatch::logs::types::Subsystem>,
    #[arg(long = "surface")]
    pub surfaces: Vec<crate::dispatch::logs::types::Surface>,
    /// Filter by dispatch action name.
    #[arg(long)]
    pub action: Option<String>,
    /// Filter by request ID (x-request-id header).
    #[arg(long)]
    pub request_id: Option<String>,
    /// Filter by session ID.
    #[arg(long)]
    pub session_id: Option<String>,
    /// Filter by correlation ID.
    #[arg(long)]
    pub correlation_id: Option<String>,
    /// Maximum number of results to return.
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Args, Default)]
pub struct LocalTailArgs {
    /// Only include events after this Unix timestamp (milliseconds).
    #[arg(long)]
    pub after_ts: Option<i64>,
    /// Resume from after this event ID (exclusive).
    #[arg(long)]
    pub since_event_id: Option<String>,
    /// Maximum number of results to return.
    #[arg(long)]
    pub limit: Option<usize>,
}

pub async fn run(args: LogsArgs, format: OutputFormat, config: &LabConfig) -> Result<ExitCode> {
    match args.command {
        LogsCommand::Search { device, query } => {
            let value = search_logs(config, &device, &query).await?;
            print(&value, format)?;
            Ok(ExitCode::SUCCESS)
        }
        LogsCommand::Local(local) => run_local(local, format, config).await,
        LogsCommand::Forward(args) => run_forward(args, config).await,
    }
}

pub async fn search_logs(config: &LabConfig, device_id: &str, query: &str) -> Result<Value> {
    crate::node::master_client::MasterClient::from_config(config, None)?
        .search_logs(device_id, query)
        .await
}

pub async fn run_local(
    local: LocalLogsArgs,
    format: OutputFormat,
    config: &LabConfig,
) -> Result<ExitCode> {
    let system = local_log_system(config).await?;
    let (action, params) = match local.command {
        LocalLogsCommand::Search(args) => (
            "logs.search".to_string(),
            json!({ "query": build_search_query(args) }),
        ),
        LocalLogsCommand::Tail(args) => (
            "logs.tail".to_string(),
            serde_json::to_value(LogTailRequest {
                after_ts: args.after_ts,
                since_event_id: args.since_event_id,
                limit: args.limit,
            })?,
        ),
        LocalLogsCommand::Stats => ("logs.stats".to_string(), json!({})),
        LocalLogsCommand::Stream => {
            return Err(anyhow::anyhow!(
                "true live log streaming is only available over HTTP SSE at `/v1/logs/stream`; use `labby logs local tail` for bounded follow-up windows"
            ));
        }
    };

    run_action_command("logs", action, params, format, move |action, params| {
        let system = Arc::clone(&system);
        async move { dispatch_with_system(&system, &action, params).await }
    })
    .await
}

async fn local_log_system(config: &LabConfig) -> Result<Arc<LogSystem>> {
    Ok(bootstrap_store_backed_log_system(
        resolve_store_path(Some(config)),
        resolve_retention(Some(config)),
    )
    .await?)
}

fn build_search_query(args: LocalSearchArgs) -> LogQuery {
    // Merge positional shorthand into --text; positional_query conflicts_with text so only one is set.
    let text = args.text.or(args.positional_query);
    LogQuery {
        text,
        after_ts: args.after_ts,
        before_ts: args.before_ts,
        levels: args.levels,
        subsystems: args.subsystems,
        surfaces: args.surfaces,
        action: args.action,
        request_id: args.request_id,
        session_id: args.session_id,
        correlation_id: args.correlation_id,
        source_node_ids: vec![],
        source_kinds: vec![],
        actor_key: None,
        limit: args.limit,
    }
}

/// Parse clap `ForwardArgs` into a `ForwardConfig` and delegate to the
/// shared dispatch-layer implementation. The CLI owns only arg parsing here.
async fn run_forward(args: ForwardArgs, _config: &LabConfig) -> Result<ExitCode> {
    // clap's `env = "..."` populates Some("") when the var is set to an empty
    // string, bypassing env_non_empty's empty-string filter. Filter here so
    // the or_else fallback fires correctly.
    let master_url = args
        .master_url
        .filter(|v| !v.is_empty())
        .or_else(|| env_non_empty("LAB_MASTER_URL"))
        .context("LAB_MASTER_URL is not set; pass --master-url or set the env var")?;

    let token = args
        .master_token
        .filter(|v| !v.is_empty())
        .or_else(|| env_non_empty("LAB_MASTER_TOKEN"))
        .or_else(|| env_non_empty("LAB_MCP_HTTP_TOKEN"));

    let node_id = resolve_node_id(args.node_id.filter(|v| !v.is_empty()));

    crate::dispatch::logs::forward::run(ForwardConfig {
        master_url,
        token,
        node_id,
        batch_size: args.batch_size,
        syslog_only: args.syslog_only,
    })
    .await
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use clap::Parser;

    use crate::cli::{Cli, Command};

    #[test]
    fn logs_cli_parser_accepts_existing_fleet_search() {
        Cli::command().debug_assert();
        assert!(Cli::try_parse_from(["lab", "logs", "search", "node-a", "timeout"]).is_ok());
    }

    #[test]
    fn logs_cli_parses_local_search() {
        let cli = Cli::try_parse_from([
            "lab",
            "logs",
            "local",
            "search",
            "--subsystem",
            "gateway",
            "--level",
            "warn",
        ])
        .expect("local search parses");

        assert!(matches!(cli.command, Command::Logs(_)));
    }

    #[test]
    fn logs_cli_rejects_invalid_local_search_filters() {
        assert!(
            Cli::try_parse_from(["lab", "logs", "local", "search", "--level", "warning",]).is_err()
        );
    }
}
