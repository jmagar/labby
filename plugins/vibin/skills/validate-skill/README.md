# validate-skill

Validate a Claude Code skill's `SKILL.md` — runs `skills-ref validate` for schema, then layers in Claude Code-specific checks that `skills-ref` doesn't cover.

## What it does

1. Resolves the skill directory (from `$ARGUMENTS` or by walking up from CWD).
2. Runs `skills-ref validate` (skipped with a WARN if not installed); suppresses known false-positives like `argument-hint` / `disable-model-invocation`.
3. Adds Claude Code-specific checks:
   - description has trigger phrases
   - `allowed-tools` lists only real tool names
   - referenced `scripts/` and `references/` paths exist on disk
   - frontmatter `name:` matches the directory basename
   - description length is sensible (40–1024 chars)
   - no raw secrets in the body
4. If a `plugin.json` is found by walking up, WARN if the skill isn't registered in its `skills` array.
5. Prints a deterministic, greppable report (`FAIL ` / `WARN ` prefixes + Summary line).

## Invoke

Triggers: "validate this skill", "check my skill", "is my skill valid", "review skill structure". Also use proactively after creating or editing any `SKILL.md`.

## Arguments

Optional: path to the skill directory or its `SKILL.md`.

## Files

- `SKILL.md` — checks + report template
