# check-skill-clis

Audits CLI dependencies referenced by installed skills across Claude, Codex, and Copilot. Reports which required commands are missing, shadowed by name collisions, or only available through shell aliases.

## What it does

1. Inventories skills under `~/.claude/**`, `~/.codex/skills/**`, `~/.config/github-copilot/**`, plus local authored skills like `~/.agents/shared/skills/**`.
2. Extracts CLI references from Markdown, YAML, JSON, TOML, and shell snippets.
3. For each candidate CLI: `command -v`, `which -a`, plus a low-risk `--version` probe.
4. Classifies each skill as `active` / `installed` / `disabled` / `unknown` and each CLI as resolved / missing / suspicious.
5. Writes a markdown report.

## Invoke

Triggers: "check if my skill CLIs are installed", "audit skill commands", "what skills have missing tools", "verify skill dependencies".

## Usage

```bash
python3 ~/.agents/shared/skills/check-skill-clis/scripts/audit_skill_clis.py \
  --output docs/reports/skill-cli-audit.md
```

Flags: `--json <path>`, `--include-common`, `--only-root <path>`, `--active-skills-file <path>`, `--disabled-skill <name>`. Exit code is 1 if any required CLI is missing — useful for CI gating.

## Files

- `SKILL.md` — agent instructions
- `scripts/audit_skill_clis.py` — the audit script (zero-dependency stdlib)
- `agents/openai.yaml` — OpenAI runtime metadata
