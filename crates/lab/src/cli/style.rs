//! Aurora-themed clap help/error styling.
//!
//! A single `AURORA_STYLES` applied at the root `#[command(...)]` propagates
//! to every subcommand, so `labby gateway --help`, `labby gateway help`, and
//! deeper subcommands all render with the Aurora palette. The ANSI-256 indices
//! come from `crate::output::theme::aurora` so clap help stays in sync with the
//! catalog and table renderers.
//!
//! Note: clap only emits these escapes when its `color` feature is enabled AND
//! the resolved `clap::ColorChoice` is not `Never`. `crates/lab/src/main.rs`
//! maps the resolved `ColorPolicy` onto `ColorChoice`, so `--color plain` /
//! `NO_COLOR` strip the styling. `render_long_help().to_string()` (used by the
//! docs generator) stays plain regardless, keeping `cli-help.md` ANSI-free.

use clap::builder::styling::{Ansi256Color, Styles};

use crate::output::theme::aurora;

/// Aurora-themed styling for clap help and error output.
pub const AURORA_STYLES: Styles = Styles::styled()
    // Section headers (`Usage:`, `Options:`, `Commands:`) and the usage line —
    // primary cyan, bold.
    .header(Ansi256Color(aurora::ACCENT_PRIMARY).on_default().bold())
    .usage(Ansi256Color(aurora::ACCENT_PRIMARY).on_default().bold())
    // Literals: subcommand names and flag spellings — strong cyan.
    .literal(Ansi256Color(aurora::ACCENT_STRONG).on_default())
    // Placeholders: `<COMMAND>`, value names — violet (AI/automation identity).
    .placeholder(Ansi256Color(aurora::VIOLET).on_default())
    // Validation/error accents — muted status triad.
    .valid(Ansi256Color(aurora::SUCCESS).on_default())
    .invalid(Ansi256Color(aurora::WARN).on_default())
    .error(Ansi256Color(aurora::ERROR).on_default());
