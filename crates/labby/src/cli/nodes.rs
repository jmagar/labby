use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::config::LabConfig;
use crate::node::master_client::MasterClient;
use crate::output::{OutputFormat, print};

#[derive(Debug, Args)]
pub struct NodesArgs {
    #[command(subcommand)]
    pub command: NodesCommand,
}

#[derive(Debug, Subcommand)]
pub enum NodesCommand {
    /// List all registered nodes visible from the controller.
    List,
    /// Get details for a specific node by `node_id`.
    Get {
        /// Node ID to retrieve.
        #[arg(value_name = "NODE_ID")]
        node_id: String,
    },
    /// Build and roll out the local release binary to selected nodes.
    #[cfg(feature = "deploy")]
    Update(UpdateArgs),
    /// Manage pending, approved, and denied node enrollments.
    Enrollments(EnrollmentArgs),
}

#[cfg(feature = "deploy")]
#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Update every configured node and, when running on the controller, run the local controller last.
    #[arg(long)]
    pub all: bool,
    /// Explicit node targets to update.
    pub targets: Vec<String>,
}

#[derive(Debug, Args)]
pub struct EnrollmentArgs {
    #[command(subcommand)]
    pub command: EnrollmentCommand,
}

#[derive(Debug, Subcommand)]
pub enum EnrollmentCommand {
    /// List pending, approved, and denied enrollments.
    List,
    /// Approve a pending enrollment.
    Approve {
        node_id: String,
        #[arg(long)]
        note: Option<String>,
    },
    /// Deny a pending or approved enrollment.
    Deny {
        node_id: String,
        #[arg(long)]
        reason: Option<String>,
    },
}

pub async fn run(args: NodesArgs, format: OutputFormat, config: &LabConfig) -> Result<ExitCode> {
    match args.command {
        NodesCommand::List => {
            print(&fetch_nodes(config).await?, format)?;
        }
        NodesCommand::Get { node_id } => {
            print(&fetch_node(config, &node_id).await?, format)?;
        }
        #[cfg(feature = "deploy")]
        NodesCommand::Update(args) => {
            if !args.all && args.targets.is_empty() {
                anyhow::bail!("nodes update requires one or more targets or `--all`");
            }
            print(
                &crate::node::update::run_update(config, args.targets, args.all).await?,
                format,
            )?;
        }
        NodesCommand::Enrollments(args) => match args.command {
            EnrollmentCommand::List => {
                print(&fetch_enrollments(config).await?, format)?;
            }
            EnrollmentCommand::Approve { node_id, note } => {
                print(
                    &approve_enrollment(config, &node_id, note.as_deref()).await?,
                    format,
                )?;
            }
            EnrollmentCommand::Deny { node_id, reason } => {
                print(
                    &deny_enrollment(config, &node_id, reason.as_deref()).await?,
                    format,
                )?;
            }
        },
    }
    Ok(ExitCode::SUCCESS)
}

pub async fn fetch_nodes(config: &LabConfig) -> Result<serde_json::Value> {
    MasterClient::from_config(config, None)?
        .fetch_devices()
        .await
}

pub async fn fetch_node(config: &LabConfig, node_id: &str) -> Result<serde_json::Value> {
    MasterClient::from_config(config, None)?
        .fetch_device(node_id)
        .await
}

pub async fn fetch_enrollments(config: &LabConfig) -> Result<serde_json::Value> {
    MasterClient::from_config(config, None)?
        .fetch_enrollments()
        .await
}

pub async fn approve_enrollment(
    config: &LabConfig,
    node_id: &str,
    note: Option<&str>,
) -> Result<serde_json::Value> {
    MasterClient::from_config(config, None)?
        .approve_enrollment(node_id, note)
        .await
}

pub async fn deny_enrollment(
    config: &LabConfig,
    node_id: &str,
    reason: Option<&str>,
) -> Result<serde_json::Value> {
    MasterClient::from_config(config, None)?
        .deny_enrollment(node_id, reason)
        .await
}
