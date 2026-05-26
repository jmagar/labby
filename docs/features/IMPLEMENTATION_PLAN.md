# Feature / Enhancement / Issue Template

Copy this template into a new file under `docs/plans/` (e.g. `docs/plans/feat-foo-bar.md`) and fill in the sections.

---

## Title

<!-- One-line summary. Prefix with the type: feat / enhance / fix / refactor / docs -->

**Type:** `feat` | `enhance` | `fix` | `refactor`
**Service(s) affected:** <!-- e.g. radarr, unraid, marketplace, gateway, _new_ -->
**Priority:** `p0-critical` | `p1-high` | `p2-normal` | `p3-low`

---

## Problem / Motivation

<!-- What is broken, missing, or inefficient? Why does this matter? -->

---

## Acceptance Criteria

<!-- What must be true for this to be considered done? Use checkboxes. -->

- [ ] <!-- criterion 1 -->
- [ ] <!-- criterion 2 -->
- [ ] <!-- criterion 3 -->
- [ ] All-features build passes (`just build`)
- [ ] All-features tests pass (`just test`)
- [ ] Clippy clean (`just lint`)

---

## Proposed Approach

<!-- High-level description of the implementation strategy. -->

### lab-apis changes

<!-- Changes to `crates/lab-apis/src/<service>/`. Leave blank if none. -->

- [ ] `client.rs` — <!-- describe method additions / changes -->
- [ ] `types.rs` — <!-- new request/response types -->
- [ ] `error.rs` — <!-- new error variants, if any -->

### dispatch layer changes

<!-- Changes to `crates/lab/src/dispatch/<service>/`. See dispatch/CLAUDE.md for required layout. -->

- [ ] `catalog.rs` — <!-- new ActionSpec entries -->
- [ ] `params.rs` — <!-- new param structs -->
- [ ] `dispatch.rs` — <!-- new match arms -->
- [ ] `client.rs` — <!-- client wiring changes -->

### CLI changes

<!-- Changes to `crates/lab/src/cli/<service>.rs`. Thin shims only — no logic. -->

- [ ] <!-- new subcommand or flag -->

### API changes

<!-- Changes to `crates/lab/src/api/services/<service>.rs`. Thin shims only. -->

- [ ] <!-- new route or handler -->

### Config / env vars

<!-- New env vars needed. Format: `SERVICE_VARNAME` — description. -->

| Var | Required | Description |
|-----|----------|-------------|
| | | |

### Other files

<!-- Anything that doesn't fit above. -->

- [ ] `crates/lab/src/registry.rs` — <!-- registration changes -->
- [ ] `docs/` — <!-- doc updates -->

---

## Observability

<!-- Confirm each item applies or mark N/A. See docs/OBSERVABILITY.md. -->

- [ ] Dispatch event emitted with `surface`, `service`, `action`, `elapsed_ms`
- [ ] HTTP `request.start` / `request.finish` | `request.error` emitted
- [ ] Secrets not logged
- [ ] Destructive actions logged with intent + outcome
- [ ] N/A — no new request paths

---

## Error Handling

<!-- Confirm each item applies or mark N/A. See docs/ERRORS.md. -->

- [ ] New `kind` values added to canonical list in `docs/ERRORS.md`
- [ ] MCP and HTTP error envelopes are consistent
- [ ] No panics introduced
- [ ] N/A — no new error surfaces

---

## Destructive Actions

<!-- List any actions that delete, overwrite, or push irreversible state. -->

| Action | Why destructive | Elicitation / `-y` required |
|--------|----------------|----------------------------|
| | | |

<!-- None — leave table empty or add N/A row -->

---

## Testing Plan

<!-- What tests cover this? -->

- [ ] Unit tests with `wiremock` in `lab-apis` (CI-safe)
- [ ] Integration tests marked `#[ignore]` (requires live service)
- [ ] Existing tests updated for changed behavior
- [ ] N/A — no testable surface changes

### Test scenarios

| Scenario | Type | Location |
|----------|------|----------|
| Happy path | unit | `crates/lab-apis/src/<service>/` |
| Auth failure | unit | |
| Unknown action | unit | |
| | | |

---

## Open Questions

<!-- Things to resolve before or during implementation. -->

1. <!-- question -->

---

## Out of Scope

<!-- Explicitly list what this change does NOT cover, to avoid scope creep. -->

- <!-- item -->

---

## References

<!-- Relevant docs, issues, PRs, API specs, or external links. -->

- `docs/DISPATCH.md` — dispatch layer contract
- `docs/OBSERVABILITY.md` — logging requirements
- `docs/ERRORS.md` — error taxonomy
- `docs/SERVICE_ONBOARDING.md` — new service checklist
- <!-- other links -->
