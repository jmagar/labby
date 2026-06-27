# Code Mode Upstream Description Session

Date: 2026-06-25
Branch: `codex/codemode-upstream-description`

## Summary

Added the first Code Mode description slice for upstream awareness:

- Render the synthetic `codemode` tool description dynamically from the current
  enabled, route-visible gateway upstream namespace names.
- Add model-visible workflow guidance for `codemode.search` ->
  `codemode.describe` -> helper or raw `callTool`.
- Expose `upstreams` and `tools` as top-level `codemode` MCP tool inputs rather
  than implying they belong inside sandbox JavaScript.
- Replace stale “no per-run call-count cap” wording with the current default
  call budget and error kind.
- Add tests for the dynamic description, empty upstream snapshot, route scoping,
  and top-level schema inputs.

## Validation

Ran focused tests:

```bash
cargo test -p labby --all-features code_mode_description -- --nocapture
cargo test -p labby --all-features codemode_description_lists_route_scoped_enabled_upstreams -- --nocapture
```

Both focused test sets passed.

## Follow-Up

The remaining gateway enrichment work is tracked in:

- `docs/superpowers/plans/2026-06-25-gateway-enrichment-hints.md`
- Bead epic `lab-hue6e`

That follow-up covers persisted operator-approved hints, read-only
deterministic/Claude/Codex preview providers, explicit apply, add/import scoped
suggestions, generated docs, and all-features verification.
