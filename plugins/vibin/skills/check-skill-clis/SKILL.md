---
name: check-skill-clis
description: "This skill should be used when the user asks to verify, audit, or report on CLI tools required by their agent skills. Triggers include: \"check whether skill CLIs are installed\", \"are all my skill tools available\", \"which commands do my skills need\", \"find missing skill dependencies\", \"compare enabled vs disabled skill CLIs\", or \"produce a report of missing agent tools\" across Claude, Codex, and Copilot surfaces."
---

# Check Skill CLIs

## Overview

Use this skill to inventory skills across Claude, Codex, and Copilot surfaces, extract command-line tools referenced by those skills, and verify whether each CLI resolves on the current machine. Produce evidence: skill path, inferred agent ecosystem, status, command, resolved binary, version probe, and missing or suspicious entries.

## Scope

Audit these ecosystems by default:

- `claude`: `~/.claude/**`, plus Claude plugin-cache skill folders when present.
- `codex`: `~/.codex/skills/**`, `~/.codex/plugins/cache/**/skills/**`, and local authored skills such as `/home/jmagar/.agents/src/skills/**`.
- `copilot`: GitHub Copilot CLI and extension skill surfaces under `~/.config/github-copilot/**`, `~/.config/gh/**`, or plugin-cache paths when present.

Classify status conservatively:

- `active`: skill appears in the current session skill list, active config, symlink target, or enabled manifest.
- `installed`: skill exists on disk but no active evidence was found.
- `disabled`: skill exists under a disabled directory, disabled manifest entry, `.disabled` suffix, commented config entry, or explicit disable list.
- `unknown`: skill exists but status cannot be inferred without more runtime context.

## Workflow

1. **Inventory skill files**
   - Run the bundled audit script from this skill:

     ```bash
     python3 <skill-dir>/scripts/audit_skill_clis.py --output docs/reports/skill-cli-audit.md
     ```

   - If the repo does not use `docs/reports/`, write to a clear local path such as `/tmp/skill-cli-audit.md`.
   - Include `--json <path>` when the user wants machine-readable output.
   - When the current session exposes an active skill list, pass it with repeated `--active-skill <name>` flags or `--active-skills-file <path>`.

2. **Review extraction results**
   - Treat script output as a starting point, not a final truth source.
   - Inspect any high-value skills whose CLI references look ambiguous, especially skills with shell snippets, `allowed-tools`, command examples, setup sections, or wrapper functions.
   - Add manually confirmed CLIs to the report if the script missed them.

3. **Verify installed CLIs**
   - Use `command -v <cli>` or `which -a <cli>` for each candidate.
   - Probe versions with low-risk commands only: `--version`, `version`, `-V`, or tool-specific documented version commands.
   - Do not run mutating commands while auditing availability.

4. **Check resolution quality**
   - Flag missing commands.
   - Flag suspicious resolution, such as a name collision where the resolved binary is clearly the wrong tool.
   - Flag multiple binaries when `which -a` shows conflicting candidates.
   - Flag skills that reference commands only available through shell functions or aliases, because non-interactive agents may not see them.

5. **Report**
   - Summarize totals by ecosystem and status.
   - List missing CLIs first, grouped by command and affected skills.
   - List suspicious or ambiguous CLIs next.
   - Include installed-good CLIs after the findings, not before.
   - Include exact commands run and output paths.

## Expected Output Shape

```markdown
# Skill CLI Audit

## Summary

| ecosystem | active | installed | disabled | unknown | cli refs | missing |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |

## Findings

### Missing

- `rtk`: referenced by `<skill path>`; `command -v rtk` failed.

### Suspicious

- `lab`: multiple binaries found with `which -a`; verify the expected one comes first.

## Inventory

| ecosystem | status | skill | cli | resolution | version |
| --- | --- | --- | --- | --- | --- |
```

## Script Notes

The bundled script intentionally favors recall over precision. It scans Markdown, YAML, JSON, TOML, and shell snippets for common CLI-reference patterns, filters obvious shell keywords, and checks each candidate with `command -v`, `which -a`, and safe version probes. Always review important findings before claiming a CLI is truly required by a skill.
