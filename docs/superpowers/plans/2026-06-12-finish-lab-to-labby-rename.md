# Finish Lab To Labby Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the half-completed public rename from standalone product name "Lab" to "Labby" while preserving existing runtime compatibility identifiers that would break users, configs, auth, or crate consumers if renamed blindly.

**Architecture:** Treat "Labby" as the user-facing product, CLI binary, web UI, plugin, marketplace, and documentation name. Keep `lab` only where it is an intentional stable contract: repository/worktree path, Rust crate names, Rust module paths, `~/.labby`, `LAB_*` environment variables, `lab:read` / `lab:admin` scopes, `lab_session` cookie, `lab_admin` internal service id, `lab-apis` / `lab-auth` / `lab-winjob`, generated docs derived from source, and historical session/report artifacts.

**Tech Stack:** Rust 2024 workspace, Cargo, Just, Next.js/React admin app, TypeScript, Markdown docs, Claude/Codex plugin manifests and skills, shell/PowerShell install scripts.

---

## Preconditions

- [ ] Confirm the worktree is current with `origin/main`.

  ```bash
  cd /home/jmagar/workspace/lab/.worktrees/readme-rewrite
  git status --short --branch
  git fetch origin
  git rebase origin/main
  ```

  Expected output after the rebase: the branch line includes `codex/readme-rewrite...origin/main`; uncommitted docs work may remain, but there are no conflict markers.

- [ ] Create a bead before editing rename files.

  ```bash
  bd create --title="Finish Lab to Labby public rename" --description="Normalize user-facing product copy to Labby while preserving stable lab runtime identifiers." --type=task --priority=2 | tee /tmp/labby-rename-bead.txt
  BEAD_ID="$(rg -o 'lab-[a-z0-9]+' /tmp/labby-rename-bead.txt | head -1)"
  test -n "$BEAD_ID"
  bd update "$BEAD_ID" --claim
  ```

  Expected output: a new bead id is printed, `test -n "$BEAD_ID"` exits `0`, and the update marks it claimed.

## Task 1: Add A Rename Policy Document

**Files:**
- Create: `docs/BRANDING.md`
- Modify: `docs/README.md`

- [ ] Create `docs/BRANDING.md` with the exact policy below.

  ```markdown
  # Branding And Compatibility

  Labby is the public product name for this repository's homelab control plane.

  Use **Labby** for user-facing prose, the web UI, CLI help, install instructions, marketplace copy, plugin descriptions, generated site titles, and current documentation. Use **labby** for the CLI binary, release artifact names, command examples, executable paths, and package names that already use the binary spelling.

  Keep **lab** only for stable compatibility identifiers:

  | Identifier | Why it stays |
  |---|---|
  | `github.com/jmagar/lab` and local `~/workspace/lab` paths | Repository identity and existing checkout paths. |
  | `crates/lab`, `crates/lab-apis`, `crates/lab-auth`, `crates/lab-winjob` | Rust crate and workspace package contracts. |
  | `lab_apis`, `lab_auth`, and Rust module paths | Rust import paths derived from existing crate names. |
  | `~/.labby` and `~/.config/labby` | Existing operator config locations. |
  | `LAB_*` environment variables | Existing deployment, CI, shell, and plugin configuration contracts. |
  | `lab:read` and `lab:admin` OAuth scopes | Existing authorization contracts. |
  | `lab_session` cookie | Existing browser session compatibility. |
  | `lab_admin` service id and API/catalog identifiers | Existing MCP/API/catalog compatibility. |
  | Historical `docs/sessions/**`, `docs/reports/**`, and archived references | Historical records should not be rewritten for branding churn. |

  Do not introduce new standalone "Lab" product copy. If a sentence refers to the product a user runs, installs, configures, or sees, call it Labby.

  Generated files under `docs/generated/**` should be regenerated from source instead of edited directly.
  ```

- [ ] Link the new policy from `docs/README.md` in the top-level documentation index.

  Add a row or bullet named `Branding And Compatibility` pointing to `BRANDING.md`, with summary text: `Public Labby naming and intentionally retained lab compatibility identifiers.`

- [ ] Verify the policy file has no broken local links.

  ```bash
  just docs-check
  ```

  Expected output includes `docs artifacts: fresh` or the existing generated-artifact warning, with exit code `0`.

- [ ] Commit the policy document.

  ```bash
  git add docs/BRANDING.md docs/README.md
  git commit -m "Document Labby branding compatibility policy"
  ```

  Expected output: a commit is created containing only `docs/BRANDING.md` and the `docs/README.md` index update.

## Task 2: Update Public Entry Points And Plugin Copy

**Files:**
- Modify: `README.md`
- Modify: `scripts/install.sh`
- Modify: `scripts/install.ps1`
- Modify: `.claude-plugin/marketplace.json`
- Modify: `.agents/plugins/marketplace.json`
- Modify: `plugins/labby/README.md`
- Modify: `plugins/labby/skills/using-labby/SKILL.md`

- [ ] Update `README.md` so the product is consistently "Labby".

  Apply these replacements only in current user-facing prose:

  | From | To |
  |---|---|
  | `Lab is` | `Labby is` |
  | `Lab gives` | `Labby gives` |
  | `Lab exposes` | `Labby exposes` |
  | `Lab runs` | `Labby runs` |
  | `Lab ships` | `Labby ships` |
  | `Lab includes` | `Labby includes` |
  | `Lab keeps` | `Labby keeps` |
  | `Lab's` | `Labby's` |
  | `the Lab CLI` | `the Labby CLI` |
  | `the Lab gateway` | `the Labby gateway` |
  | `the Lab MCP` | `the Labby MCP` |
  | `Lab web UI` | `Labby web UI` |
  | `Lab docs` | `Labby docs` |

  Keep these identifiers unchanged in code blocks and inline literals: `labby`, `LAB_*`, `~/.labby`, `~/.config/labby`, `lab-apis`, `lab-auth`, `lab-winjob`, `lab_apis`, `lab_auth`, `lab:read`, `lab:admin`, `lab_session`, `lab_admin`, `github.com/jmagar/lab`, and file paths containing `/lab/`.

- [ ] Update `scripts/install.sh`.

  Replace `the Lab homelab control plane binary` with `the Labby homelab control plane binary`.

- [ ] Update `scripts/install.ps1`.

  Replace `the Lab homelab control plane binary` with `the Labby homelab control plane binary`.

- [ ] Update `.claude-plugin/marketplace.json`.

  Keep `name` unchanged; ensure the `description` says `Labby homelab control plane`.

- [ ] Update `.agents/plugins/marketplace.json`.

  Keep `name: "jmagar-lab"` unchanged; change `displayName` to `Jacob's Labby`; change `description` to `Local Codex marketplace for Jacob Magar's Labby homelab plugins.`

- [ ] Update `plugins/labby/README.md`.

  Change `Skills and MCP configuration for the Lab homelab control plane.` to `Skills and MCP configuration for the Labby homelab control plane.`

- [ ] Update `plugins/labby/skills/using-labby/SKILL.md`.

  Replace standalone user-facing `Lab binary`, `Lab gateway`, `Lab service`, and `Lab installation` phrasing with `Labby binary`, `Labby gateway`, `Labby service`, and `Labby installation`; keep command examples and environment variable names unchanged.

- [ ] Verify public entry point leftovers.

  ```bash
  rg -n '"Lab|Lab is|Lab gives|Lab exposes|Lab runs|Lab ships|Lab includes|Lab keeps|Lab service|Lab gateway|Lab CLI|Lab web UI|Lab docs|the Lab ' README.md scripts .claude-plugin .agents plugins/labby
  ```

  Expected output: no matches except allowed compatibility explanations in `docs/BRANDING.md` if the command is expanded later to include `docs/`.

- [ ] Commit public entry point copy.

  ```bash
  git add README.md scripts/install.sh scripts/install.ps1 .claude-plugin/marketplace.json .agents/plugins/marketplace.json plugins/labby/README.md plugins/labby/skills/using-labby/SKILL.md
  git commit -m "Rename public Lab copy to Labby"
  ```

  Expected output: a commit is created containing only public entry point, installer, marketplace, and plugin copy updates.

## Task 3: Update Current Documentation

**Files:**
- Modify: `docs/README.md`
- Modify: `docs/ARCH.md`
- Modify: `docs/TECH.md`
- Modify: `docs/OPERATIONS.md`
- Modify: `docs/GATEWAY.md`
- Modify: `docs/runtime/CONFIG.md`
- Modify: `docs/runtime/ENV.md`
- Modify: `docs/services/GATEWAY.md`
- Modify: `docs/services/MARKETPLACE.md`
- Modify: `docs/services/LOCAL_LOGS.md`
- Modify: `docs/surfaces/CLI.md`
- Modify: `docs/surfaces/MCP.md`
- Modify: `config/config.example.toml`

- [ ] Apply the same public-copy rename policy to current docs.

  Rename visible product prose from standalone "Lab" to "Labby". Keep compatibility identifiers unchanged under the policy from `docs/BRANDING.md`.

- [ ] Update `docs/README.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep `labby`, `LAB_*`, `~/.labby`, `lab-apis`, `lab-auth`, `lab-winjob`, and paths containing `/lab/` unchanged.

- [ ] Update `docs/ARCH.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep crate names, module names, environment variables, and repo paths unchanged.

- [ ] Update `docs/TECH.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep literal crate names and dependency names unchanged.

- [ ] Update `docs/OPERATIONS.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep `LAB_*`, `~/.labby`, `lab:read`, `lab:admin`, and `lab_session` unchanged.

- [ ] Update `docs/GATEWAY.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep `lab_admin` and gateway route or catalog identifiers unchanged.

- [ ] Update `docs/runtime/CONFIG.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep config paths such as `~/.labby` and `~/.config/labby` unchanged.

- [ ] Update `docs/runtime/ENV.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep every `LAB_*` environment variable unchanged.

- [ ] Update `docs/services/GATEWAY.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep gateway service ids, action names, and config keys unchanged.

- [ ] Update `docs/services/MARKETPLACE.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep marketplace ids and file paths unchanged.

- [ ] Update `docs/services/LOCAL_LOGS.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep route names, log target names, and `LAB_*` environment variables unchanged.

- [ ] Update `docs/surfaces/CLI.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep command examples using `labby` unchanged.

- [ ] Update `docs/surfaces/MCP.md`.

  Rename visible product prose from standalone "Lab" to "Labby". Keep MCP tool names, resource URIs, and `lab://` resource identifiers unchanged.

- [ ] Fix the stale logging filter example in `config/config.example.toml`.

  Set the example to the actual current binary target:

  ```toml
  [log]
  # Tracing filter directive.
  # Override with LAB_LOG env var.
  # filter = "labby=info,lab_apis=warn,rmcp=warn"       # default: "labby=info,lab_apis=warn,rmcp=warn"
  ```

  Code inspection on 2026-06-12 showed `crates/lab/src/main.rs` defaults to `labby=info,lab_apis=warn,rmcp=warn`; keep the example synchronized with that value unless the code changes first.

- [ ] Leave historical artifacts unchanged.

  Do not edit files under:

  - `docs/sessions/`
  - `docs/reports/`
  - `docs/references/`
  - `.full-review/`

- [ ] Audit current docs for accidental standalone product leftovers.

  ```bash
  rg -n '\bLab\b|Lab'\''s|\bthe Lab\b|\bLab (CLI|gateway|service|services|web UI|docs|binary|installation|process|operator|admin)' docs config \
    --glob '!docs/generated/**' \
    --glob '!docs/sessions/**' \
    --glob '!docs/reports/**' \
    --glob '!docs/references/**'
  ```

  Expected output: only intentional compatibility-policy mentions in `docs/BRANDING.md` and literal identifiers that the policy says to keep.

- [ ] Commit current documentation copy.

  ```bash
  git add docs/README.md docs/ARCH.md docs/TECH.md docs/OPERATIONS.md docs/GATEWAY.md docs/runtime/CONFIG.md docs/runtime/ENV.md docs/services/GATEWAY.md docs/services/MARKETPLACE.md docs/services/LOCAL_LOGS.md docs/surfaces/CLI.md docs/surfaces/MCP.md config/config.example.toml
  git commit -m "Align current docs with Labby branding"
  ```

  Expected output: a commit is created containing only current docs and `config/config.example.toml` branding updates.

## Task 4: Update Web UI Copy

**Files:**
- Modify: `apps/gateway-admin/app/setup/layout.tsx`
- Modify: `apps/gateway-admin/app/setup/core-config/page.tsx`
- Modify: `apps/gateway-admin/app/(admin)/settings/core/page.tsx`
- Modify: `apps/gateway-admin/app/(admin)/docs/page.tsx`

- [ ] Update visible React UI strings in `apps/gateway-admin`.

  Files and exact replacements:

  - `apps/gateway-admin/app/setup/layout.tsx`: change `Lab Setup` to `Labby Setup`.
  - `apps/gateway-admin/app/setup/core-config/page.tsx`: change `Operator-level defaults for the lab process.` to `Operator-level defaults for the Labby process.`
  - `apps/gateway-admin/app/(admin)/settings/core/page.tsx`: change `Core lab settings` to `Core Labby settings`; change `Operator-level lab process defaults.` to `Operator-level Labby process defaults.`
  - `apps/gateway-admin/app/(admin)/docs/page.tsx`: change visible labels `Lab service`, `Lab services available`, `Lab Services`, and `Lab-backed onboarding` to `Labby service`, `Labby services available`, `Labby Services`, and `Labby-backed onboarding`.

- [ ] Keep internal API, route, and type names unchanged unless they are visible copy.

  Do not rename API paths, service ids, environment variables, or generated client keys in this task. In particular, keep `lab_admin` and any `/v1/lab-admin` or equivalent route stable if present.

- [ ] Run the admin app copy audit.

  ```bash
  rg -n '\bLab\b|Lab'\''s|\blab process\b|\blab settings\b|Lab-backed|Lab service|Lab services' apps/gateway-admin \
    --glob '!node_modules/**' \
    --glob '!out/**' \
    --glob '!next-env.d.ts'
  ```

  Expected output: no visible-copy matches except allowed compatibility identifiers and comments that explain retained internal names.

- [ ] Run the app's available static checks.

  ```bash
  pnpm --dir apps/gateway-admin lint
  pnpm --dir apps/gateway-admin test
  ```

  Expected output: both commands exit `0`. If the app does not define one of these scripts, record the exact `Missing script` output in the final implementation notes and run the nearest defined check from `apps/gateway-admin/package.json`.

- [ ] Commit web UI copy.

  ```bash
  git add apps/gateway-admin/app/setup/layout.tsx apps/gateway-admin/app/setup/core-config/page.tsx 'apps/gateway-admin/app/(admin)/settings/core/page.tsx' 'apps/gateway-admin/app/(admin)/docs/page.tsx'
  git commit -m "Rename visible admin UI copy to Labby"
  ```

  Expected output: a commit is created containing only visible admin UI copy updates.

## Task 5: Add A Branding Guardrail

**Files:**
- Create: `scripts/check-labby-branding`
- Modify: `Justfile`
- Modify: `docs/README.md`

- [ ] Add `scripts/check-labby-branding` with this exact content.

  ```bash
  #!/usr/bin/env bash
  set -euo pipefail

  pattern='\bLab\b|Lab'\''s|\bthe Lab\b|\bLab (CLI|gateway|service|services|web UI|docs|binary|installation|process|operator|admin|setup|settings)|Lab-backed'

  set +e
  rg -n \
    "$pattern" \
    README.md docs apps/gateway-admin scripts plugins/labby .claude-plugin .agents config \
    --glob '!docs/BRANDING.md' \
    --glob '!docs/generated/**' \
    --glob '!docs/sessions/**' \
    --glob '!docs/reports/**' \
    --glob '!docs/references/**' \
    --glob '!apps/gateway-admin/out/**' \
    --glob '!apps/gateway-admin/node_modules/**' \
    --glob '!target/**' \
    --glob '!.full-review/**'
  status=$?
  set -e

  if [[ "$status" -eq 0 ]]; then
    echo "Found disallowed standalone Lab branding. Use Labby for public product copy or document the retained identifier in docs/BRANDING.md." >&2
    exit 1
  fi

  if [[ "$status" -gt 1 ]]; then
    exit "$status"
  fi
  ```

- [ ] Make the script executable.

  ```bash
  chmod +x scripts/check-labby-branding
  ```

- [ ] Add a `Justfile` recipe named `branding-check`.

  ```make
  branding-check:
      ./scripts/check-labby-branding
  ```

  Expected behavior: `just branding-check` exits `1` and prints matches when disallowed standalone "Lab" copy remains; it exits `0` when no disallowed matches remain.

- [ ] Update `docs/README.md` to mention `just branding-check` in the docs maintenance section.

  Add this sentence: `Run `just branding-check` before publishing rename-sensitive docs or UI copy.`

- [ ] Commit the guardrail.

  ```bash
  git add scripts/check-labby-branding Justfile docs/README.md
  git commit -m "Add Labby branding guardrail"
  ```

  Expected output: a commit is created containing only the branding check script, `Justfile` recipe, and docs index note.

## Task 6: Regenerate And Verify

**Files:**
- Modify only if generated output changes: `docs/generated/**`

- [ ] Regenerate docs artifacts instead of directly editing `docs/generated/**`.

  ```bash
  just docs-generate
  ```

  Expected output: generated docs are refreshed without errors.

- [ ] Run documentation and branding checks.

  ```bash
  just docs-check
  just branding-check
  git diff --check
  ```

  Expected output: all commands exit `0`.

- [ ] Run the Rust checks that can catch renamed comments, examples, or generated docs embedded in tests.

  ```bash
  cargo fmt --all --check
  cargo check --workspace --all-features
  ```

  Expected output: both commands exit `0`.

- [ ] Run the full default verification.

  ```bash
  just test
  ```

  Expected output: `cargo nextest run --workspace --all-features` exits `0`.

## Task 7: Final Review And Close The Bead

**Files:**
- Review: all files changed by Tasks 1-6

- [ ] Review the final diff.

  ```bash
  git diff --stat
  git diff -- README.md docs/BRANDING.md apps/gateway-admin plugins/labby scripts config Justfile
  ```

  Expected output: only branding/copy/policy/guardrail changes plus generated docs, with no unrelated source rewrites.

- [ ] Commit the rename cleanup.

  ```bash
  if ! git diff --quiet -- docs/generated; then
    git add docs/generated
    git commit -m "Regenerate docs after Labby branding rename"
  fi
  ```

  Expected output: a generated-docs commit is created only when `docs/generated/**` changed. If no generated docs changed, the command prints nothing and exits `0`.

- [ ] Close the bead.

  ```bash
  BEAD_ID="$(rg -o 'lab-[a-z0-9]+' /tmp/labby-rename-bead.txt | head -1)"
  test -n "$BEAD_ID"
  bd close "$BEAD_ID" --reason="Completed Labby branding rename policy, copy cleanup, guardrail, and verification."
  ```

  Expected output: the bead is marked closed.

## Out Of Scope

- Renaming the GitHub repository from `jmagar/lab`.
- Renaming Rust crates from `lab-*` to `labby-*`.
- Renaming `~/.labby`, `~/.config/labby`, `LAB_*`, `lab:read`, `lab:admin`, `lab_session`, or `lab_admin`.
- Renaming historical session logs, reports, archived references, or old review artifacts.
- Changing API, MCP, OAuth, cookie, or config compatibility contracts without aliases and a migration guide.

## Final Review Checklist

- [ ] `README.md` introduces Labby as the product and does not use standalone "Lab" as product copy.
- [ ] Install scripts, plugin descriptions, marketplace copy, and skill docs say Labby where users see product naming.
- [ ] Current docs use Labby for public prose and retain `lab` only for documented compatibility identifiers.
- [ ] Web UI visible strings use Labby.
- [ ] `docs/BRANDING.md` explains every retained `lab` identifier class.
- [ ] `just branding-check`, `just docs-check`, `git diff --check`, and `cargo check --workspace --all-features` pass.
