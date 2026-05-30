---
title: "Reference Refresh Change Log"
doc_type: "template"
status: "draft"
owner: "refresh-docs"
audience:
  - "contributors"
scope: "reference"
source_of_truth: false
upstream_refs:
  - "docs/references/CHANGES-REPORT.md"
  - "docs/references/CHANGES.md"
  - "docs/references/INDEX.md"
related: []
last_reviewed: "2026-05-13"
last_modified: "2026-05-13"
modified_on_branch: "main"
modified_at_version: "0.1.0"
modified_at_commit: "unborn"
review_basis: "template reviewed against refresh-docs workflow and local docs/references snapshot"
generated_by: "scripts/refresh-docs.sh"
created_at: "TIMESTAMP_UTC"
timezone: "UTC"
purpose: "Append-only log of docs/references refresh changes"
---

# Reference Refresh Change Log

Each entry records file-level changes detected after a real `scripts/refresh-docs.sh` run. Generated log/report files are excluded from the detected file-change set.

Cross-reference this template with:

- `scripts/refresh-docs.sh`
- `docs/references/INDEX.md`
- the upstream corpora under `docs/references/`

Script behavior caveats:

- `--dry-run` does not append an entry.
- `--skip-crawl` records scope `repomix-only`.
- `--skip-repomix` records scope `crawl-only`.
- `docs/references/CHANGES.md`, `docs/references/CHANGES-REPORT.md`, and `docs/references/archive/changes-reports/*` are excluded from the detected file-change set.
- Changed paths are relative to `docs/references/`; trim any leading whitespace inside the backticks before resolving them.

## TIMESTAMP_UTC

- script: `scripts/refresh-docs.sh`
- scope: `full`
- axon_output_dir: `AXON_OUTPUT_DIR_VALUE`
- summary: `0 added, 0 modified, 0 removed`

### Added (0)

_None_

### Modified (0)

_None_

### Removed (0)

_None_
