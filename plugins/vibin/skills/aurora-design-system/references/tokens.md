# Aurora tokens — full reference

All Aurora components read from these CSS custom properties. They are declared twice in `registry/aurora/styles/aurora.css` — once under `:root, .dark` and once under `.light`. **Never hard-code hex in product code** — reach for these vars, or `color-mix(...)` against them for tinted fills.

The shadcn token bridge at the bottom of each block remaps `--background`, `--primary`, etc. onto Aurora vars so vanilla shadcn components inherit the system without code changes.

## Surfaces

| Token | Dark | Light | Purpose |
|---|---|---|---|
| `--aurora-page-bg` | `#07131c` | `#f0f6f8` | Page background |
| `--aurora-bg` | alias of page-bg | alias of page-bg | Generic surface alias |
| `--aurora-nav-bg` | `#07111a` | `#e4f0f5` | Sidebars, nav rails |
| `--aurora-panel-medium` | `#102330` | `#ffffff` | Tier 1 (toolbars, headers, cards) |
| `--aurora-panel-strong` | `#13293a` | `#f8fbfc` | Tier 2 (inspectors, primary content panels) |
| `--aurora-control-surface` | `#0c1a24` | `#edf5f8` | Input/control backgrounds |
| `--aurora-hover-bg` | `#17364b` | `#dceef4` | Hovered rows, menu items |
| `--aurora-surface-raised` | gradient | gradient | Optional layered surface gradient |

## Borders

| Token | Dark | Light | Purpose |
|---|---|---|---|
| `--aurora-border-default` | `#1d3d4e` | `#b8d0da` | Resting separators, table rules |
| `--aurora-border-strong` | `#24536c` | `#8fb4c4` | Cards, inputs, selected surfaces |

## Text

| Token | Dark | Light | Purpose |
|---|---|---|---|
| `--aurora-text-primary` | `#e6f4fb` | `#07131c` | Headings, body, control labels |
| `--aurora-text-muted` | `#a7bcc9` | `#3d6070` | Captions, meta, descriptions, placeholders |

## Cyan accent (primary)

| Token | Dark | Light | Purpose |
|---|---|---|---|
| `--aurora-accent-primary` | `#29b6f6` | `#0288d1` | Primary CTAs, selection, focus |
| `--aurora-accent-strong` | `#67cbfa` | `#0277bd` | Hover/active emphasis |
| `--aurora-accent-deep` | `#1c7fac` | `#01579b` | Pressed, deep accent fills |
| `--aurora-accent-lift` | `#4dc8fa` | `#1aa6e8` | Gradient top in button face |
| `--aurora-accent-button` | `#1da8e6` | `#0277bd` | Gradient bottom in button face |
| `--aurora-accent-foreground` | `#051520` | `#ffffff` | Foreground on filled cyan |
| `--aurora-accent-gradient` | linear-gradient | (inherits) | Filled cyan gradient |

## Rose accent (secondary)

Rose is sanctioned for mono code highlights, key labels, send/agent affordances, rose buttons, and active filter tags. **Keep it to one or two touch points per screen.**

| Token | Dark | Light | Purpose |
|---|---|---|---|
| `--aurora-accent-pink` | `#f9a8c4` | `#d63a6f` | Base rose |
| `--aurora-accent-pink-strong` | `#fbc4d6` | `#e0527f` | Hover/active rose |
| `--aurora-accent-pink-deep` | `#c46b88` | `#a0284f` | Deep rose |
| `--aurora-accent-pink-button` | `#e879a0` | `#b82f60` | Gradient bottom of rose button |
| `--aurora-accent-pink-surface` | mix 12% panel | mix 10% panel | Tinted rose backgrounds |
| `--aurora-accent-pink-border` | mix 32% transparent | mix 26% transparent | Matching rose borders |
| `--aurora-rose-gradient` | linear-gradient | (inherits) | Filled rose gradient |

## Violet accent (AI / automation identity)

Use for autonomous-agent affordances, model selectors, reasoning/thinking surfaces — anywhere the "this is the AI doing something" read matters.

| Token | Dark | Light | Purpose |
|---|---|---|---|
| `--aurora-accent-violet` | `#a78bfa` | `#7c3aed` | Base violet |
| `--aurora-accent-violet-strong` | `#c4b5fd` | `#8b5cf6` | Hover/active violet |
| `--aurora-accent-violet-deep` | `#7c3aed` | `#5b21b6` | Deep violet |
| `--aurora-accent-violet-button` | `#8b5cf6` | `#6d28d9` | Gradient bottom |
| `--aurora-accent-violet-surface` | mix 12% panel | mix 10% panel | Tinted violet backgrounds |
| `--aurora-accent-violet-border` | mix 32% transparent | mix 26% transparent | Matching violet borders |

## Status families — muted, never neon

Each family has base / surface / border / foreground for full chip + banner composition.

### info

| Token | Dark | Light |
|---|---|---|
| `--aurora-info` | `#72c8f5` | `#0f7db8` |
| `--aurora-info-surface` | mix 12% panel | mix 10% panel |
| `--aurora-info-border` | mix 34% transparent | mix 26% transparent |
| `--aurora-info-foreground` | `#dff5ff` | `#0a4f74` |

### success

| Token | Dark | Light |
|---|---|---|
| `--aurora-success` | `#7dd3c7` | `#2d7d6e` |
| `--aurora-success-surface` | mix 12% panel | mix 12% panel |
| `--aurora-success-border` | mix 34% transparent | mix 24% transparent |
| `--aurora-success-foreground` | `#dcfbf6` | `#184b43` |

### warn

| Token | Dark | Light |
|---|---|---|
| `--aurora-warn` | `#c6a36b` | `#8a6914` |
| `--aurora-warn-surface` | mix 12% panel | mix 12% panel |
| `--aurora-warn-border` | mix 34% transparent | mix 24% transparent |
| `--aurora-warn-foreground` | `#f8ead0` | `#5c450c` |

### error

| Token | Dark | Light |
|---|---|---|
| `--aurora-error` | `#c78490` | `#9c3545` |
| `--aurora-error-lift` | `#d9909a` | `#b84a5b` |
| `--aurora-error-surface` | mix 12% panel | mix 10% panel |
| `--aurora-error-border` | mix 34% transparent | mix 24% transparent |
| `--aurora-error-foreground` | `#fde6eb` | `#6f2330` |
| `--aurora-error-gradient` | linear-gradient | (inherits) |

### neutral

| Token | Dark | Light |
|---|---|---|
| `--aurora-neutral` | `#91a8b6` | `#6f8793` |
| `--aurora-neutral-surface` | mix 10% panel | mix 12% panel |
| `--aurora-neutral-border` | mix 28% transparent | mix 24% transparent |
| `--aurora-neutral-foreground` | `#d7e3ea` | `#334e5d` |

`--aurora-status-offline` is deprecated — it aliases to `--aurora-neutral`. Don't introduce new usages.

## Interaction + system

| Token | Purpose |
|---|---|
| `--aurora-overlay` | Modal/dialog backdrop |
| `--aurora-disabled-text` | Disabled text color |
| `--aurora-disabled-surface` | Disabled control surface |
| `--aurora-subtle-bg` | Subtle hover / selected mix |
| `--aurora-selected-bg` | Selected row/item background |
| `--aurora-pressed-bg` | Pressed/active button background |
| `--aurora-focus-ring` | Focus-visible ring (subtle) |
| `--aurora-focus-ring-strong` | Focus-visible ring (strong) |
| `--aurora-active-glow` | Border + glow for selected/active items |

## Shadows + highlights

| Token | Dark value | Use |
|---|---|---|
| `--aurora-shadow-medium` | `0 12px 24px rgba(0,0,0,0.18)` | Tier 1 surfaces |
| `--aurora-shadow-strong` | `0 20px 38px rgba(0,0,0,0.26)` | Tier 2 surfaces |
| `--aurora-highlight-medium` | `inset 0 1px 0 rgba(255,255,255,0.035)` | Tier 1 inset highlight |
| `--aurora-highlight-strong` | `inset 0 1px 0 rgba(255,255,255,0.055)` | Tier 2 inset highlight |

Pair an inset highlight with every shadow — flat-shadowed panels read as plastic without it.

## Radii

| Token | Value | Use |
|---|---|---|
| `--aurora-radius-1` | 14px | Buttons, chips |
| `--aurora-radius-2` | 18px | Small cards, popovers |
| `--aurora-radius-3` | 22px | Panels (Tier 1, Tier 2) |

Tables override to `border-radius: 8px` on the wrapper. The default shadcn bridge `--radius` is `14px`.

## Typography

```css
--aurora-font-display: 'Manrope', -apple-system, BlinkMacSystemFont, system-ui, sans-serif;
--aurora-font-sans:    'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
--aurora-font-mono:    'JetBrains Mono', 'IBM Plex Mono', ui-monospace, Menlo, monospace;
```

Type sizes:

| Token | Value |
|---|---|
| `--aurora-type-body` | 14px |
| `--aurora-type-body-sm` | 13px |
| `--aurora-type-control` | 13px |
| `--aurora-type-table` | 13px |
| `--aurora-type-label` | 12px |
| `--aurora-type-caption` | 11px |
| `--aurora-type-micro` | 10.5px |

Line heights:

| Token | Value |
|---|---|
| `--aurora-line-body` | 1.58 |
| `--aurora-line-ui` | 1.35 |
| `--aurora-line-dense` | 1.28 |

Letter spacing:

| Token | Value |
|---|---|
| `--aurora-letter-ui` | 0.005em |
| `--aurora-letter-label` | 0.012em |
| `--aurora-letter-meta` | 0.018em |
| `--aurora-letter-eyebrow` | 0.095em |

Weights:

| Token | Value |
|---|---|
| `--aurora-weight-body` | 480 |
| `--aurora-weight-ui` | 560 |
| `--aurora-weight-label` | 650 |
| `--aurora-weight-heading` | 760 |

## Page shell

`--aurora-shell-bg` composes two radial gradients with `--aurora-page-bg`:

```css
--aurora-shell-bg: radial-gradient(circle at 12% 0%, rgba(41, 182, 246, 0.09), transparent 28%),
                   radial-gradient(circle at 88% 0%, rgba(28, 127, 172, 0.10), transparent 24%),
                   var(--aurora-page-bg);
```

Apply via `className="aurora-page-shell"` — this is the **only** sanctioned chrome gradient.

## shadcn token bridge

Aurora remaps shadcn's defaults onto its own surface system, so vanilla shadcn components pick up the Aurora look without per-component changes:

| shadcn token | Aurora source |
|---|---|
| `--background` | `--aurora-page-bg` |
| `--foreground` | `--aurora-text-primary` |
| `--card` | `--aurora-panel-medium` |
| `--popover` | `--aurora-panel-strong` |
| `--primary` | `--aurora-accent-primary` |
| `--primary-foreground` | `--aurora-accent-foreground` |
| `--secondary` | `--aurora-panel-medium` (dark) / `--aurora-control-surface` (light) |
| `--muted` | `--aurora-control-surface` |
| `--accent` | `--aurora-hover-bg` |
| `--destructive` | `--aurora-error` |
| `--border` | `--aurora-border-default` |
| `--input` | `--aurora-control-surface` |
| `--ring` | `--aurora-focus-ring` |
| `--radius` | 14px |
| `--sidebar`, `--sidebar-*` | mapped to `--aurora-nav-*`, accent, border, ring |

When adding new shadcn-style tokens, mirror them in both blocks. Never set the shadcn token in product code — set the Aurora source and let the bridge propagate.
