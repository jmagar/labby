# refresh-docs

Refresh local reference docs (Repomix XML packs + Axon crawls + sparse-cloned docs), inspect what changed, cross-reference the changes against the codebase, and produce an impact report.

## What it does

1. Runs the host project's `scripts/refresh-docs.sh`.
2. Reads the latest entry in `docs/references/CHANGES.md`.
3. Cross-references every changed reference file against the implementation (Rust crates, scripts, skills, docs, config, generated contracts).
4. Archives the prior `CHANGES-REPORT.md` (so writes don't clobber history).
5. Writes a new `docs/references/CHANGES-REPORT.md` listing required updates, verification steps, and possible new additions.

Does **not** make application code changes — pure review/reporting.

## Invoke

Triggers: "refresh docs", "refresh references", "update references", "check what the refresh changed", "write the changes report".

## Prerequisites

Host project must provide `scripts/refresh-docs.sh` and a `docs/references/` tree. The archive script uses `$PWD` (or `$PROJECT_ROOT` if set) as the project root.

## Files

- `SKILL.md` — agent workflow
- `references/CHANGES-REPORT-template.md` — required report structure
- `references/CHANGES-template.md` — append-only changelog format
- `scripts/archive-changes-report.sh` — run before writing a new report
