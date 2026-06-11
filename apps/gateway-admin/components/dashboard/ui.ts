/**
 * Dashboard-local design tokens.
 *
 * Radii: semantic aliases over the Aurora radius tokens (`rounded-aurora-*`,
 * driven by `--radius-1/2/3` in app/globals.css). The repo convention forbids
 * arbitrary rounded radius literals, so squaring is done by retuning those
 * global tokens (currently 6/8/10) rather than per-component overrides.
 *
 * Type: one metric ramp so every big number reads at the same weight/size.
 */

/** Panels, cards, banners — the largest surfaces (radius-3). */
export const DASH_SURFACE = 'rounded-aurora-3'
/** Toggles, table wrappers, list rows (radius-2). */
export const DASH_CONTROL = 'rounded-aurora-2'
/** Pills, hover rows, icon tiles, chips — the smallest elements (radius-1). */
export const DASH_INNER = 'rounded-aurora-1'

/** Primary metric number (stat tiles). Apply a color class alongside. */
export const DASH_METRIC = 'font-display text-[22px] leading-none font-extrabold tabular-nums'
/** Secondary metric number (drawer + panel stats). Apply a color class alongside. */
export const DASH_METRIC_SM = 'font-display text-[19px] leading-none font-extrabold tabular-nums'

/**
 * Pill-toggle tone driven by Aurora tokens, so it reads in light AND dark —
 * unlike the shared `pillTone` helper, whose gradients are hardcoded dark.
 */
export function dashPill(active: boolean): string {
  return active
    ? 'border-aurora-accent-primary/45 bg-[color-mix(in_srgb,var(--aurora-accent-primary)_16%,transparent)] text-aurora-text-primary'
    : 'border-transparent text-aurora-text-muted hover:text-aurora-text-primary'
}
