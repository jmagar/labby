//! Deploy runner construction — service instantiation per the mandatory
//! `dispatch/<service>/client.rs` layout.
//!
//! This module owns env lookup, config loading, SSH inventory loading, and
//! runner construction. CLI and MCP surfaces call these helpers instead of
//! reaching into `runner.rs` directly.

use super::runner::DefaultRunner;

/// Build a `DefaultRunner` from the supplied deploy config and the
/// `~/.ssh/config` inventory.
///
/// Failures loading the SSH inventory are treated as non-fatal — they
/// produce an empty inventory (useful so `config.list` still works) and
/// emit a tracing warning. Both CLI and MCP surfaces call this at dispatch
/// time rather than at startup, keeping construction surface-neutral.
pub fn build_runner(config: crate::config::DeployPreferences) -> DefaultRunner {
    super::runner::build_default_runner(config)
}

/// Return a reference to the process-global `DefaultRunner`, initialising
/// it from on-disk config and `~/.ssh/config` on first call.
///
/// Config load failures are non-fatal: the runner is built with default
/// preferences so that `help` / `schema` / `config.list` still work.
/// Only the MCP path uses this static — CLI dispatch owns its runner
/// directly (config is threaded in from `cli.rs`).
///
/// **Restart required to pick up config changes.** The underlying
/// `MCP_RUNNER` is a `OnceLock` initialised on first call. Changes to
/// `~/.ssh/config` or deploy preferences are not reflected until the
/// `lab` process is restarted.
pub fn static_runner() -> &'static DefaultRunner {
    super::runner::mcp_runner()
}
