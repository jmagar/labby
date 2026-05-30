---
title: "Reference Changes Impact Report"
doc_type: "template"
status: "draft"
owner: "refresh-docs"
audience:
  - "contributors"
scope: "reference"
source_of_truth: false
upstream_refs:
  - "docs/references/CHANGES.md"
  - "docs/references/INDEX.md"
related: []
last_reviewed: "2026-05-13"
last_modified: "2026-05-13"
modified_on_branch: "main"
modified_at_version: "0.1.0"
modified_at_commit: "unborn"
review_basis: "template reviewed against refresh-docs workflow and local docs/references snapshot"
generated_at: "TIMESTAMP_UTC"
source_changes_log: "docs/references/CHANGES.md"
reviewed_change_entry: "TIMESTAMP_UTC"
reviewer: "refresh-docs skill"
---

# Reference Changes Impact Report

## Latest Refresh Summary

Summarize the latest `docs/references/CHANGES.md` entry.

Include:

- Refresh timestamp and scope.
- Whether the entry came from a real run, not `--dry-run`.
- Added, modified, and removed counts.
- Any path-normalization caveats, such as leading whitespace inside backticked paths.
- Reference layout/source assumptions checked against `docs/references/INDEX.md`.

## Changed References Reviewed

List every added, modified, and removed reference file from the latest entry with review status and notes.

Normalize each changed path under `docs/references/` before review. For removed files, record how they were assessed, such as matched to an added slug, confirmed absent, or compared against a current replacement.

## Upstream Reference Paths Used

List the exact upstream reference paths used to cross-check this report. Include `docs/references/INDEX.md`, `scripts/refresh-docs.sh`, `docs/references/CHANGES.md`, and each changed `docs/references/...` path or grouped corpus path actually reviewed.

## Affected Implementation Areas

List exact repo paths that may need updates, grouped by area.

## Required Changes

List changes that should be made to keep the implementation aligned with refreshed references. If none, state that explicitly.

## New Additions To Consider

List new capabilities, commands, docs, tests, workflows, or integrations enabled by the refreshed references.

## Verification To Run

List focused tests, checks, or manual verification needed before implementing follow-up work.

Also list the commands used to generate and verify the report, including the refresh command and focused `rg`, `find`, `sed`, or `awk` checks.

## Unknowns

List assumptions, ambiguous upstream changes, or decisions needing human review.
