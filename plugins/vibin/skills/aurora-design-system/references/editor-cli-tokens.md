# Editor & CLI tokens — terminal, shell, and Zed Neon palettes

Aurora theming extends past the web registry into editors, terminals, and CLI
tools. These live in `~/workspace/aurora-design-system/editors/` and `/shell/`
(hand-authored per-tool native formats, excluded from the Next/TS/eslint build).
They are **canonical there**; `~/.config/...`, `~/.claude/themes/`, etc. are
deployed copies kept in sync.

## ⚠️ Three palette tiers — do not cross-contaminate

The Aurora cyan/rose deliberately diverge by surface. Know which tier you're in
before copying a hex:

| Tier | Cyan primary | Rose | Background | Where |
|---|---|---|---|---|
| **Web canonical** | `#29b6f6` | `#f9a8c4` | `#07131c` | `registry/aurora/styles/aurora.css`, all React UI, Android. The source of truth — see `references/tokens.md`. |
| **CLI / Claude Code** | `#36c9ff` | `#ff7eb6` | `#07131c` | `editors/claude-code/*.json`, Claude Code statusline. Brightened so accents survive a dark terminal + low-gamut emulators. |
| **Zed Neon** | `#38d2ff` | `#ff9ec9` | `#102a3e` | `editors/zed/` only. A lifted-canvas neon variant. **Most divergent.** |
| Shell tools | `#29b6f6` | `#f9a8c4` | `#07131c` | `shell/*` (p10k, bat, mc, nano, zsh) — mirror the web canonical palette. |

**Rule:** never sync one tier's cyan/rose into another without an explicit
decision to re-base the whole system. When in doubt for web/React work, use the
canonical `--aurora-*` vars from `references/tokens.md`.

---

## Zed — "Aurora Neon" (`editors/zed/`)

A Zed extension (`id = aurora-neon`) shipping two UI themes + two icon themes:
**Aurora Neon** / **Aurora Neon Light**, **Aurora Neon Icons** / **… Light**.
Validated against `https://zed.dev/schema/themes/v0.2.0.json`. The **dark** variant
is the brightened neon palette; the **light** variant uses the canonical light hues.

### Aurora Neon — dark surfaces & chrome

| Role | Hex | Zed key |
|---|---|---|
| canvas / editor bg | `#102a3e` | `background`, `editor.background` |
| surface (panels/tabs) | `#14334a` | `surface.background` |
| elevated surface | `#1a3d56` | `elevated_surface.background` |
| hover / active line | `#26507a` | `element.hover` |
| border | `#356b8c` | `border` |
| border focused / accent | `#38d2ff` | `border.focused`, `text.accent` |
| text | `#ffffff` | `text`, `editor.foreground` |
| text muted | `#cfe0ea` | `text.muted` |
| active line number / indent guide | `#38d2ff` | `editor.active_line_number`, `editor.indent_guide_active` |

### Aurora Neon — syntax (neon tier)

| Token | Hex | Role |
|---|---|---|
| function / tag | `#38d2ff` | electric cyan |
| type / title / enum | `#6fdcff` | light cyan |
| property | `#5ed0ff` | sky |
| keyword | `#c4a5ff` | violet |
| string | `#5ef0d8` | mint |
| number / attribute | `#ffcf6b` | gold |
| constant / boolean | `#ff9ec9` | rose |
| comment | `#93b8c8` | muted (italic) |
| operator / punctuation | `#cfe0ea` | bright muted |

`accents` cycle: `#38d2ff` `#c4a5ff` `#ff9ec9` `#5ef0d8` `#ffcf6b` `#6fdcff`.
**Light** variant: cyan primary `#0288d1` on `#ffffff` (canonical light hues).

Icon theme: 60 generated glyph-tile SVGs (`editors/zed/icons/*.svg`) covering 98
file suffixes + 22 stems, category-tinted (cyan=web, gold=systems, mint=scripting,
violet=jvm/config, light-cyan=data, rose=docs/media). Regenerate via
`python3 editors/zed/generate-icons.py`. Icon themes load **only** from an
installed extension, not a drop-in `themes/` file.

---

## CLI / Claude Code (`editors/claude-code/`)

Canonical CLI terminal palette — brightened cyan/rose. Full key set in the repo's
`editors/claude-code/TOKENS.md` (every Claude Code theme key + value).

| Role | Hex | Notes |
|---|---|---|
| cyan primary (`claude`, `permission`, `suggestion`) | `#36c9ff` | web uses `#29b6f6` |
| cyan shimmer | `#7ee0ff` | |
| ide / secondary blue | `#4dc8fa` | |
| deep cyan (borders) | `#1c7fac` | |
| rose (`remember`) | `#ff7eb6` | web uses `#f9a8c4` |
| violet (`merged`, `effortUltra`) | `#a78bfa` | |
| success teal | `#7dd3c7` | |
| warn amber | `#c6a36b` | |
| error rose | `#c78490` | |
| text primary | `#e6f4fb` | |
| inactive text | `#cfe0ec` | brightened from `#a7bcc9` for menu legibility |
| page bg | `#07131c` | |

Diff uses the Aurora rose scheme (removals are rose `#2e151c`/`#5e2a38`, not red;
additions stay teal `#0f2a24`/`#1d5448`). Subagent wheel + `rainbow_*` ramps are
fully defined in `TOKENS.md`. Light variant keeps web hues (cyan `#0288d1`).

Deploy: `cp editors/claude-code/aurora.json ~/.claude/themes/aurora.json` then
re-run `/theme`.

---

## Shell tools (`shell/`) — mirror the web canonical palette

Themes for tools that run *inside* any terminal. These use the **canonical**
`#29b6f6`/`#f9a8c4` palette (not the brightened CLI tier).

| Token | Hex | 24-bit | Role |
|---|---|---|---|
| navy (bg) | `#07131c` | `7;19;28` | background |
| panel | `#102330` | `16;35;48` | surfaces |
| white | `#e6f4fb` | `230;244;251` | values / typed text |
| muted | `#a7bcc9` | `167;188;201` | labels |
| dim | `#3d6070` | `61;96;112` | line numbers / suggestions |
| cyan | `#29b6f6` | `41;182;246` | identity / commands |
| cyan+ | `#67cbfa` | `103;203;250` | accents |
| deep-blue | `#1c7fac` | `28;127;172` | separators |
| rose | `#f9a8c4` | `249;168;196` | git branch |
| rose-deep | `#e879a0` | `232;121;160` | git dirty |
| violet | `#a78bfa` | `167;139;250` | python / consts |
| teal | `#7dd3c7` | `125;211;199` | success / strings |
| amber | `#c6a36b` | `198;163;107` | numbers / timing |
| error | `#c78490` | `199;132;144` | errors |

Per-tool files: `shell/p10k/aurora-p10k.zsh` (Powerlevel10k, 24-bit), `shell/bat/Aurora.tmTheme`,
`shell/mc/aurora.ini` (Midnight Commander), `shell/nano/nanorc` (named colors only),
`shell/zsh/aurora-{fzf,fsh,eza}.zsh` + `aurora.dircolors`, `shell/statusline/statusline-aurora.sh`.

### Terminal ANSI foundation (`editors/warp/themes/aurora.yaml`)

`bat --theme=ansi` and `nano`'s named colors resolve through the terminal's 16
ANSI colors, so the emulator palette is the highest-leverage surface. Aurora's
ANSI mapping (Warp `terminal_colors`):

| | black | red | green | yellow | blue | magenta | cyan | white |
|---|---|---|---|---|---|---|---|---|
| normal | `#14304a` | `#e090a0` | `#8fe6d8` | `#e0bc7a` | `#4dc8fa` | `#ffb0d0` | `#67d4ff` | `#c4d8e4` |
| bright | `#3a92c0` | `#f9a8c4` | `#b0f0e6` | `#f2d090` | `#8ad8ff` | `#c4b5fd` | `#d0f0ff` | `#f4fafe` |

---

## Source-of-truth files (in the repo)

- `editors/zed/themes/aurora.json` + `editors/zed/icon_themes/aurora.json` — Zed Neon
- `editors/claude-code/TOKENS.md` — exhaustive Claude Code CLI token reference
- `editors/warp/themes/aurora.yaml` — Warp + the ANSI foundation
- `shell/README.md` — shell palette + per-tool inventory

When palette values change, the deployed copies (`~/.config/zed/`, `~/.claude/themes/`,
`public/{zed,warp}/`) must be re-synced; see each tool's README.
