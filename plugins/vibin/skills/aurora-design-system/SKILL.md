---
name: aurora-design-system
description: Use whenever building, modifying, or styling React/Next.js UI for any Aurora, Labby, or Lab/gateway-admin surface, or when the user mentions Aurora, Labby UI, the operator console, the aurora-design-system repo (`~/workspace/aurora-design-system`), `aurora.tootie.tv`, the `@aurora` shadcn registry, `registry/aurora/`, `--aurora-*` CSS tokens, `aurora-page-shell`, or asks for the dark operator/agent control-plane look. Make sure to invoke this skill any time React, Next.js, shadcn, Tailwind, or "dashboard"-style UI work happens inside `~/workspace/aurora-design-system` or in a project that consumes the Aurora registry, even if the user doesn't say "Aurora" by name. Aurora is dark-first, uses a navy palette with cyan primary / rose secondary / violet AI accents, muted status colors, a Manrope + Inter + JetBrains Mono type stack, and `var(--aurora-*)` semantic tokens — never raw hex.
---

# Aurora Design System

Aurora is the operator-grade design system that powers Labby and the broader Lab gateway-admin surfaces. It is published as a shadcn-compatible registry at `aurora.tootie.tv` and lives in source at `~/workspace/aurora-design-system`. Use it whenever you're producing React/Next.js UI that should look like it belongs in that family.

If the user invokes this skill without a concrete build target, ask what surface they want designed or implemented and whether the output should be production code, a static HTML artifact, or a mock/prototype. Keep the questions short, then proceed as an expert designer once the target is clear.

The non-negotiables Aurora gives you and that every consumer must respect:

- Dark-first, navy lift tiers — flat page, raised toolbars/headers, strongly raised inspector panels
- Cyan primary accent, rose secondary, violet for AI/automation identity, muted status colors (never neon)
- Manrope for display, Inter for working UI, JetBrains Mono **only** for code/paths/IDs/hashes
- Tokenized everything via `--aurora-*` CSS custom properties — never inline hex in product code
- Selection and focus are **border + glow**, not flooded fills
- Sentence case copy, matter-of-fact status text, no marketing voice
- Lucide line icons only, 14-18px, stroke 1.5-1.75px — no emoji as UI

## When you're working inside the source repo

Source-of-truth files in `~/workspace/aurora-design-system`:

- `registry/aurora/styles/aurora.css` — canonical token bridge, semantic CSS variables, type classes, `.aurora-page-shell`, `.aurora-nav-shell`. **Read this before claiming a token exists.**
- `registry/aurora/ui/*.tsx` — 64 stable React primitives.
- `registry/aurora/blocks/<domain>/<name>/*.tsx` — composed product blocks (ai, auth, feedback, files, navigation, workspace).
- `registry.json` — shadcn registry source. **Read this before claiming registry status or counts.**
- `public/r/registry.json` and `public/r/aurora-*.json` — generated output. Stale until `pnpm registry:build` runs.
- `app/gallery/[section]/page.tsx` — gallery routes and live demos. Verify exact route inventory here, not from memory.
- `app/globals.css` — imports `registry/aurora/styles/aurora.css`.

Derive counts, route lists, and component inventories from these files — do not hard-code them in commits or PR descriptions. The "Component inventory" section below is a *category map* for orienting, not a count source.

## When you're consuming Aurora in another project

Install the token layer first, then any component:

```bash
# Token layer (required — every component reads from these vars)
npx shadcn@latest add https://aurora.tootie.tv/r/aurora-tokens.json

# Then any component
npx shadcn@latest add https://aurora.tootie.tv/r/aurora-button.json
```

Or register the namespace once in your `components.json`:

```json
{
  "registries": {
    "@aurora": "https://aurora.tootie.tv/r/{name}.json"
  }
}
```

Then `npx shadcn@latest add @aurora/aurora-tokens` and `@aurora/aurora-<name>`.

Required setup in the consuming app:

```css
/* app/globals.css */
@import "../registry/aurora/styles/aurora.css";
```

```tsx
// app/layout.tsx — load fonts and default to dark
import { Manrope, Inter, JetBrains_Mono } from "next/font/google"

<html lang="en" className="dark">
```

Toggle light with `document.documentElement.classList.toggle("light")` and remove `"dark"`. Both modes must remain usable — verify the surface in `.light` before shipping.

## When you're making static artifacts

For slides, mocks, throwaway prototypes, design reviews, and other non-production visual artifacts, create a static HTML file the user can open directly unless the artifact genuinely needs a dev server.

- Load Manrope, Inter, and JetBrains Mono from Google Fonts.
- Include the Aurora token layer in the artifact folder: copy the source CSS when available, or embed only the required token/type/page-shell CSS for a self-contained mock.
- Default to dark mode with `<html class="dark">` and put the main canvas on `<body class="aurora-page-shell">` or a top-level `.aurora-page-shell` wrapper.
- Copy any needed brand assets into the artifact directory and reference them locally. Do not hotlink, redraw, or approximate the Labby mark when a real asset exists.
- Use Lucide icons through the web package/CDN or inline icon markup that matches Lucide stroke rules: 14-18px, `currentColor`, 1.5-1.75 stroke.
- Keep the artifact faithful to Aurora's production rules: tokenized colors, Tier 2 panels, border + glow selection, sentence case copy, muted status colors, and no emoji.
- If you create an HTML artifact, tell the user the local path and whether it is static-openable or served by a dev server.

## Tokens — never raw hex

Reach for semantic Aurora vars, or `color-mix(in srgb, var(--aurora-accent-primary) 14%, transparent)` for tinted fills. Full list and light/dark values in `references/tokens.md`; headline shape:

### Surfaces

| Token | Purpose |
|---|---|
| `--aurora-page-bg` | Page background (apply via `.aurora-page-shell`) |
| `--aurora-nav-bg` | Sidebars, nav rails (apply via `.aurora-nav-shell`) |
| `--aurora-panel-medium` | Tier 1 surfaces (toolbars, headers, cards) |
| `--aurora-panel-strong` | Tier 2 surfaces (inspectors, primary content panels) |
| `--aurora-control-surface` | Input/control backgrounds |
| `--aurora-hover-bg` | Hovered rows, menu items |

### Borders

| Token | Purpose |
|---|---|
| `--aurora-border-default` | Resting separators, dividers, table rules |
| `--aurora-border-strong` | Cards, inputs, selected surfaces |

### Text

| Token | Purpose |
|---|---|
| `--aurora-text-primary` | Headings, body, control labels |
| `--aurora-text-muted` | Captions, meta, descriptions, placeholders |

### Accents

| Family | Use for |
|---|---|
| `--aurora-accent-primary` / `-strong` / `-deep` / `-lift` / `-button` (cyan) | Primary CTAs, selection, focus, active state |
| `--aurora-accent-pink` / `-strong` / `-deep` / `-button` (rose) | Secondary CTAs, agent affordances, send buttons, key labels in mono code, active filter tags — **one or two touch points per screen, not splattered** |
| `--aurora-accent-violet` / `-strong` / `-deep` / `-button` (violet) | AI/automation identity — model selectors, reasoning panels, autonomous actions |

Each accent family has matching `-surface` and `-border` mix tokens; use those for tinted backgrounds and matching borders.

### Status — muted, never neon

| Family | Tokens |
|---|---|
| info (cyan-lean) | `--aurora-info`, `--aurora-info-surface`, `--aurora-info-border`, `--aurora-info-foreground` |
| success (teal-mint) | `--aurora-success`, `--aurora-success-surface`, `--aurora-success-border`, `--aurora-success-foreground` |
| warn (warm sand) | `--aurora-warn`, `--aurora-warn-surface`, `--aurora-warn-border`, `--aurora-warn-foreground` |
| error (rose-clay) | `--aurora-error`, `--aurora-error-surface`, `--aurora-error-border`, `--aurora-error-foreground` |
| neutral (slate) | `--aurora-neutral`, `--aurora-neutral-surface`, `--aurora-neutral-border`, `--aurora-neutral-foreground` |

`--aurora-status-offline` is deprecated; alias to `--aurora-neutral`.

### Radii, shadows, glows

- `--aurora-radius-1` (14px) — chips, buttons
- `--aurora-radius-2` (18px) — small cards, popovers
- `--aurora-radius-3` (22px) — panels (Tier 1, Tier 2)
- Tables override to `border-radius: 8px` on the wrapper (resolved decision)
- `--aurora-shadow-medium` for Tier 1, `--aurora-shadow-strong` for Tier 2
- `--aurora-highlight-medium` / `-strong` — inset top highlights to pair with shadows
- `--aurora-active-glow` — border + glow for selected/active states
- `--aurora-focus-ring` / `--aurora-focus-ring-strong` — focus-visible rings

## Typography ramp — semantic, not pixel-pushing

Use the `.aurora-text-*` classes from `registry/aurora/styles/aurora.css`. Pick the slot first; if the color is wrong, override color before inventing a new size.

| Class | Font | Use for |
|---|---|---|
| `.aurora-text-display-1` | Manrope 800 | Page heroes, big numbers |
| `.aurora-text-display-2` | Manrope 700 | Section heroes |
| `.aurora-text-section` | Manrope 760 | Section headers, card titles |
| `.aurora-text-body` | Inter 480, 14px | Default body |
| `.aurora-text-body-sm` | Inter 480, 13px | Compact body |
| `.aurora-text-ui` | Inter 560, 13px | Working UI text (controls, list items) |
| `.aurora-text-control` | Inter 560, 13px dense | Control labels, button text |
| `.aurora-text-table` | Inter 480, 13px | Table cells |
| `.aurora-text-label` | Inter 650, 12px | Form labels |
| `.aurora-text-caption` | Inter 560, 11px | Captions |
| `.aurora-text-meta` | Inter 560, 11px muted | Metadata, timestamps |
| `.aurora-text-eyebrow` | Inter 650, 11px uppercase | Eyebrows, badge labels |
| `.aurora-text-code` | JetBrains Mono 520 | Inline code, IDs, paths |

**Mono is restricted.** Use `JetBrains Mono` / `.aurora-text-code` only for code blocks, terminal output, file paths, IDs/hashes, badge chips in code contexts, and inline code snippets. Never for form labels, file tree names, body prose, general UI copy, or breadcrumb text.

## Visual rules

- **Three lift tiers.** Tier 0 page is flat (the `.aurora-page-shell` wash counts). Tier 1 toolbars/headers use `--aurora-shadow-medium`. Tier 2 inspectors/primary panels use `--aurora-shadow-strong`. Pair every elevated surface with an inset top highlight (e.g. `inset 0 1px 0 rgba(255,255,255,0.05)`).
- **Selection + focus = border + glow.** Don't flood fills. Use `--aurora-active-glow` for selected items and `--aurora-focus-ring`/`-strong` for keyboard focus.
- **Borders.** `--aurora-border-default` for resting, `--aurora-border-strong` for cards, inputs, selected surfaces.
- **No glassmorphism. No imagery on chrome.** The only sanctioned chrome gradient is the `.aurora-page-shell` wash.
- **Left-border glow** (`border-left: 3px solid`) for in-progress / active list indicators. Don't use `box-shadow` for left-edge indicators on rounded elements — it rounds the glow.
- **Stat cards** are narrow tracks, not full-width: `grid-template-columns: repeat(auto-fill, minmax(175px, 220px))`.
- **Tables** use `border-radius: 8px` on the wrapper, not the 22px panel radius.

## Components — use the registry, don't hand-roll

The full inventory with import paths lives in `references/components.md`. The categories:

- **Foundations**: tokens, type ramp, brand mark
- **Controls**: button, button group, badge, switch, avatar, progress, spinner, toolbar, kbd, separator, accordion, toggle, toggle group
- **Form elements**: field, label, input, input group, input OTP, select, native select, textarea, checkbox, radio group, slider, number input, combobox, date picker, tabs
- **Feedback**: alert/callout, banner, toast, tooltip, empty state, skeleton, shimmer
- **Navigation**: breadcrumb, sidebar, command palette, navigation menu, menubar, scroll area, pagination
- **Data**: stat card, card, item, table, data table, chart, carousel, filter bar, marketplace catalog, status indicator, timeline, description list, search results, calendar
- **Overlays**: dialog, alert dialog, dropdown menu, context menu, hover card, popover, sheet/drawer, collapsible, permissions dropdown, thinking disclosure
- **Chat & AI blocks**: prompt input, AI elements, message, inline citation, sources, suggestion, queue, checkpoint, confirmation, context, conversation, model selector, tool calls, reasoning, code block, artifact, terminal, permission prompt, ask-user-question, attachment, agent, commit, package info, sandbox, schema display, snippet, stack trace, test results, env vars
- **Workspace blocks**: file picker, file tree, code editor, JSX preview, web preview, share dialog, resizable panels
- **Auth & errors**: login, OAuth flow, error pages

**Rule**: if a registry component covers the affordance, import it. Don't hand-roll a `<button>` next to an Aurora `Button`. Don't hand-roll a status pill next to `Badge`.

### Button — canonical example

```tsx
import { Button } from "@/registry/aurora/ui/button"

<Button variant="aurora">Run query</Button>        {/* primary cyan glow */}
<Button variant="rose">Send to agent</Button>      {/* agent / send affordance */}
<Button variant="neutral">Cancel</Button>          {/* secondary control */}
<Button variant="ghost">Filter</Button>            {/* tertiary, no chrome */}
<Button variant="destructive">Delete</Button>      {/* destructive action */}
<Button variant="aurora" size="sm">…</Button>      {/* sm | default | lg | icon */}
```

The canonical button style is *Aurora glow border* — the component implements the gradient + inset highlight + cyan glow automatically. Hand-rolling that style is a smell.

### Badge — Style B is canonical

```tsx
import { Badge } from "@/registry/aurora/ui/badge"

<Badge tone="info">Pending</Badge>
<Badge tone="success">Online</Badge>
<Badge tone="warn">Degraded</Badge>
<Badge tone="error">Failed</Badge>
<Badge tone="rose">Filtered</Badge>      {/* active filter tags */}
<Badge tone="violet">AI</Badge>           {/* AI / autonomous */}
<Badge tone="neutral">Idle</Badge>
```

Style B is the canonical badge: square chip, JetBrains Mono, 4px radius, optional glow dot in `currentColor`. Pill styles (Style A, C, D) exist but are secondary — only reach for them when the surface specifically needs the lower-noise read.

### Banner

Style A1 (elevated + glowing dot + dismiss) for high-priority alerts (error, warn, info). Style C (mono tag + single line) for inline table rows and compact notices. **Style B (left rule) was explored and removed — don't reintroduce it.**

## Content rules

- Sentence case for buttons, headers, table columns, menu items. *Active gateways*, not *Active Gateways*.
- Uppercase only for eyebrows and badge labels.
- No exclamation marks in chrome.
- No "we", no apology framing, no marketing verbs.
- Status copy is matter-of-fact: *"Backend unavailable."*, *"Plex authorized."*, *"Couldn't reach gateway. Retrying."* — not *"Oops! Something went wrong"*.

## Designing a new surface — recipe

1. Apply `className="aurora-page-shell"` to the page root when the surface owns the page background.
2. For sidebars/nav rails, apply `className="aurora-nav-shell"`.
3. Build working areas on Tier 2 panels:
   ```tsx
   <div
     className="rounded-[var(--aurora-radius-3)]"
     style={{
       background: "var(--aurora-panel-strong)",
       borderColor: "var(--aurora-border-strong)",
       borderWidth: 1,
       boxShadow: "var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05)",
     }}
   />
   ```
4. Headers/toolbars sit on Tier 1: `background: var(--aurora-panel-medium)`, `boxShadow: var(--aurora-shadow-medium)`.
5. Type: pick from the `.aurora-text-*` ramp. Manrope for display/section titles, Inter everywhere else, mono only for code/paths/IDs.
6. Use tokenized tint fills: `color-mix(in srgb, var(--aurora-accent-primary) 14%, transparent)`.
7. Selection: border + glow (`--aurora-active-glow`). Focus-visible: `--aurora-focus-ring-strong`.
8. Use registry components (`Button`, `Badge`, `Banner`, `StatCard`, `DataTable`, etc.) before hand-rolling.
9. Verify the same surface renders correctly in `.light` before shipping.

See `references/recipes.md` for full copy-pasteable patterns: two-pane operator layout, command palette trigger row, stat grid, status row with badge + meta, agent prompt with model selector, data table with filter bar.

## Resolved decisions — don't relitigate

These have been argued and settled. If you find yourself proposing the rejected option, re-read this list:

- **Terminal chrome:** Aurora-native only. No macOS circles/dots. Title bar uses `--aurora-panel-strong`, the stacked-plane mark when available, and Aurora `Button` actions for kill/clear/run.
- **Left-border glow:** `border-left: 3px solid` for active/in-progress. Avoid `box-shadow` for left-edge on rounded elements (rounds the glow).
- **Breadcrumb badge position:** Badge goes *left* of the item name.
- **Stat cards:** narrow tracks (`repeat(auto-fill, minmax(175px, 220px))`), do not stretch full-width.
- **Table radii:** 8px wrapper, not 22px panel radius.
- **Active filter tags:** rose. Cyan blends into the control surface.
- **Toasts:** Labby stacked-plane mark replaces colored dot/circle icons where the mark is available. Dismiss `x` is colored by status type.
- **Banner Style B (left rule):** removed. Use A1 or C.

## Brand & mark

- **Primary mark — stacked plane.** Four isometric diamond planes (dark → light cyan, bottom → top) for cli / api / mcp / web layers. Canonical favicon, app icon, and inline mark when available.
- **Secondary mark — hub & spoke.** Six nodes radiating from a central core. Use only when the *control plane fanning out to services* read is needed.
- **Wordmark — `Labby`.** Manrope 800, tight tracking, sentence case. The gallery renders `Labb` in primary text and accents `y` with `--aurora-accent-pink`. Verify `app/gallery/demos/brand-demo.tsx` before changing the canonical mark.

## Registry workflow (only when working in the source repo)

When you've changed registry source:

1. Inspect / update `registry.json`. Token-dependent components must declare `registryDependencies: ["aurora-tokens"]`.
2. Run `pnpm registry:build`. This regenerates `public/r/*.json` and `public/r/registry.json`. They are otherwise stale.
3. If you added a gallery demo, wire both `app/gallery/[section]/page.tsx` *and* `app/gallery/layout.tsx`.
4. Run `pnpm lint`. For route/demo changes, run `pnpm build` to catch SSR/static-param breakage.
5. Verify inventory claims against `registry.json` and gallery routes — don't write counts from memory.

Install URL for end users: `https://aurora.tootie.tv` (root) or `https://aurora.tootie.tv/r/<name>.json` (direct).

## Reference files

- `references/tokens.md` — full token list with values for both dark and light modes, plus the shadcn token bridge.
- `references/components.md` — every UI primitive and block with import path and the props that matter.
- `references/recipes.md` — copy-pasteable patterns: page shell, two-pane layout, stat grid, command palette trigger, prompt input with model selector, data table with filter bar.

Read the reference that matches what you're about to build before you write the JSX.
