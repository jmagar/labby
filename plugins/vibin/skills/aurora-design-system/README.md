# aurora-design-system

Aurora design-system guidance for React/Next.js UI work in the operator/agent control-plane look (dark, navy/cyan/rose/violet, Manrope+Inter+JetBrains Mono, `--aurora-*` CSS tokens).

## When to invoke
Any React/Next.js/shadcn/Tailwind work in `~/workspace/aurora-design-system` or a project consuming the Aurora registry — even if the user doesn't say "Aurora" by name. Also use it for static HTML mocks, slides, design reviews, prototypes, screenshots, or throwaway demos that should look like Labby or the Aurora operator console.

## Files
- `SKILL.md` — entrypoint rules, visual foundations, production setup, static artifact workflow, content rules, and registry workflow.
- `references/tokens.md` — full token list with dark/light values and the shadcn token bridge.
- `references/components.md` — UI primitive and product-block inventory with source/import paths.
- `references/recipes.md` — copy-pasteable patterns for page shells, two-pane layouts, Tier 1/Tier 2 panels, status rows, prompts, tables, banners, and empty states.
- `CHANGELOG.md` — local skill changes.
- `agents/openai.yaml` — agent metadata for the skill bundle.

## Source repo map

When working in `~/workspace/aurora-design-system`, verify current facts from source before claiming inventory, counts, or routes:

- `registry/aurora/styles/aurora.css` — canonical tokens, type classes, `.aurora-page-shell`, and `.aurora-nav-shell`.
- `registry/aurora/ui/*.tsx` — reusable UI primitives.
- `registry/aurora/blocks/<domain>/<name>/*.tsx` — composed product blocks for AI, auth, feedback, files, navigation, and workspace surfaces.
- `registry.json` — shadcn registry source.
- `public/r/*.json` — generated registry output after `pnpm registry:build`.
- `app/gallery/[section]/page.tsx` and `app/gallery/demos/*.tsx` — live demo routes and examples.

## Static artifacts

For visual artifacts, create a local static HTML file when possible. Load the Aurora token layer, Google fonts, dark mode, `.aurora-page-shell`, local brand assets when available, and Lucide-style icons. Keep the same production constraints: tokenized colors, Tier 2 panels, border + glow selection, sentence case copy, muted status, and no emoji.
