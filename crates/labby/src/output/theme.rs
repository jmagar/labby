use std::io::IsTerminal;

use clap::ValueEnum;

// ---------------------------------------------------------------------------
// Aurora palette — single source of truth for the CLI.
//
// Each entry pairs an Aurora dark-mode design token (see
// `aurora-design-system/registry/aurora/styles/aurora.css` →
// `--aurora-*`) with its exact truecolor RGB and the closest ANSI-256 index.
// `CliTheme` emits the truecolor triple on 24-bit terminals and falls back to
// the ANSI-256 index otherwise; the log formatter (console crate, ANSI-256
// only) consumes the same `aurora::*` indices so both surfaces stay in sync.
// ---------------------------------------------------------------------------
pub mod aurora {
    // Text
    pub const TEXT_PRIMARY: u8 = 255; // --aurora-text-primary  #e6f4fb (230,244,251)
    pub const TEXT_MUTED: u8 = 250; // --aurora-text-muted    #a7bcc9 (167,188,201)
    // Cyan accent (primary)
    pub const ACCENT_PRIMARY: u8 = 39; // --aurora-accent-primary #29b6f6 (41,182,246)
    pub const ACCENT_STRONG: u8 = 81; // --aurora-accent-strong  #67cbfa (103,203,250)
    // Rose accent (secondary)
    pub const SERVICE_NAME: u8 = 217; // --aurora-accent-pink    #f9a8c4 (249,168,196)
    // Violet accent (AI / automation identity)
    pub const VIOLET: u8 = 141; // --aurora-accent-violet  #a78bfa (167,139,250)
    // Borders
    pub const BORDER: u8 = 239; // --aurora-border-default #1d3d4e (29,61,78)
    // Status families — muted, never neon
    pub const INFO: u8 = 117; // --aurora-info          #72c8f5 (114,200,245)
    pub const SUCCESS: u8 = 115; // --aurora-success       #7dd3c7 (125,211,199)
    pub const WARN: u8 = 180; // --aurora-warn          #c6a36b (198,163,107)
    pub const ERROR: u8 = 174; // --aurora-error         #c78490 (199,132,144)
    pub const NEUTRAL: u8 = 109; // --aurora-neutral       #91a8b6 (145,168,182)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum ColorPolicy {
    #[default]
    Auto,
    Plain,
    Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorLevel {
    Plain,
    Ansi256,
    TrueColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolMode {
    Unicode,
    Ascii,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderEnv {
    pub stream_is_tty: bool,
    pub no_color: bool,
    pub term: Option<String>,
    pub colorterm: Option<String>,
    pub lang: Option<String>,
    pub lab_symbols: Option<String>,
}

impl RenderEnv {
    #[must_use]
    pub fn stdout() -> Self {
        Self::for_tty(std::io::stdout().is_terminal())
    }

    #[must_use]
    pub fn stderr() -> Self {
        Self::for_tty(std::io::stderr().is_terminal())
    }

    fn for_tty(stream_is_tty: bool) -> Self {
        Self {
            stream_is_tty,
            no_color: std::env::var_os("NO_COLOR").is_some(),
            term: std::env::var("TERM").ok(),
            colorterm: std::env::var("COLORTERM").ok(),
            lang: std::env::var("LC_ALL")
                .ok()
                .or_else(|| std::env::var("LANG").ok()),
            lab_symbols: std::env::var("LAB_SYMBOLS").ok(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderContext {
    pub level: ColorLevel,
    pub symbols: SymbolMode,
}

impl RenderContext {
    #[must_use]
    pub fn from_policy(policy: ColorPolicy, env: RenderEnv) -> Self {
        Self {
            level: detect_color_level(policy, &env),
            symbols: detect_symbol_mode(&env),
        }
    }

    #[must_use]
    pub const fn styled(self) -> bool {
        !matches!(self.level, ColorLevel::Plain)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputFormat {
    kind: OutputKind,
    ctx: RenderContext,
}

impl OutputFormat {
    #[must_use]
    pub fn from_json_flag(json: bool, policy: ColorPolicy, env: RenderEnv) -> Self {
        let kind = if json {
            OutputKind::Json
        } else {
            OutputKind::Human
        };
        Self {
            kind,
            ctx: if json {
                RenderContext {
                    level: ColorLevel::Plain,
                    symbols: detect_symbol_mode(&env),
                }
            } else {
                RenderContext::from_policy(policy, env)
            },
        }
    }

    #[must_use]
    pub const fn kind(self) -> OutputKind {
        self.kind
    }

    #[must_use]
    pub const fn is_json(self) -> bool {
        matches!(self.kind, OutputKind::Json)
    }

    #[must_use]
    #[allow(dead_code)]
    pub const fn is_human(self) -> bool {
        matches!(self.kind, OutputKind::Human)
    }

    #[must_use]
    pub const fn render_context(self) -> RenderContext {
        self.ctx
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CliTheme {
    pub(crate) ctx: RenderContext,
}

impl CliTheme {
    #[must_use]
    pub const fn from_context(ctx: RenderContext) -> Self {
        Self { ctx }
    }

    #[must_use]
    pub const fn context(self) -> RenderContext {
        self.ctx
    }

    #[must_use]
    pub fn heading(self, text: &str) -> String {
        format!(
            "{}\n{}",
            self.display(text),
            self.border(
                &self
                    .symbol(self.symbols().divider)
                    .repeat(text.chars().count().max(12))
            )
        )
    }

    #[must_use]
    pub fn display(self, text: &str) -> String {
        self.bold(self.accent(text))
    }

    #[must_use]
    pub fn section(self, text: &str) -> String {
        self.bold(self.primary(text))
    }

    #[must_use]
    pub fn key(self, text: &str) -> String {
        self.section(text)
    }

    #[must_use]
    pub fn value(self, text: &str) -> String {
        self.accent(text)
    }

    /// `--aurora-accent-primary` (cyan) — primary CTAs, selection, focus.
    #[must_use]
    pub fn accent(self, text: &str) -> String {
        paint((41, 182, 246), aurora::ACCENT_PRIMARY, text, self.ctx)
    }

    /// `--aurora-accent-pink` (rose) — secondary/agent affordances, key labels.
    #[must_use]
    pub fn service_name(self, text: &str) -> String {
        paint((249, 168, 196), aurora::SERVICE_NAME, text, self.ctx)
    }

    /// `--aurora-accent-violet` — AI / automation identity.
    #[must_use]
    #[allow(dead_code)]
    pub fn violet(self, text: &str) -> String {
        paint((167, 139, 250), aurora::VIOLET, text, self.ctx)
    }

    /// `--aurora-text-primary` — headings, body, control labels.
    #[must_use]
    pub fn primary(self, text: &str) -> String {
        paint((230, 244, 251), aurora::TEXT_PRIMARY, text, self.ctx)
    }

    /// `--aurora-text-muted` — captions, meta, descriptions.
    #[must_use]
    pub fn secondary(self, text: &str) -> String {
        paint((167, 188, 201), aurora::TEXT_MUTED, text, self.ctx)
    }

    /// `--aurora-accent-strong` — hover/active cyan emphasis.
    #[must_use]
    pub fn tertiary(self, text: &str) -> String {
        paint((103, 203, 250), aurora::ACCENT_STRONG, text, self.ctx)
    }

    /// `--aurora-border-default` — resting separators, dividers, table rules.
    #[must_use]
    pub fn border(self, text: &str) -> String {
        paint((29, 61, 78), aurora::BORDER, text, self.ctx)
    }

    /// `--aurora-text-muted` — captions, meta, descriptions, placeholders.
    #[must_use]
    pub fn muted<T: AsRef<str>>(self, text: T) -> String {
        paint((167, 188, 201), aurora::TEXT_MUTED, text.as_ref(), self.ctx)
    }

    /// `--aurora-info` — informational status (muted cyan).
    #[must_use]
    #[allow(dead_code)]
    pub fn info(self, text: &str) -> String {
        paint((114, 200, 245), aurora::INFO, text, self.ctx)
    }

    /// `--aurora-success` — success status (muted teal-mint).
    #[must_use]
    pub fn success(self, text: &str) -> String {
        paint((125, 211, 199), aurora::SUCCESS, text, self.ctx)
    }

    /// `--aurora-warn` — warning status (warm sand).
    #[must_use]
    pub fn warn(self, text: &str) -> String {
        paint((198, 163, 107), aurora::WARN, text, self.ctx)
    }

    /// `--aurora-error` — error status (rose-clay).
    #[must_use]
    pub fn error(self, text: &str) -> String {
        paint((199, 132, 144), aurora::ERROR, text, self.ctx)
    }

    /// `--aurora-neutral` — neutral/idle status (slate).
    #[must_use]
    #[allow(dead_code)]
    pub fn neutral(self, text: &str) -> String {
        paint((145, 168, 182), aurora::NEUTRAL, text, self.ctx)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn panel(self, text: &str) -> String {
        paint_bg((16, 35, 48), 235, text, self.ctx)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn panel_strong(self, text: &str) -> String {
        paint_bg((19, 41, 58), 236, text, self.ctx)
    }

    #[must_use]
    pub fn ok_badge(self) -> String {
        self.success(self.symbol(self.symbols().ok))
    }

    #[must_use]
    pub fn warn_badge(self) -> String {
        self.warn(self.symbol(self.symbols().warn))
    }

    #[must_use]
    pub fn error_badge(self) -> String {
        self.error(self.symbol(self.symbols().error))
    }

    #[must_use]
    pub fn bool_icon(self, value: bool) -> String {
        if value {
            self.ok_badge()
        } else {
            self.error_badge()
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn bullet(self) -> &'static str {
        self.symbol(self.symbols().bullet)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn disclosure(self) -> &'static str {
        self.symbol(self.symbols().disclosure)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn dot(self) -> &'static str {
        self.symbol(self.symbols().dot)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn divider(self) -> &'static str {
        self.symbol(self.symbols().divider)
    }

    #[must_use]
    pub fn bold(self, text: String) -> String {
        if self.ctx.styled() {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text
        }
    }

    const fn symbols(self) -> Symbols {
        match self.ctx.symbols {
            SymbolMode::Unicode => Symbols::UNICODE,
            SymbolMode::Ascii => Symbols::ASCII,
        }
    }

    const fn symbol(self, value: &'static str) -> &'static str {
        value
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct Symbols {
    bullet: &'static str,
    disclosure: &'static str,
    dot: &'static str,
    divider: &'static str,
    ok: &'static str,
    warn: &'static str,
    error: &'static str,
}

impl Symbols {
    const UNICODE: Self = Self {
        bullet: "•",
        disclosure: "▸",
        dot: "·",
        divider: "─",
        ok: "✓",
        warn: "⚠",
        error: "✗",
    };

    const ASCII: Self = Self {
        bullet: "*",
        disclosure: ">",
        dot: ".",
        divider: "-",
        ok: "ok",
        warn: "!",
        error: "x",
    };
}

#[must_use]
pub fn human_output_styling_enabled(policy: ColorPolicy, env: RenderEnv) -> bool {
    RenderContext::from_policy(policy, env).styled()
}

fn detect_color_level(policy: ColorPolicy, env: &RenderEnv) -> ColorLevel {
    if matches!(policy, ColorPolicy::Plain) {
        return ColorLevel::Plain;
    }

    if matches!(policy, ColorPolicy::Auto) && (!env.stream_is_tty || env.no_color) {
        return ColorLevel::Plain;
    }

    let colorterm = env
        .colorterm
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let term = env.term.as_deref().unwrap_or_default().to_ascii_lowercase();

    if colorterm.contains("truecolor")
        || colorterm.contains("24bit")
        || term.contains("truecolor")
        || term.contains("24bit")
        || term.contains("direct")
    {
        ColorLevel::TrueColor
    } else {
        ColorLevel::Ansi256
    }
}

fn detect_symbol_mode(env: &RenderEnv) -> SymbolMode {
    match env
        .lab_symbols
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "ascii" => return SymbolMode::Ascii,
        "unicode" => return SymbolMode::Unicode,
        _ => {}
    }

    let term = env.term.as_deref().unwrap_or_default().to_ascii_lowercase();
    if term == "dumb" {
        return SymbolMode::Ascii;
    }

    let lang = env.lang.as_deref().unwrap_or_default().to_ascii_lowercase();
    if lang == "c" || lang == "posix" {
        return SymbolMode::Ascii;
    }

    SymbolMode::Unicode
}

fn paint(rgb: (u8, u8, u8), ansi256: u8, text: &str, ctx: RenderContext) -> String {
    match ctx.level {
        ColorLevel::Plain => text.to_string(),
        ColorLevel::Ansi256 => format!("\x1b[38;5;{ansi256}m{text}\x1b[0m"),
        ColorLevel::TrueColor => {
            let (r, g, b) = rgb;
            format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
        }
    }
}

#[allow(dead_code)]
fn paint_bg(rgb: (u8, u8, u8), ansi256: u8, text: &str, ctx: RenderContext) -> String {
    match ctx.level {
        ColorLevel::Plain => text.to_string(),
        ColorLevel::Ansi256 => format!("\x1b[48;5;{ansi256}m{text}\x1b[0m"),
        ColorLevel::TrueColor => {
            let (r, g, b) = rgb;
            format!("\x1b[48;2;{r};{g};{b}m{text}\x1b[0m")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(
        stream_is_tty: bool,
        no_color: bool,
        term: Option<&str>,
        colorterm: Option<&str>,
    ) -> RenderEnv {
        RenderEnv {
            stream_is_tty,
            no_color,
            term: term.map(str::to_string),
            colorterm: colorterm.map(str::to_string),
            lang: Some("en_US.UTF-8".to_string()),
            lab_symbols: None,
        }
    }

    #[test]
    fn auto_tty_without_no_color_prefers_truecolor_when_available() {
        let ctx = RenderContext::from_policy(
            ColorPolicy::Auto,
            env(true, false, Some("xterm-256color"), Some("truecolor")),
        );
        assert_eq!(ctx.level, ColorLevel::TrueColor);
    }

    #[test]
    fn auto_non_tty_disables_color() {
        let ctx = RenderContext::from_policy(
            ColorPolicy::Auto,
            env(false, false, Some("xterm-256color"), Some("truecolor")),
        );
        assert_eq!(ctx.level, ColorLevel::Plain);
    }

    #[test]
    fn auto_with_no_color_disables_color() {
        let ctx = RenderContext::from_policy(
            ColorPolicy::Auto,
            env(true, true, Some("xterm-256color"), Some("truecolor")),
        );
        assert_eq!(ctx.level, ColorLevel::Plain);
    }

    #[test]
    fn forced_color_ignores_no_color_and_non_tty() {
        let ctx = RenderContext::from_policy(
            ColorPolicy::Color,
            env(false, true, Some("xterm-256color"), Some("truecolor")),
        );
        assert_eq!(ctx.level, ColorLevel::TrueColor);
    }

    #[test]
    fn plain_forces_unstyled_output() {
        let ctx = RenderContext::from_policy(
            ColorPolicy::Plain,
            env(true, false, Some("xterm-256color"), Some("truecolor")),
        );
        assert_eq!(ctx.level, ColorLevel::Plain);
    }

    fn theme_at(level: ColorLevel) -> CliTheme {
        CliTheme::from_context(RenderContext {
            level,
            symbols: SymbolMode::Unicode,
        })
    }

    #[test]
    fn accents_emit_aurora_truecolor_triples() {
        // (method output, expected dark-mode Aurora RGB)
        let t = theme_at(ColorLevel::TrueColor);
        let cases = [
            (t.accent("x"), "38;2;41;182;246"), // accent-primary #29b6f6
            (t.service_name("x"), "38;2;249;168;196"), // accent-pink    #f9a8c4
            (t.violet("x"), "38;2;167;139;250"), // accent-violet  #a78bfa
            (t.info("x"), "38;2;114;200;245"),  // info           #72c8f5
            (t.success("x"), "38;2;125;211;199"), // success        #7dd3c7
            (t.warn("x"), "38;2;198;163;107"),  // warn           #c6a36b
            (t.error("x"), "38;2;199;132;144"), // error          #c78490
            (t.neutral("x"), "38;2;145;168;182"), // neutral        #91a8b6
        ];
        for (rendered, escape) in cases {
            assert!(
                rendered.contains(escape),
                "expected truecolor escape {escape}, got: {rendered:?}"
            );
        }
    }

    #[test]
    fn accents_emit_aurora_ansi256_indices() {
        let t = theme_at(ColorLevel::Ansi256);
        let cases = [
            (t.service_name("x"), aurora::SERVICE_NAME),
            (t.violet("x"), aurora::VIOLET),
            (t.info("x"), aurora::INFO),
            (t.neutral("x"), aurora::NEUTRAL),
        ];
        for (rendered, idx) in cases {
            assert!(
                rendered.contains(&format!("38;5;{idx}")),
                "expected ansi256 index {idx}, got: {rendered:?}"
            );
        }
    }

    #[test]
    fn plain_level_strips_all_accent_escapes() {
        let t = theme_at(ColorLevel::Plain);
        for rendered in [
            t.violet("x"),
            t.info("x"),
            t.neutral("x"),
            t.service_name("x"),
        ] {
            assert_eq!(rendered, "x", "plain mode must not emit escapes");
        }
    }

    #[test]
    fn symbol_mode_falls_back_to_ascii_for_dumb_term() {
        let ctx = RenderContext::from_policy(
            ColorPolicy::Auto,
            RenderEnv {
                stream_is_tty: true,
                no_color: false,
                term: Some("dumb".to_string()),
                colorterm: None,
                lang: Some("C".to_string()),
                lab_symbols: None,
            },
        );
        assert_eq!(ctx.symbols, SymbolMode::Ascii);
    }
}
