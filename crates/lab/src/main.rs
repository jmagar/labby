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
#[cfg(feature = "fs")]
mod workspace;

use std::ffi::OsStr;
use std::process::ExitCode;

use clap::{ColorChoice, CommandFactory, FromArgMatches};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, filter::filter_fn, fmt, prelude::*};

use crate::cli::Cli;
use crate::dispatch::logs::ingest::LogIngestLayer;
use crate::log_fmt::formatter::PremiumEventFormatter;
use crate::output::{ColorPolicy, OutputFormat, RenderEnv, human_output_styling_enabled};

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

/// Global flags that may appear *before* a subcommand and must be skipped when
/// the pre-parse shim scans for the root `help` / `-h` / `--help` tokens.
///
/// These mirror the `#[arg(global = true)]` flags on [`Cli`]. **If you add a new
/// global flag to `Cli`, add it here too** — otherwise the catalog shim will
/// mistake its value for a subcommand. A missed flag degrades to clap's own
/// help (it never crashes), but the root catalog would stop firing.
mod global_flags {
    /// Boolean global flags (no value follows).
    pub const BOOLEAN: &[&str] = &["--json"];
    /// Value-taking global flags in `--flag VALUE` form. The `--flag=VALUE`
    /// form is handled separately by prefix match.
    pub const VALUED: &[&str] = &["--color"];
}

/// A global flag's captured value, used by the catalog shim.
#[derive(Default)]
struct GlobalFlags {
    json: bool,
    color: Option<ColorPolicy>,
}

/// Parse a `--color` value string (`auto`/`plain`/`color`) into a [`ColorPolicy`].
///
/// Styling is cosmetic, so an unrecognized value falls back to `Auto` rather
/// than erroring — the real validation happens later in clap's parse pass.
fn parse_color_value(value: &str) -> ColorPolicy {
    match value.to_ascii_lowercase().as_str() {
        "plain" => ColorPolicy::Plain,
        "color" => ColorPolicy::Color,
        _ => ColorPolicy::Auto,
    }
}

/// Resolve the effective color policy.
///
/// The CLI `--color` flag wins when set explicitly; when it is `Auto`, the
/// `LAB_LOG_COLOR` env var can force or disable color (e.g. inside Docker where
/// there is no TTY). This is the single source of truth shared by the catalog
/// shim, the clap parser's `ColorChoice`, and `init_tracing` so help color and
/// log color never drift.
fn resolve_color_policy(cli_color: ColorPolicy) -> ColorPolicy {
    if cli_color == ColorPolicy::Auto {
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
        cli_color
    }
}

/// Map a resolved [`ColorPolicy`] onto clap's [`ColorChoice`] so themed clap
/// help obeys `--color` / `NO_COLOR` / `LAB_LOG_COLOR`. `Auto` defers to clap's
/// own TTY + `NO_COLOR` detection.
const fn color_choice_for(policy: ColorPolicy) -> ColorChoice {
    match policy {
        ColorPolicy::Plain => ColorChoice::Never,
        ColorPolicy::Color => ColorChoice::Always,
        ColorPolicy::Auto => ColorChoice::Auto,
    }
}

/// If the invocation is a *root-level* help request, return the captured global
/// flags so the caller can render the Aurora catalog instead of clap help.
///
/// Returns `Some` only when, after skipping leading global flags, the first
/// positional token is:
/// - bare `help`, optionally followed by any mix of global flags (`--json`,
///   `--color`) and `--all`, or
/// - `-h` / `--help` at the root (no preceding subcommand).
///
/// Global flags are accepted on *both* sides of the trigger because they are
/// `global = true` on [`Cli`] — `labby help --json` and `labby --json help` must
/// both reach the catalog (scripts consume `help --json`). Their values are
/// folded into the returned flags regardless of position.
///
/// `help <subcommand>` (e.g. `help gateway`) returns `None` and falls through to
/// clap's native, now-themed `help` subcommand.
fn root_help_request<I, T>(args: I) -> Option<GlobalFlags>
where
    I: IntoIterator<Item = T>,
    T: AsRef<OsStr>,
{
    let mut flags = GlobalFlags::default();
    let mut iter = args.into_iter();
    // Skip argv[0] (program name).
    iter.next();

    let mut iter = iter.peekable();
    while let Some(arg) = iter.peek() {
        let arg = arg.as_ref().to_string_lossy().into_owned();
        if global_flags::BOOLEAN.contains(&arg.as_str()) {
            if arg == "--json" {
                flags.json = true;
            }
            iter.next();
        } else if let Some(rest) = arg.strip_prefix("--color=") {
            flags.color = Some(parse_color_value(rest));
            iter.next();
        } else if global_flags::VALUED.contains(&arg.as_str()) {
            // `--color VALUE` — consume the flag, then its value (if present).
            iter.next();
            if let Some(value) = iter.next() {
                if arg == "--color" {
                    flags.color = Some(parse_color_value(&value.as_ref().to_string_lossy()));
                }
            }
        } else {
            break;
        }
    }

    // First non-global token must be the help trigger.
    let first = iter.next()?;
    let first = first.as_ref().to_string_lossy().into_owned();
    match first.as_str() {
        "-h" | "--help" => {
            // Root `-h`/`--help` with no preceding subcommand → catalog. Fold any
            // trailing global flags (`-h --json`) into the captured flags; a
            // foreign token after a terminal help flag is ignored, mirroring
            // clap's treatment of `--help` as short-circuiting.
            trailing_globals_only(&mut iter, &mut flags);
            Some(flags)
        }
        "help" => {
            // Bare `help` plus any trailing global flags / `--all` is the root
            // catalog; `help <subcommand>` (a foreign trailing token) falls
            // through to clap's native help subcommand.
            if trailing_globals_only(&mut iter, &mut flags) {
                Some(flags)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Consume the tokens trailing a root help trigger, folding recognized global
/// flag values (`--json`, `--color VALUE`, `--color=VALUE`) into `flags` and
/// tolerating `--all`. Returns `true` if every remaining token was a global
/// flag or `--all`, or `false` on the first foreign token (e.g. a subcommand
/// name) — which marks the invocation as `help <subcommand>`.
fn trailing_globals_only<I, T>(iter: &mut I, flags: &mut GlobalFlags) -> bool
where
    I: Iterator<Item = T>,
    T: AsRef<OsStr>,
{
    while let Some(arg) = iter.next() {
        let arg = arg.as_ref().to_string_lossy().into_owned();
        if arg == "--all" || global_flags::BOOLEAN.contains(&arg.as_str()) {
            if arg == "--json" {
                flags.json = true;
            }
        } else if let Some(rest) = arg.strip_prefix("--color=") {
            flags.color = Some(parse_color_value(rest));
        } else if global_flags::VALUED.contains(&arg.as_str()) {
            // `--color VALUE` — the value (if present) is the next token.
            if let Some(value) = iter.next() {
                if arg == "--color" {
                    flags.color = Some(parse_color_value(&value.as_ref().to_string_lossy()));
                }
            }
        } else {
            return false;
        }
    }
    true
}

/// Whether the (already-skipped) help invocation requested `--all`.
fn help_wants_all<I, T>(args: I) -> bool
where
    I: IntoIterator<Item = T>,
    T: AsRef<OsStr>,
{
    args.into_iter()
        .any(|a| a.as_ref().to_string_lossy() == "--all")
}

/// Scan the whole argv for a `--color` value (the flag is `global = true`, so
/// it may appear before or after the subcommand). Returns the last occurrence's
/// policy, or `None` if `--color` is absent. Used only to pick clap's
/// `ColorChoice`; clap itself still performs full validation afterwards.
fn scan_color_flag<I, T>(args: I) -> Option<ColorPolicy>
where
    I: IntoIterator<Item = T>,
    T: AsRef<OsStr>,
{
    let mut found = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let arg = arg.as_ref().to_string_lossy().into_owned();
        if let Some(rest) = arg.strip_prefix("--color=") {
            found = Some(parse_color_value(rest));
        } else if arg == "--color" {
            if let Some(value) = iter.next() {
                found = Some(parse_color_value(&value.as_ref().to_string_lossy()));
            }
        }
    }
    found
}

/// Render the Aurora service + action catalog for the root help path.
fn run_root_catalog(flags: &GlobalFlags) -> ExitCode {
    // The env-filtered catalog needs config + .env (unlike the metadata-only
    // Docs fast-path). Failures are non-fatal — fall back to defaults.
    config::load_dotenv().ok();
    let policy = resolve_color_policy(flags.color.unwrap_or_default());
    let format = OutputFormat::from_json_flag(flags.json, policy, RenderEnv::stdout());
    let all = help_wants_all(std::env::args_os());
    match cli::help::run(cli::help::HelpArgs { all }, format) {
        Ok(code) => code,
        Err(err) => {
            // This runs before `init_tracing`, so `tracing::error!` would have no
            // subscriber and the failure (e.g. a malformed config.toml) would be
            // invisible. Use stderr directly, matching the pre-tracing error path
            // in `main`. (`clippy::print_stderr` is `allow` workspace-wide.)
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    // Pre-parse shim: root `help` / `--help` / `-h` renders the Aurora catalog,
    // which clap cannot express via derive (it would auto-handle `--help` and
    // panics on a duplicate `help`). Every *non-root* help path (`gateway help`,
    // `gateway --help`, `help gateway`) falls through to clap's themed output.
    if let Some(flags) = root_help_request(std::env::args_os()) {
        return run_root_catalog(&flags);
    }

    // Build the parser with an explicit ColorChoice so themed clap help obeys
    // our `--color` policy (clap's `color` feature otherwise ignores it). We
    // scan argv for `--color` directly rather than doing a clap pre-parse: a
    // pre-parse `get_matches()` would itself auto-exit (rendering unthemed help)
    // the moment it saw `--help`, before the real themed parse could run.
    let cli = {
        let pre = scan_color_flag(std::env::args_os()).unwrap_or_default();
        let choice = color_choice_for(resolve_color_policy(pre));
        let matches = Cli::command().color(choice).get_matches();
        Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit())
    };

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
            // Silence upstream connect/discovery warnings — failures are surfaced
            // inline in command output (e.g. `gateway list`); raw events just leak
            // above the human-readable result. Set LAB_LOG=labby=warn to see them.
            Some("labby=warn,labby::dispatch::upstream=error,lab_apis=warn,rmcp=warn".to_string())
        }
        _ => None,
    };

    // LAB_LOG_COLOR overrides the CLI default when running without a TTY (e.g.
    // inside Docker). The CLI --color flag wins when the user sets it explicitly,
    // but since clap cannot distinguish "user passed --color auto" from "defaulted
    // to auto", the env var only activates when the policy is Auto. Shared with
    // the catalog shim and clap's ColorChoice so help and log color stay in sync.
    let color_policy = resolve_color_policy(cli.color);

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
