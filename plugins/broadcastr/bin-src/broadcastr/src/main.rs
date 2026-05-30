mod bus;
mod config;
mod emit;
mod format;
mod monitor;
mod recent;
mod status;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "broadcastr", about = "Shared activity bus for concurrent Claude sessions")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write an event to the bus
    Emit {
        category: String,
        tier: String,
        summary: String,
        #[arg(long)]
        data: Option<String>,
        #[arg(long, default_value = "cli")]
        source: String,
        #[arg(long)]
        branch: Option<String>,
    },
    /// Run all monitors in one process: bus feed display + fs watchers + alert gateway
    Monitor,
    /// Tail and display the bus feed (no fs watchers or alert gateway)
    Tail,
    /// Dump recent events as JSONL
    Recent {
        #[arg(long, default_value = "5m")]
        since: String,
    },
    /// Show bus paths, sizes, and event counts
    Status,
}

fn main() {
    let cli = Cli::parse();
    let cfg = config::Config::from_env();

    // Emit still runs when BROADCASTR_DISABLED=1 so hooks don't blow up;
    // they just no-op inside emit::run.
    let result = match cli.cmd {
        Cmd::Emit { category, tier, summary, data, source, branch } =>
            emit::run(&cfg, &category, &tier, &summary,
                      data.as_deref(), &source, branch.as_deref(),
                      true /* use_session_id */),
        Cmd::Monitor  => monitor::run(&cfg),
        Cmd::Tail     => monitor::run_feed(&cfg),
        Cmd::Recent { since } => recent::run(&cfg, &since),
        Cmd::Status   => status::run(&cfg),
    };

    if let Err(e) = result {
        eprintln!("broadcastr: {e}");
        std::process::exit(1);
    }
}
