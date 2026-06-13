# CLI Output Theme API Proposal

**Status:** Active
**Scope:** `crates/lab/src/output.rs` and future CLI human-readable renderers

## Purpose

This document describes the Rust API that implements the CLI design contract in [CLI Design System Contract](./CLI_DESIGN_SYSTEM.md).

It is intentionally scoped to CLI output, not the Ratatui TUI. The TUI can later share palette names, but it should not be forced into the same renderer API.

## Goals

- centralize color and style policy
- keep command handlers presentation-free
- preserve pipeability and `NO_COLOR` behavior
- support truecolor-first rendering with ANSI-256 fallback
- degrade cleanly to plain text
- make current `output.rs` helpers (`primary`, `accent`, `status_ok`, etc.) semantic rather than ad hoc

## Non-Goals

- redesign JSON output
- introduce styling in `lab-apis`
- force Ratatui to share CLI-specific formatter types

## Current Fit

The output layer is now split into:

- [output.rs](../../crates/lab/src/output.rs) — stable public API and re-exports
- [theme.rs](../../crates/lab/src/output/theme.rs) — policy, environment detection, palette, semantic theme, symbol fallback
- [render.rs](../../crates/lab/src/output/render.rs) — human-readable rendering and JSON routing

The active design is:

- explicit `OutputFormat` context threaded from the CLI boundary
- no global mutable color policy
- truecolor/ANSI/plain resolution from `RenderEnv`
- semantic rendering through `CliTheme`

## Proposed Types

### Color policy

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorPolicy {
    #[default]
    Auto,
    Plain,
    Color,
}
```

Rules:

- `Auto` styles only when `stdout` is a TTY and `NO_COLOR` is unset
- `Plain` forces no styling
- `Color` forces styling

### Terminal color level

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorLevel {
    Plain,
    Ansi256,
    TrueColor,
}
```

Rules:

- resolve once at render start
- `Plain` is used for non-TTY `auto`, `NO_COLOR`, or explicit plain mode
- `TrueColor` is preferred when supported
- `Ansi256` is the fallback styled mode

### Render environment

```rust
#[derive(Debug, Clone, Copy)]
pub struct RenderEnv {
    pub stdout_is_tty: bool,
    pub no_color: bool,
    pub term: Option<&'static str>,
    pub colorterm: Option<&'static str>,
}
```

This can be internal if a borrowed env view is awkward; the point is to separate environment inspection from styling decisions.

### Render context

```rust
#[derive(Debug, Clone, Copy)]
pub struct RenderContext {
    pub policy: ColorPolicy,
    pub level: ColorLevel,
}
```

This replaces the current `RenderContext { color: bool }`.

## Proposed Semantic Tokens

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    TextPrimary,
    TextMuted,
    AccentPrimary,
    AccentStrong,
    AccentDeep,
    BorderDefault,
    BorderStrong,
    StateSuccess,
    StateWarn,
    StateError,
    StateInfo,
}
```

Notes:

- CLI output does not need background tokens immediately if the current renderer stays mostly foreground-only
- background tokens can be added later without changing the color policy model

## Proposed Text Roles

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRole {
    Display,
    Section,
    Body,
    Dense,
    Eyebrow,
    Metric,
    ControlLabel,
    ControlValue,
    StatusSuccess,
    StatusWarn,
    StatusError,
}
```

Use cases:

- `Display` for top-level headings
- `Section` for table titles and grouped blocks
- `ControlLabel` / `ControlValue` for key-value rows
- `Status*` for labeled status fragments

## Proposed Palette Definition

```rust
#[derive(Debug, Clone, Copy)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, Clone, Copy)]
pub struct PaletteEntry {
    pub truecolor: Rgb,
    pub ansi256: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct CliPalette {
    pub text_primary: PaletteEntry,
    pub text_muted: PaletteEntry,
    pub accent_primary: PaletteEntry,
    pub accent_strong: PaletteEntry,
    pub accent_deep: PaletteEntry,
    pub border_default: PaletteEntry,
    pub border_strong: PaletteEntry,
    pub state_success: PaletteEntry,
    pub state_warn: PaletteEntry,
    pub state_error: PaletteEntry,
    pub state_info: PaletteEntry,
}
```

Provide a single constant:

```rust
pub const AURORA_CLI: CliPalette = /* ... */;
```

## Proposed API Shape

### Theme object

```rust
pub struct CliTheme {
    ctx: RenderContext,
    palette: &'static CliPalette,
}
```

Core methods:

```rust
impl CliTheme {
    pub fn detect(policy: ColorPolicy) -> Self;

    pub fn style<'a>(&self, text: &'a str, tone: Tone) -> String;
    pub fn role<'a>(&self, text: &'a str, role: TextRole) -> String;

    pub fn bullet<'a>(&self, text: &'a str) -> String;
    pub fn heading<'a>(&self, text: &'a str) -> String;
    pub fn section<'a>(&self, text: &'a str) -> String;
    pub fn key<'a>(&self, text: &'a str) -> String;
    pub fn value<'a>(&self, text: &'a str) -> String;
    pub fn muted<'a>(&self, text: &'a str) -> String;

    pub fn status_success<'a>(&self, label: &'a str) -> String;
    pub fn status_warn<'a>(&self, label: &'a str) -> String;
    pub fn status_error<'a>(&self, label: &'a str) -> String;

    pub fn divider(&self, width: usize) -> String;
}
```

This keeps render sites semantic and small.

## Recommended Internal Mapping

Current helpers in `output.rs` should collapse into `CliTheme` methods.

Suggested mapping:

- `primary(...)` -> `theme.style(..., Tone::TextPrimary)`
- `subtle(...)` and `dim(...)` -> `theme.style(..., Tone::TextMuted)`
- `accent(...)` -> `theme.style(..., Tone::AccentPrimary)`
- `render_heading(...)` -> `theme.heading(...)`
- `status_ok(...)` -> `theme.status_success(...)`
- `status_warn(...)` -> `theme.status_warn(...)`
- `status_fail(...)` -> `theme.status_error(...)`

This eliminates the current spread of partially-overlapping helpers.

## CLI Flag Integration

The top-level CLI should expose a single shared flag:

```text
--color=auto|plain|color
```

`OutputFormat::Json` remains separate.

Recommended integration:

```rust
pub fn render<T: Serialize>(
    value: &T,
    format: OutputFormat,
    color_policy: ColorPolicy,
) -> Result<String>
```

Then:

```rust
match format {
    OutputFormat::Json => serde_json::to_string(value)?,
    OutputFormat::Human => {
        let theme = CliTheme::detect(color_policy);
        render_human(&serde_json::to_value(value)?, &theme)
    }
}
```

That is cleaner than reading environment state directly from deep helper functions.

## Capability Detection

Suggested precedence:

1. if `format == Json`, use `Plain`
2. if `policy == Plain`, use `Plain`
3. if `policy == Auto` and `stdout` is not a TTY, use `Plain`
4. if `NO_COLOR` is set and policy is not forced, use `Plain`
5. if `COLORTERM=truecolor` or `24bit`, use `TrueColor`
6. otherwise if terminal is color-capable, use `Ansi256`
7. otherwise use `Plain`

This is intentionally conservative. Pipeability and predictable automation matter more than squeezing color into every environment.

## Plain-Mode Invariants

Plain mode must preserve:

- headings
- status meaning
- table structure
- bullets and dividers where supported

Plain mode must not emit:

- ANSI escapes
- invisible control sequences
- formatting that changes grep/cut behavior unexpectedly

## Example Usage

```rust
fn render_service_health(items: &[Value], theme: &CliTheme) -> String {
    let mut out = String::new();
    out.push_str(&theme.heading("Service Health"));
    out.push('\n');
    out.push_str(&theme.muted("(14 total)"));
    out
}
```

Example output intent:

```text
Service Health
(3 total)

Status  Service  Auth  Version  Latency  Message
──────  ───────  ────  ───────  ───────  ───────
ok      gateway      yes   -        24 ms    reachable
warn    marketplace  yes   -        412 ms   registry sync delayed
fail    logs         yes   -        -        store unavailable
```

In styled mode:

- heading uses `Display` / primary emphasis
- separator uses muted border tone
- `ok/warn/fail` use state tones

In plain mode:

- the exact same text remains readable

## Migration Plan

1. Add `ColorPolicy` and thread it through the top-level CLI
2. Replace `RenderContext { color: bool }` with `CliTheme`
3. Move raw helper functions behind semantic methods
4. Convert existing renderer call sites incrementally
5. Keep JSON behavior unchanged

## Ratatui Note

The TUI should not reuse this API directly.

It should reuse:

- semantic token names
- palette values
- status meaning

It should not be forced to use:

- `String`-returning helper methods
- CLI-specific plain-text fallback logic

That split keeps CLI output and TUI rendering aligned without coupling them unnecessarily.
