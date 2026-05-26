## Title

**feat: core plugin + Claude Code plugin lifecycle on top of the locked Setup/Settings UI**

**Type:** `feat`
**Service(s) affected:** `setup` (extends with plugin lifecycle actions); `cli` (env-aware help, marketplace generator, PATH symlink); `gateway-admin` (extends Setup wizard step 4); _new_ marketplace tree under `marketplace/` produced from `PluginMeta`
**Priority:** `p1-high`
**Depends on:** `lab-bg3e.3`, `.4`, `.5` (currently marked CLOSED but not implemented — see Prerequisite Status below)

---

## Problem / Motivation

The Setup/Settings refactor in `lab-bg3e` already specs and plans the right onboarding UX:

- Locked spec: [`docs/superpowers/specs/2026-04-25-setup-settings-design.md`](../superpowers/specs/2026-04-25-setup-settings-design.md) — 7-step wizard at `/setup`, settings rail at `/settings`, `setup.draft.set`/`setup.draft.commit` write path, schema-driven `ServiceForm` over `UiSchema`, `doctor.service_probe` Test buttons, `/dev/api/nodeinfo` for re-run pre-population.
- React plans: [`docs/superpowers/plans/2026-04-26-setup-wizard.md`](../superpowers/plans/2026-04-26-setup-wizard.md) (14 tasks), [`docs/superpowers/plans/2026-04-26-settings-page.md`](../superpowers/plans/2026-04-26-settings-page.md) (9 tasks).
- HTML mockups: `~/.superpowers/brainstorm/content/setup.html`, `~/.superpowers/brainstorm/content/settings.html`.

What that work does not cover:

1. **Distribution and Claude Code integration** — how a homelab user installs the binary, how they end up with one MCP server per service in Claude Code, and how the web UI turns into actual `claude plugin install` calls.
2. **A wizard mode for plugin users** — bg3e was implicitly designed for the operator who runs the full `lab` stack (web UI, HTTP API, OAuth, multi-node fleet, etc.). A user who installed `lab-plex` from the Claude Code marketplace doesn't care about Surfaces, Nodes, OAuth, or PreFlight; they want to enter Plex creds and have the MCP server start working. Showing them all of that pushes operator-grade complexity onto a plugin-grade use case.

This plan adds both, on top of bg3e. The wizard becomes one surface with two **modes** — `plugin` (default when invoked from a plugin slash command, hides operator surfaces) and `full` (default for standalone `labby setup`, shows everything). Plugin-mode users can promote to full at any time; full-mode users see nothing different from what bg3e already specs.

This is not a parallel "plugin wizard." It is the same wizard with a render-time gate driven by `setup.state.mode`. Adding the gate is cheap; building a parallel UI would duplicate `ServiceForm`, the stepper, and the API surface for no reason.

### Prerequisite Status (audit performed 2026-04-30)

| Bead | Bead status | Code state | Notes |
|---|---|---|---|
| `lab-bg3e.1` UiSchema/PluginMeta | CLOSED | **Shipped** (`crates/lab-apis/src/core/plugin_ui.rs`, `EnvVar.ui` field, commit `78a8f7f7`) | Ready to use. |
| `lab-bg3e.2` doctor dispatch | CLOSED | **Shipped** (full 3-tier `crates/lab/src/dispatch/doctor/`, commit `0c7f4cbc`) | `service_probe`, `audit.full` SSE both available. |
| `lab-bg3e.3` setup dispatch + env_merge | CLOSED | **Not implemented.** No `crates/lab/src/dispatch/setup/`, no `crates/lab-apis/src/setup/`, no `crates/lab/src/config/env_merge.rs`. | Closed prematurely. Reopen + ship before this plan starts. |
| `lab-bg3e.4` /setup wizard React | CLOSED | **Not implemented.** `app/(admin)/setup/page.tsx` is still a legacy credential viewer; no `(wizard)` route group; no `ServiceForm`. | Closed prematurely. Reopen + ship before this plan's wizard extension starts. |
| `lab-bg3e.5` /settings nested-rail | CLOSED | **Not implemented.** `/settings` is still the gateway-fleet posture dashboard. | Closed prematurely. Reopen + ship before this plan's settings extension starts. |

The first acceptance criterion below is to reopen the three premature closures and update them with refreshed status.

---

## Acceptance Criteria

### Prerequisite cleanup
- [ ] `lab-bg3e.3`, `.4`, `.5` reopened with comments noting close-without-land. Spec's "Implementation Notes" updated to read "Phases 1+2 shipped; phases 3–5 in progress."

### Wizard modes
- [ ] `setup.state` (already specced in bg3e.3) gains a `mode: "plugin" | "full"` field. Mode is set by the wizard URL (`?mode=plugin` or `?mode=full`) and persisted to `~/.lab/.setup-state.json` so re-runs remember the user's choice.
- [ ] **`plugin` mode** renders only:
  - Step 1 (Welcome) — one-line copy: "Configure the services you've installed plugins for."
  - Step 4 (Services) — only the services for which a plugin is installed (queried via `setup.installed_plugins`). Other services are hidden, not just collapsed.
  - Step 7 (Finalize).
  - Steps 2 (Core Config), 3 (PreFlight 1), 5 (Surfaces), 6 (PreFlight 2) are skipped in plugin mode. The stepper compresses to a 3-dot strip.
- [ ] **`full` mode** renders the complete 7-step flow per the locked bg3e spec. No change to existing behavior.
- [ ] Both modes share the same components, hooks, and API. Mode is checked in the parent wizard shell; individual step components do not branch on mode.
- [ ] Plugin mode shows a **"Show advanced setup"** affordance in the topbar. Clicking it switches to full mode in place (no reload), unlocks the hidden steps, and persists the new mode. There is no demote-to-plugin affordance — once a user has explicitly opted into operator complexity, they keep it. (They can edit `~/.lab/.setup-state.json` to undo this; intentional friction.)
- [ ] `labby setup --mode plugin` and `labby setup --mode full` are CLI flags. The slash command `/setup-core` (shipped by the core plugin) invokes `labby setup --mode plugin`. Standalone `labby setup` defaults to `full`.
- [ ] The `/settings` rail follows the same gate: in plugin mode, only the Services panel is visible; Core/Doctor/Extract/v2-stubs are hidden behind the same advanced affordance.

### Plugin lifecycle in setup
- [ ] `setup.install_plugin` action lands inside the existing setup dispatch service from bg3e.3. Shells out `claude plugin install <id>@<org> --scope <user|project>`, parses result, returns structured envelope. Destructive.
- [ ] `setup.uninstall_plugin` action — same shape, calls `claude plugin uninstall`. Destructive.
- [ ] `setup.installed_plugins` action — read-only `claude plugin list` parser; powers re-run mode and the wizard's per-service Enabled badge.
- [ ] These three actions are **only mounted on the HTTP API when bound to a loopback address**. Non-loopback bind logs a single startup line that they are skipped. They are unconditionally available over stdio MCP and the CLI shim.
- [ ] Package allowlist: requests with package IDs not matching a configured prefix (`LAB_PLUGIN_ALLOWLIST`, default `@lab,@yourorg`) return `package_not_allowlisted`.
- [ ] All three actions emit dispatch events with `surface`, `service=setup`, `action`, `elapsed_ms`, plus `package_id` and `scope` for install/uninstall. **No env-var values, no token material in any log.**

### Wizard integration
- [ ] Step 4 of the wizard (Services, per the locked spec) gains a per-service "Enable in Claude Code" toggle next to the existing field group. State for the toggle is sourced from `setup.installed_plugins`. Toggling on calls `setup.install_plugin` after the service's draft is committed; toggling off calls `setup.uninstall_plugin`. The wizard never exposes the package ID directly — it derives it from the service name + the configured org prefix.
- [ ] Step 7 (Finalize) adds a one-line summary of plugin actions taken in this session ("Installed: plex@lab, radarr@lab. Uninstalled: none.") below the existing finalize summary.
- [ ] The settings rail (`/settings`) Services panel mirrors the toggle. State stays consistent across `/setup` and `/settings` because both read `setup.installed_plugins`.

### Distribution
- [ ] `labby marketplace generate --out <dir>` is a real CLI command. Reads every compiled-in service's `PluginMeta`, emits a marketplace tree:
  - `<dir>/lab-core/` — plugin manifest, `bin/labby` (built binary), `commands/setup-core.md` slash command, README, optional setup skill.
  - `<dir>/lab-<service>/` for each service — plugin manifest, `commands/install-core.md` slash command, `.mcp.json` containing `{ "command": "<absolute-path-to-core-binary>", "args": ["mcp", "--services", "<service>"] }` where the absolute path is `~/.claude/plugins/lab-core/bin/labby` (escaped per the user's home), README listing the env vars and link-back to `/setup`.
- [ ] The generator runs in CI on each release tag and the resulting tree is published as a release artefact.
- [ ] `bin/labby` inside `lab-core` is a copy of the freshly built release binary, not a symlink — Claude Code's plugin clone needs the file present.
- [ ] The generator is **not** a feature flag and is **not** the only allowed plugin source. Hand-written plugin overrides in the marketplace repo are preserved; the generator only writes service plugins it owns.

### Core plugin
- [ ] `marketplace/lab-core/commands/setup-core.md` slash command runs `labby setup --mode plugin`. The user types `/setup-core` in Claude Code and the wizard opens in plugin mode (Services + Finalize only).
- [ ] `marketplace/lab-core/commands/setup-core-advanced.md` slash command runs `labby setup --mode full`. Documented in the core plugin's README as the operator entry point.
- [ ] `marketplace/lab-core/skills/install-binary/` (or equivalent) handles the PATH question:
  - If `~/.local/bin/labby` does not exist or doesn't point at the core plugin's `bin/labby`, offer to create the symlink. Skip silently otherwise.
  - On failure to symlink (permissions, etc.), fall back to printing one line telling the user the absolute path of the core binary and that `claude plugin install` will use it directly via service plugins' `.mcp.json`.
  - This skill **never** auto-installs other plugins, never edits Claude Code config, never restarts anything.
- [ ] No SessionStart hook in the core plugin auto-spawns a webserver. Setup runs only when the user invokes `/setup-core` (or `labby setup` from a terminal).

### Service plugins
- [ ] Each `marketplace/lab-<service>/` plugin contains a `commands/install-core.md` slash command. When the user has installed the service plugin without core, `/install-core` prints a one-liner `claude plugin install lab-core@<org>` and asks the user to restart. The slash command does not shell out itself.
- [ ] `marketplace/lab-<service>/.mcp.json` references the absolute path to the core binary; **no PATH dependency**.
- [ ] Each service plugin's README lists the required env vars (sourced from `PluginMeta.required_env`) and a one-liner pointing at `/setup-core` to fill them in.

### Env-aware CLI
- [ ] `lab help` shows only services whose required env vars are present. Always-visible operator commands (`init`, `setup`, `doctor`, `plugins`, `gateway`, `help`, `completions`, `scaffold`, `audit`, `marketplace`) are never filtered.
- [ ] `LAB_SHOW_ALL=1` and `lab help --all` bypass the filter.
- [ ] The MCP `lab://catalog` resource uses the same filter (registry filter). `--services foo,bar` continues to override at `labby serve` / `labby mcp` level.
- [ ] (Optional polish) `labby --help` (clap-derived, top-level) honors the same filter via `Cli::command_for_update()` + `mut_subcommand("<svc>", |c| c.hide(true))`.

### Verification
- [ ] All-features build passes (`just build`).
- [ ] All-features tests pass (`just test`).
- [ ] Clippy clean (`just lint`).
- [ ] `cargo deny` clean (no new restricted licences from `webbrowser` etc. — bg3e.3 already pulled the dep).

---

## Proposed Approach

This plan extends the bg3e setup service with three plugin-lifecycle actions, integrates them into the existing wizard, and adds three independent pieces (marketplace generator, core/service plugin trees, env-aware help). It does not redesign the Setup or Settings UI — those are owned by the locked bg3e spec and the two React plans.

### lab-apis changes

None. `setup` lives in dispatch only (no `lab-apis/src/setup/` per bg3e.3's locked decisions, despite an earlier draft suggesting otherwise). Plugin lifecycle is internal to the binary.

- [ ] N/A — no lab-apis changes.

### dispatch layer changes

#### `crates/lab/src/dispatch/setup/` (extending bg3e.3)

bg3e.3 must land first with `setup.state`, `setup.schema.get`, `setup.draft.get`, `setup.draft.set`, `setup.draft.commit`, `setup.finalize`. This plan adds three more actions to the same module.

- [ ] `catalog.rs` — append three `ActionSpec` entries:
  - `installed_plugins` (read) — params `{ scope?: "user" | "project" }`. Calls `claude plugin list` and parses output; returns `{ plugins: [{ id, scope, version? }] }`.
  - `install_plugin` (**destructive**) — params `{ service: string, scope?: "user" | "project" }`. Resolves `service` to `lab-<service>@<org>` via the configured allowlist prefix, then shells out. Refuses if `service` is not a registered service in this binary.
  - `uninstall_plugin` (**destructive**) — same shape; shells `claude plugin uninstall`.
- [ ] `params.rs` — `InstalledPluginsParams`, `InstallPluginParams`, `UninstallPluginParams` with serde-derived deserialization and validation hooks for the allowlist check.
- [ ] `dispatch.rs` — three new match arms calling new client methods.
- [ ] `client.rs` — add `installed_plugins`, `install_plugin`, `uninstall_plugin` methods using `tokio::process::Command`. Timeout via `LAB_PLUGIN_TIMEOUT_SECS` (default 300s). Output parsing returns `claude_cli_unavailable` if the binary is missing, `plugin_install_failed` with stderr summary on non-zero exit. **stderr is summarized to one line in the envelope; full stderr is logged at WARN, with secret-suffix redaction applied to defend against marketplace URLs that embed tokens.**

#### Surface enforcement

The "loopback-only over HTTP" rule lives at the API router, not in the dispatch module — keep dispatch surface-neutral so the same actions remain reachable over stdio MCP and the CLI shim. The router:

- [ ] `crates/lab/src/api/services/setup.rs` — when registering `install_plugin` and `uninstall_plugin` and `installed_plugins` routes, check whether the bind host is loopback (existing `is_loopback_host` helper from `serve.rs`). On non-loopback bind, skip route mounting and emit a single startup log: `setup.plugin routes skipped — non-loopback bind`. The other `setup.*` routes (state, schema, draft, finalize) mount unconditionally.

### CLI changes

- [ ] `crates/lab/src/cli/help.rs` — env-aware filter:
  - Add `configured_services(registry: &ToolRegistry) -> Vec<&str>` walking each service's `PluginMeta.required_env`. A service is "configured" iff every `required: true` env var has a non-empty value in `std::env::vars()`. Optional vars don't count.
  - Default filter applied to `lab help`. Operator commands listed in the acceptance criteria are never filtered.
  - `LAB_SHOW_ALL=1` and `--all` bypass.
- [ ] `crates/lab/src/cli.rs` — optional polish: `Cli::command_for_update()` + `mut_subcommand` to hide unconfigured service subcommands from `labby --help`. Behind the same `LAB_SHOW_ALL` env / `--all` flag. Mark this as the last AC item; it can ship in a follow-up PR.
- [ ] `crates/lab/src/cli/setup.rs` (already created in bg3e.3) — add three Tier-2 dispatch shims for the new actions: `labby setup installed_plugins`, `labby setup install_plugin`, `labby setup uninstall_plugin`. Standard destructive flag handling per `crates/lab/src/cli/CLAUDE.md`.
- [ ] `crates/lab/src/cli/marketplace.rs` (existing) — add a new subcommand `labby marketplace generate --out <dir> [--org <prefix>] [--binary <path>]`:
  - Iterates `build_default_registry()` and emits the marketplace tree.
  - `--org` defaults to the value baked in at build time (cargo env var `LAB_PLUGIN_ORG`, default `lab`).
  - `--binary` defaults to `target/release/labby` from the workspace root.
  - The generator copies the binary into `<out>/lab-core/bin/labby`, sets executable bits, and writes templated `plugin.json` / `.mcp.json` / `commands/*.md` files.
  - Tests assert that round-tripping `PluginMeta` → generator → re-parse yields the same env-var lists.

### API changes

- [ ] `crates/lab/src/api/services/setup.rs` — three new handlers calling shared `helpers::handle_action`:
  - `GET /v1/setup/plugins` → `installed_plugins`
  - `POST /v1/setup/plugins/install` → `install_plugin`
  - `POST /v1/setup/plugins/uninstall` → `uninstall_plugin`
- [ ] Loopback gating per the dispatch section above. Existing setup routes (`state`, `schema`, `draft`, `finalize`) mount unconditionally and are unchanged by this plan.
- [ ] `crates/lab/src/api/router.rs` — no new top-level routes; only the conditional mount inside the setup route group.

### Web UI changes (gateway-admin)

These extend the bg3e.4 wizard and bg3e.5 settings rail; they are not new pages.

- [ ] `apps/gateway-admin/components/wizard/setup-wizard.tsx` (created by bg3e.4) — read `mode` from URL query param + `setup.state` response. In `plugin` mode, render only steps 1, 4, 7; in `full` mode, render all 7. Mode is held in the wizard's state hook and threaded through to `<Stepper>` so the strip compresses correctly. Persist mode changes to `setup.state` via a new `setup.state.set_mode` action (lightweight extension of bg3e.3, behind the same loopback gate as plugin lifecycle).
- [ ] `apps/gateway-admin/components/wizard/show-advanced-toggle.tsx` — new component. Pinned in the topbar in plugin mode only. Clicking switches to full mode in place. No demote-to-plugin affordance.
- [ ] `apps/gateway-admin/components/settings/settings-rail.tsx` (created by bg3e.5) — same mode gate. Plugin mode shows only the Services panel + a "Show advanced setup" link. Full mode shows the full rail.
- [ ] `apps/gateway-admin/components/wizard/services-step.tsx` (created by bg3e.4) — add an "Enable in Claude Code" toggle to each `<ServiceForm>` row. The toggle is disabled until the form's draft is saved (`setup.draft.set` succeeded). On enable: call `setup.install_plugin`, optimistically flip the toggle, roll back on error with toast. On disable: call `setup.uninstall_plugin`. Initial state from `useInstalledPlugins()`.
- [ ] `apps/gateway-admin/components/settings/services-panel.tsx` (created by bg3e.5) — same toggle, same data source.
- [ ] `apps/gateway-admin/lib/api/setup.ts` — add `installed_plugins`, `install_plugin`, `uninstall_plugin` clients alongside the existing `draft.*` calls.
- [ ] `apps/gateway-admin/components/wizard/finalize-step.tsx` — append the one-line plugin summary.
- [ ] No new design system primitives. Aurora tokens, `Switch`, `Spinner`, `Card`, toast — all already in place.

### Marketplace generator + core plugin tree

- [ ] `crates/lab/src/cli/marketplace.rs::run_generate(args)` — emits to `--out`:

```
<out>/
├── plugin-marketplace.json          # generated; lists every plugin
├── lab-core/
│   ├── plugin.json
│   ├── README.md
│   ├── bin/
│   │   └── lab                      # copied release binary
│   ├── commands/
│   │   └── setup-core.md            # /setup-core → "labby setup"
│   └── skills/
│       └── install-binary/
│           ├── SKILL.md
│           └── ...
└── lab-<service>/                    # one per registered service
    ├── plugin.json
    ├── README.md                    # required env from PluginMeta
    ├── .mcp.json                    # absolute path to core binary
    └── commands/
        └── install-core.md          # /install-core
```

- [ ] The slash command bodies and READMEs are templated; templates live under `crates/lab/templates/marketplace/` and are embedded with `include_str!` so the binary is self-sufficient.
- [ ] `plugin-marketplace.json` schema follows the Claude Code plugin marketplace spec; the generator validates against it before writing.

### Config / env vars

| Var | Required | Description |
|-----|----------|-------------|
| `LAB_SHOW_ALL` | no | When `1`/`true`, disables env-aware filtering of `lab help` and the MCP catalog. |
| `LAB_PLUGIN_ALLOWLIST` | no | Comma-separated package-ID prefixes accepted by `setup.install_plugin` (e.g. `lab,yourorg`). Defaults to a hard-coded org prefix shipped with the binary at build time. |
| `LAB_PLUGIN_TIMEOUT_SECS` | no | Hard ceiling on `claude plugin install/uninstall/list` calls. Default 300. |
| `LAB_CLAUDE_BIN` | no | Path to the `claude` CLI; defaults to PATH lookup. |
| `LAB_PLUGIN_ORG` (build-time) | no | Cargo env var consumed by the binary at compile time to set the default org prefix used by the marketplace generator and the install allowlist. |

New `config.toml` keys (optional, all surfaced in `setup.schema.get` per bg3e.3):

- `[plugins] default_scope` — `user` (default) or `project`. UI surfaces this in /setup as a single radio.
- `[plugins] allowlist` — array, mirrors `LAB_PLUGIN_ALLOWLIST`.

### Other files

- [ ] `crates/lab/src/registry.rs` — no change. Setup is already registered by bg3e.3.
- [ ] `crates/lab/src/mcp/services.rs` / `mcp/services/setup.rs` — no change beyond the new actions surfacing automatically through the existing thin bridge.
- [ ] `docs/PLUGINS.md` — new doc, ~150 lines, covering: marketplace tree shape, core/service split, generator usage, the loopback-only constraint on plugin lifecycle actions, the `/install-core` and `/setup-core` slash commands, and the absolute-path `.mcp.json` rationale.
- [ ] `docs/ERRORS.md` — append `package_not_allowlisted`, `claude_cli_unavailable`, `plugin_install_failed`. (Already noted in earlier draft of this plan; ensure the additions land alongside whatever bg3e.3 contributes.)
- [ ] `docs/DISPATCH.md` — note the loopback-only mounting rule for `setup.install_plugin`/`uninstall_plugin`/`installed_plugins`. The rest of `setup.*` is unchanged.
- [ ] `docs/superpowers/specs/2026-04-25-setup-settings-design.md` — append a "Plugin lifecycle (follow-up)" section pointing at this doc; do not edit the locked sections.
- [ ] `Justfile` — add `just marketplace` running `cargo run --release -- marketplace generate --out target/marketplace`.
- [ ] CI release workflow — invoke `just marketplace` and upload `target/marketplace/` as a release artefact.

---

## Observability

Per `docs/OBSERVABILITY.md`, plus bg3e.3's existing setup-service redaction rules.

- [x] Dispatch event for every action with `surface`, `service=setup`, `action`, `elapsed_ms`, `kind` on error.
- [x] `setup.install_plugin` / `setup.uninstall_plugin` log intent (`package_id`, `scope`) and outcome. **Never log env values, never log captured stderr unredacted** (existing secret-suffix redaction applies). Full stderr is logged at WARN; the envelope `message` is summarized to one line.
- [x] `setup.installed_plugins` is a read; logged at DEBUG only by default (the wizard polls it).
- [x] When the API router skips loopback-gated routes due to non-loopback bind, emit one INFO log line at startup with the bind host.
- [x] HTTP request lifecycle (`request.start`/`finish`/`error`) covered by existing middleware.

---

## Error Handling

- [x] New stable `kind` values added to `docs/ERRORS.md`:
  - `package_not_allowlisted` — install/uninstall package ID outside `LAB_PLUGIN_ALLOWLIST`.
  - `claude_cli_unavailable` — `claude` binary not found / not executable / version mismatch.
  - `plugin_install_failed` — `claude plugin install` returned non-zero; one-line stderr summary in `message`.
  - `plugin_uninstall_failed` — symmetric.
  - `unknown_service` — install/uninstall reference a service not registered in this binary (caught before shell-out).
- [x] MCP and HTTP error envelopes share the existing `ToolError → IntoResponse` mapping.
- [x] No new panics; every `tokio::process::Command` call returns `Result<_, ToolError>` with explicit kind mapping.
- [x] Loopback-mount skip is **not** an error — it's a startup configuration decision logged at INFO. A non-loopback caller hitting a non-mounted route gets the standard 404, not a structured envelope.

---

## Destructive Actions

| Action | Why destructive | Elicitation / `-y` required |
|--------|----------------|-----------------------------|
| `setup.install_plugin` | Triggers a marketplace fetch + Claude Code config mutation. Loopback-only over HTTP; allowlist-gated. | yes |
| `setup.uninstall_plugin` | Mutates the user's Claude Code plugin state. Loopback-only over HTTP. | yes |

CLI shims honor `-y` / `--no-confirm` / `--dry-run` per `crates/lab/src/cli/CLAUDE.md`. The web UI counts the "Enable in Claude Code" toggle plus the underlying credentials being saved as the user's confirmation; no extra modal is required.

`installed_plugins` is read-only and not destructive.

---

## Testing Plan

- [x] Unit tests in `crates/lab/src/dispatch/setup/` covering:
  - Allowlist enforcement (positive + negative).
  - Unknown-service rejection before shell-out.
  - Stderr redaction on `plugin_install_failed`.
  - Param parsing (missing required, scope enum out-of-range).
- [x] Integration test using a `LAB_CLAUDE_BIN`-overridable shim: a tiny shell script that emulates `claude plugin install/list/uninstall` with deterministic JSON output. Verifies happy path, exit-non-zero, missing-binary, and timeout.
- [x] Integration test for the loopback-mount gate: spin up the API on `0.0.0.0:0`, assert install/uninstall/list routes return 404; spin up on `127.0.0.1:0`, assert they exist.
- [x] Snapshot test for the marketplace generator: run against the in-process registry, golden-compare the generated `plugin.json` / `.mcp.json` / `README.md` for `lab-core` + `lab-radarr`. Refresh on `cargo insta accept`.
- [x] Snapshot test for `lab help` env-aware filter: empty env, only operator commands appear; `RADARR_URL` + `RADARR_API_KEY` set, radarr appears.
- [x] React tests (Vitest, per the bg3e.4/.5 plans) for the new toggle: optimistic flip + rollback on error, disabled state when draft not yet saved, badge renders from `useInstalledPlugins()`.

### Test scenarios

| Scenario | Type | Location |
|----------|------|----------|
| `install_plugin` allowlist reject → `package_not_allowlisted` | unit | `crates/lab/src/dispatch/setup/` |
| `install_plugin` unknown service → `unknown_service` | unit | `crates/lab/src/dispatch/setup/` |
| `install_plugin` happy path with shim | integration | `crates/lab/src/dispatch/setup/` |
| `install_plugin` shim returns non-zero → `plugin_install_failed` | integration | `crates/lab/src/dispatch/setup/` |
| `claude` binary missing → `claude_cli_unavailable` | integration | `crates/lab/src/dispatch/setup/` |
| Plugin routes 404 on non-loopback bind | integration | `crates/lab/src/api/services/setup.rs` |
| Plugin routes 200 on loopback bind | integration | `crates/lab/src/api/services/setup.rs` |
| Marketplace generator output stable across runs | snapshot | `crates/lab/src/cli/marketplace.rs` |
| `lab help` env filter | snapshot | `crates/lab/src/cli/help.rs` |
| Wizard toggle: optimistic flip + rollback | unit (Vitest) | `apps/gateway-admin/components/wizard/services-step.test.tsx` |
| Settings toggle mirrors wizard state | unit (Vitest) | `apps/gateway-admin/components/settings/services-panel.test.tsx` |

---

## Open Questions

1. Default org prefix. `lab` is the obvious choice but is also the binary name and may collide with unrelated packages in a global namespace. Proposal: bake `LAB_PLUGIN_ORG` at build time; ship with `lab` for OSS but allow forks to set their own without recompiling the marketplace.
2. Plugin scope default. The Setup wizard chose `user` historically (homelab tools shouldn't follow project directories). Confirm and surface as a single radio in /setup with `[plugins] default_scope` persisted.
3. Whether `install_plugin` should also accept a `version` param. `claude plugin install foo@bar@1.2.3` style. Proposal: defer; ship without versioning, add later behind a `version?: string` param.
4. `labby marketplace generate` ergonomics. Should it default to writing into a sibling repo (e.g. `../lab-marketplace/`) when run from the workspace? Proposal: no — explicit `--out` only. CI ergonomics live in the release workflow.
5. Whether the env-aware `lab help` filter should also be the default for the MCP catalog when no `--services` is passed. Proposal: yes — same filter, same env vars, same `LAB_SHOW_ALL` escape hatch. Documented behavior change in `docs/CONVENTIONS.md`.
6. SessionStart hook in the core plugin. The earlier draft had one; we explicitly removed it because hook-driven webserver spawning is brittle. Confirm we ship core with **no** SessionStart hook — `/setup-core` is the only entry point.
7. Plugin-mode service filtering. Step 4 in plugin mode shows only services with installed plugins. What if the user has Plex creds in `~/.lab/.env` but never installed `lab-plex`? Proposal: still show the service in plugin mode if it's already configured, with the toggle off, so the user can choose to install the plugin or clear the leftover env. Hide it only if it's neither installed nor configured.
8. Mode for re-runs. If a user ran `/setup-core` (plugin mode) once, then later runs standalone `labby setup`, do they get plugin or full mode? Proposal: the CLI flag wins. `labby setup` with no flag picks up the persisted mode from `~/.lab/.setup-state.json`, defaulting to `full` if the state file says nothing. Users who want full from a plugin install always have `/setup-core-advanced`.
9. Whether `setup.state.set_mode` should be available over stdio MCP at all. Proposal: yes — it's not destructive (no fs write outside `~/.lab/.setup-state.json`), and stdio MCP callers (i.e. Claude Code itself) may legitimately need to flip the wizard mode programmatically. Loopback gate stays only on the actual plugin lifecycle actions.

---

## Out of Scope

- Re-doing the Setup or Settings UI. Those are owned by bg3e and the two React plans. This doc only adds a mode gate, a per-row "Enable in Claude Code" toggle, and a one-line summary on Finalize.
- A separate plugin-mode wizard. Plugin mode is a render-time gate on the existing wizard, not a new component tree. If someone proposes forking the wizard for plugin users, that's out of scope.
- A standalone `claude` dispatch service. Plugin lifecycle lives in `setup` because nothing else needs it. If a future use case appears (e.g. a diagnostic skill listing installed plugins), promote then.
- Auto-installing the core plugin from a service plugin. `/install-core` prints a command; the user runs it. No hooks, no auto-install.
- Auto-restarting Claude Code after a plugin install. The web UI tells the user to restart; the user does it.
- Bundling plugins inside the binary. The marketplace remains the distribution channel; the binary's `labby marketplace generate` produces the tree from `PluginMeta` so the marketplace stays in sync without hand-maintenance.
- Windows-specific PATH handling beyond best-effort. Linux + macOS are the supported target platforms; Windows users get the absolute-path fallback only.
- Light-mode polish for the new toggle. Inherits whatever bg3e.4/.5 ship.
- Support for marketplaces other than `claude plugin install <id>@<org>`. Other distribution channels (cargo, brew, deb) are not addressed.

---

## References

- [`docs/superpowers/specs/2026-04-25-setup-settings-design.md`](../superpowers/specs/2026-04-25-setup-settings-design.md) — locked Setup + Settings spec. Read this first.
- [`docs/superpowers/plans/2026-04-26-setup-wizard.md`](../superpowers/plans/2026-04-26-setup-wizard.md) — 14-task React plan for `/setup`.
- [`docs/superpowers/plans/2026-04-26-settings-page.md`](../superpowers/plans/2026-04-26-settings-page.md) — 9-task React plan for `/settings`.
- `~/.superpowers/brainstorm/content/setup.html`, `settings.html` — interactive Tier-1 mockups (Aurora-styled, pre-populated from `/dev/api/nodeinfo`).
- `crates/lab-apis/src/core/plugin_ui.rs` — `UiSchema` type from bg3e.1, source of truth for service field metadata.
- `crates/lab/src/dispatch/doctor/` — bg3e.2 doctor service, source of `service_probe` and `audit.full`.
- `crates/lab/src/api/router.rs:565` — existing `/dev/api/nodeinfo` endpoint, consumed by the wizard for re-run pre-population.
- `crates/lab/src/cli/serve.rs:771` — existing `is_loopback_host` helper to reuse for the route gate.
- `crates/lab/src/cli/serve.rs:784` — existing `filter_registry` pattern; the env-aware `lab help` filter follows it.
- `docs/DISPATCH.md` — dispatch layer contract; this plan adds one new invariant (loopback-only mount for plugin lifecycle).
- `docs/OBSERVABILITY.md` — logging requirements; bg3e.3's redaction rules apply unchanged.
- `docs/ERRORS.md` — canonical error vocabulary; new kinds listed in the Error Handling section.
- `docs/design/SERIALIZATION.md` — output boundary rules; envelope shape unchanged.
- Bead tree: `lab-bg3e` epic and children `.1`–`.5` (close-without-land for `.3`/`.4`/`.5` flagged in Prerequisite Status above).
