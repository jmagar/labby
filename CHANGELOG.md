# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

(empty)

---

## [0.15.2] — 2026-05-11

### Highlights

- **gateway-admin server terminology**: the admin UI now labels managed MCP upstreams as Servers instead of Gateways across navigation, dashboards, lists, detail pages, dialogs, docs, settings, protected routes, and tests.
- **OAuth refresh resource reuse**: refresh-token grants now preserve the stored resource when the request omits `resource`, while still rejecting explicit mismatched resources.

| Commit | Change |
|--------|--------|
| *(this)* | fix(gateway-admin): rename gateways to servers in UI |

### Version bumps

- Rust workspace: `0.15.1 → 0.15.2`
- Gateway admin package: `0.15.1 → 0.15.2`

---



## [0.15.1] — 2026-05-11

### Highlights

- **gateway protected-route editing**: editing an existing gateway now hydrates the protected-route path and auth mode from persisted protected route state, including late-arriving route data and stdio gateways with OAuth-protected endpoints.

| Commit | Change |
|--------|--------|
| *(this)* | fix(gateway): restore protected route edit state |

### Version bumps

- Rust workspace: `0.15.0 → 0.15.1`
- Gateway admin package: `0.15.0 → 0.15.1`

---

## [0.15.0] — 2026-05-05

### Highlights

- **gateway-admin mobile chat**: chat message bubbles now preserve long prose, markdown, code blocks, and action traces inside the mobile viewport; the copy affordance remains reachable on touch devices.
- **agent running state**: active runs now show as an inline assistant working bubble instead of a top-of-conversation status banner, with tests covering the streaming and waiting-for-permission conditions.
- **chat state mockups**: adds the assistant working-bubble mockup used to compare running-state placement options.

| Commit | Change |
|--------|--------|
| *(this)* | feat: optimize mobile chat running state |

### Version bumps

- Rust workspace: `0.14.0 → 0.15.0`
- Gateway admin package: `0.14.0 → 0.15.0`

---

## [0.14.0] — 2026-05-04

### Highlights

- **ACP sessions**: prompt dispatch now replaces the default "New session" title with a bounded title derived from the user's prompt, and unfinished provider exits now preserve provider-error details instead of always emitting the generic no-stop-reason event.
- **gateway-admin chat UI**: reasoning summaries and agent actions now render as separate panels; action traces keep grouped read/search/edit/command summaries, and a render test guards against folding actions back into reasoning.
- **Vibin GitHub workflow consolidation**: GitHub review and CI skills move under the Vibin plugin, with marketplace and plugin metadata updated to describe the expanded workflow surface.

| Commit | Change |
|--------|--------|
| *(this)* | feat: improve ACP session titles and separate chat reasoning from actions |

### Version bumps

- Rust workspace: `0.13.1 → 0.14.0`
- Gateway admin package: `0.13.1 → 0.14.0`

---

## [0.13.1] — 2026-05-04

### Highlights

- **gateway-admin chat UI**: agent tool calls are now compact by default — summary text, file preview snippets, and category/status labels moved behind the expand chevron; file paths shown inline under the label instead of as chips; skill labels now show the skill name rather than full description text

| Commit | Change |
|--------|--------|
| `d62b33bf` | fix: validate acp smoke stream output |
| `5743e804` | fix(gateway-admin): compact agent action tool calls — collapse summary/preview by default, inline paths, extract skill name from label |

---

## [0.13.0] — 2026-05-04

| Commit | Change |
|--------|--------|
| `60939ce2` | fix(nodes): close only on rejected initialize, not on pre-init method errors |
| `f619f025` | fix(lab-p760): wrap all sync stash dispatch arms in spawn_blocking |
| `2270470f` | fix(lab-qytb): provider.pull writes revision meta inside component lock |
| `5f409c05` | fix(lab-gxhk): target.add marked destructive + path validated at registration |
| `6ca17048` | fix(lab-n4fb): canonicalize fail-closed for stash deploy path denylist |
| `35036109` | fix(lab-686q): typed 404 downcast in node_connected, remove redundant log event, add retry assertion |
| `e5c3361e` | fix(lab-686q): allow dead_code on build_release compat wrapper |
| `7e9db919` | test(lab-686q.2): replace symbol-check with real behavior tests for node_connected |
| `e8bd9793` | fix(lab-686q.1): run_impl builds per-role artifacts — no more panic on Node-role hosts |
| `d9c4a050` | test(lab-686q.3): add tests for wait_for_node_connected retry and timeout logic |
| `df4bc31f` | test(lab-686q.4): add tests for --role node and config role=node without controller host |
| `e44249b2` | fix(lab-686q): fix clippy lint warnings — remove unused Duration/jitter_window, allow dead_code on reserved fields |
| `e7ae7d59` | docs(lab-686q): Task 14 — normalize controller/node naming, document artifact split |
| `aad75295` | feat(lab-686q): Task 13 — per-role artifact map in deploy runner, DeployArtifactSummary in plan/summary |
| `c93172a3` | chore(lab-686q): fix extract feature ordering in lab-apis features list |
| `a4af24a4` | feat(lab-686q): Task 12 — gate lab-apis/extract deps behind extract feature |
| `8a6766d7` | feat(lab-686q): Task 11 — make clap_complete optional, gate completions behind controller feature |
| `85ae9017` | feat(lab-686q): Task 10 — feature groups (controller, services-all, node-runtime), gate gateway/marketplace/upstream |
| `29867ca6` | feat(lab-686q): Tasks 8+9 — readiness contract docs, backup path in recovery result |
| `9411d92a` | fix(lab-686q): thread config port through verify_local_health (no hardcoded 8765) |
| `8137a3b2` | feat(lab-686q): Task 7 — role-based nodes update, wait_for_node_connected, multi-artifact build |
| `df7e13c9` | feat(lab-686q): Task 6 — MasterClient::node_connected for rollout verification |
| `1f01558e` | feat(lab-686q): Tasks 4+5 — deploy profiles, ArtifactProfile, build_artifact with timeout |
| `33650f64` | feat(lab-686q): Task 3 — move backoff helpers to net/backoff, add node-runtime feature |
| `44847e42` | feat(lab-686q): Task 2 — node-mode early return in serve, start_background_tasks, loopback health server |
| `e7f9ad68` | fix(lab-686q): add resolution source to role.resolved tracing event |
| `3889b496` | feat(lab-686q): Task 1 — NodeRuntimeRole config, ServeRole CLI, resolve_runtime_role_from_config |
| `073e1456` | fix(acp): add turn-drain timeout to handle stale messages after idle-completed turns |

### Highlights

- **Node/controller runtime split** — adds explicit node runtime role handling, node-mode serving behavior, controller/node naming docs, and deployment artifacts split by role.
- **Deploy and readiness hardening** — adds deploy profiles, artifact summaries, local-health port threading, wait-for-node-connected retry behavior, and recovery backup path reporting.
- **Feature grouping** — gates controller-only and service-heavy code behind feature groups, makes completions optional, and gates extract dependencies behind the extract feature.
- **ACP and dispatch fixes** — protects ACP multi-turn flows from stale messages and wraps sync stash dispatch paths in `spawn_blocking`.

### Version bumps

- Rust workspace: `0.12.2 → 0.13.0`
- Gateway admin package: `0.6.0 → 0.13.0`

---

## [0.12.2] — 2026-05-03

| Commit | Change |
|--------|--------|
| `50824844` | chore(lab-in5q.4): fix internal cross-references in moved doc files |
| `498b1ffa` | chore(lab-in5q.3): update Rust source doc path comments |
| `7ce5812e` | chore(lab-in5q.2): update CLAUDE.md references for moved doc files |
| `79824d98` | chore(lab-in5q.1): reorganize docs/ root — move 34 files into surfaces/ runtime/ services/ dev/ |
| `cf9373e7` | chore: dev tooling, ACP multi-turn fix, and docs reorg prep |
| `386e6d7b` | chore: save CI debugging session state |
| `9689190c` | fix: avoid Windows CI cache save failure |
| `6f8b8189` | fix: Windows release warnings |
| `68a35a37` | chore: trigger CI after history rewrite |
| `11199215` | fix: CI failures |
| `60568674` | chore: set up CI release smoke and generated docs |
| `da3a8d10` | docs: say copy config.example.toml to ~/.lab/config.toml |
| `0db193bf` | fix: config.toml is gitignored; update docs |
| `3a226869` | feat: fleet scan wizard step, config consolidation, and TS fixes |
| `8b1b9967` | chore: document cargo-deny advisory exceptions |
| `a0c5f734` | chore: integrate service wave and CI updates |
| `d31767c9` | fix(lab-8l5s): preserve ServiceForm RHF state across tab switch |
| `ef2cae3a` | docs(lab-qz0z): document R5 RHF state-loss tradeoff inline |
| `233595ca` | fix(lab-qz0z): post-review cleanup (HTTPS_SCHEME_RE, %00 blocking) |
| `f911d607` | fix(lab-qz0z): mirror sessionStorage write outside React state updater |
| `8510d39c` | fix(lab-qz0z): lifecycle cleanups (P3-3, P3-4, P3-7) |
| `de0952f7` | fix(lab-qz0z): RHF perf — Controller for bool, memoized callbacks |
| `00210371` | fix(lab-qz0z): TypeScript and code-quality nits batch |
| `19f16e8c` | fix(lab-qz0z): secret-handling hardening (P2-6, P3-8) |
| `65c4bcc7` | fix(lab-qz0z): harden schemaBuilder validation |
| `619c8445` | feat(lab,lab-apis,lab-auth): backend in-flight work |
| `13e29ede` | fix(lab-qbbt): distinguish transport errors from blocking findings |
| `7be1b484` | fix(lab-emkz): lazy-mount only the active ServiceForm tab |
| `6c16f591` | fix(lab-1ai7): re-check pathname after setup.state await |
| `f987aae6` | fix(lab-kltp): surface draft-stale check failures instead of silencing |
| `fbd2af79` | fix(lab-ijf3): synchronous lock + AbortController for core-config save |
| `7bf605a3` | fix(lab-fcz0): thread AbortSignal through doctor.service.probe |
| `9a641bc6` | fix(lab-4cn9): persist wizard selectedServices to sessionStorage |
| `7dd6570f` | fix(lab-68ja): centralize '***' secret sentinel as STORED_SECRET_MARKER |
| `44a3728a` | fix(lab-zmj1): extract CORE_FIELDS to shared module |
| `77efe9b5` | fix(lab-7bat): remove dead draftValues/setDraftValue from WizardContext |
| `927a1a6a` | feat(lab-apis,lab): onboard 3 services + extend adguard/glances/uptime-kuma |
| `9604c93f` | feat(lab-apis,lab): onboard 6 services (dozzle, freshrss, immich, loki, prowlarr, sabnzbd) |
| `331a38e1` | feat(lab-bg3e.4,bg3e.5): /setup wizard + /settings rail web UI |
| `9d24b17e` | test(lab-bg3e.3.11): mechanical guard for orchestrator one-way dependencies |
| `b28d5a28` | refactor(lab-bg3e.3.7): env_merge polish |
| `2ef9b43c` | fix(lab-bg3e.3.9): defense-in-depth hardening |
| `7a612a65` | refactor(lab-bg3e.3.8): tighten setup dispatch hygiene |
| `07041287` | refactor(lab-bg3e.3.10): drop dead is_headless() |
| `8195e86b` | feat(lab-bg3e.3.4): add write_service_creds shim over env_merge::merge |
| `b705d37b` | perf(lab-bg3e.3.1): memoize ToolRegistry + env-var/secret-key indexes |
| `4e717482` | fix(lab-bg3e.3.2): wrap doctor.audit.full in 30s timeout |
| `de859e41` | fix(lab-bg3e.3.3): apply host_validation Layer to all v1 unauthenticated routes |
| `758ec61f` | perf(lab-bg3e.3.6): single-pass audit_summary count |
| `bb1d071a` | fix(lab-bg3e.3.5): fsync parent dir after env_merge::persist |

### Highlights

- **ACP multi-turn drain timeout** — `acp_turn_drain_timeout()` + `DEFAULT_TURN_DRAIN_TIMEOUT` (5 min, overridable via `LAB_ACP_TURN_DRAIN_TIMEOUT_MS`) drains stale messages left by idle-completed turns before starting the next prompt. Prevents a late `PromptResponse`/`StopReason` from poisoning the new inner read loop during long agentic tool calls.
- **Docs reorganization (lab-in5q)** — 34 docs moved from `docs/` root into `docs/surfaces/`, `docs/runtime/`, `docs/services/`, and `docs/dev/`; CLAUDE.md, README references, and Rust source path comments all updated.
- **Service onboarding wave** — 9 new services onboarded: dozzle, freshrss, immich, loki, prowlarr, sabnzbd (wave 1) + 3 more + adguard/glances/uptime-kuma extensions.
- **Setup wizard + settings rail UI (lab-bg3e)** — full `/setup` wizard flow (fleet scan, service creds, config write) and `/settings` side-rail; `write_service_creds` shim, `env_merge` polish, `ToolRegistry` memoization, `doctor.audit.full` timeout, host-validation middleware, fsync-after-persist hardening.
- **Frontend hardening (lab-qz0z, lab-8l5s, and others)** — RHF state preservation across tab switches, secret-sentinel centralization, schemaBuilder validation hardening, sessionStorage persistence for wizard selections, AbortController for config saves, lazy-mount for inactive ServiceForm tabs.
- **CI improvements** — release smoke tests, generated-docs pipeline, Windows cache-save fix, cargo-deny advisory exceptions.

### Version bumps

- Rust workspace: `0.12.1 → 0.12.2`

---

## [0.12.1] — 2026-04-30

| Commit | Change |
|--------|--------|
| `5a00e40c` | chore(release): v0.12.1 — binary build fix |
| `bcc59e4f` | fix: declare observability module in main.rs and add stash to router parity test |

### Highlights

- **Binary build fix** — `main.rs` was missing `mod observability;`, so `crate::observability::activity::ActorKey{,Deriver}` references in `api/state.rs` and `api/router.rs` failed to resolve when compiling the binary (lib.rs already declared the module, so library-only callers were unaffected). Five E0433 errors gone. Also adds `stash` to the `registry_and_router_service_sets_are_identical` parity test, which had been silently asserting an outdated set since `lab-qz6a.8` landed stash in the HTTP router.

### Version bumps

- Rust workspace: `0.12.0 → 0.12.1`

---

## [0.12.0] — 2026-04-30

| Commit | Change |
|--------|--------|
| `3244fb7c` | chore(release): v0.12.0 — ACP review remediation epic close-out |
| `e2ade2b9` | docs(BD-lab-j04j.16): refresh ACP docs against landed first-class state |
| `f8e88fda` | feat(BD-lab-j04j.11): structured AcpProviderEntry args/cwd/env |
| `90b16a48` | feat(BD-lab-j04j.10): bound ACP event channel to 1024 with await-on-send |
| `0838775d` | docs(BD-lab-j04j.19): document provider prompt idle timeout |
| `e2d8b6c0` | feat(BD-lab-j04j.18): replace page-context allowlist with predicate sanitizer |
| `20c0a2b7` | feat(BD-lab-j04j.15): cap ACP SSE backfill at SQL layer |
| `cf2c7e5b` | feat: gate stdio gateway specs behind allow_stdio admin ack |
| `0221b23f` | docs: expand product and marketplace surface |
| `4a8a2d53` | docs: expand product feature overview |
| `3215a9ba` | docs: describe product feature surface |
| `18a5684b` | chore: update marketplace docs and monitors |
| `fe09366c` | fix(dev): address code review findings |
| `4ae40caf` | feat(lab-zxx5.8): add MCP server install modal with gateway selection |

### Highlights

- **ACP review remediation (lab-j04j) — epic closed** — 18 of 19 child beads landed; runtime/security hardening across SSE backfill, event channel bounding, provider config, page-context sanitizer, idle-timeout docs. Bridge\* compat removal (.12) deferred pending coordinated frontend wire-format change.
- **ACP SSE backfill SQL cap (.15)** — `load_events_since_capped` on `AcpPersistence` trait + SQLite subquery (`ORDER BY seq DESC LIMIT N`, re-sorted ASC) preserves "last N events" backfill contract without materialising the full event range. Previous in-Rust truncation was a memory waste at high event rates.
- **ACP event channel bounded (.10)** — per-session `UnboundedSender<AcpEvent>` from runtime → registry hub becomes `mpsc::Sender<AcpEvent>` at capacity 1024 with await-on-send. Back-pressures the provider's stdio reader on persistence stalls instead of growing memory unboundedly. Five sync `emit_*`/`push_session_update` helpers become async; `std::Mutex` guard scopes restructured to avoid spanning `.await`.
- **Structured AcpProviderEntry (.11)** — `command + args + cwd + env` schema with serde defaults; legacy entries fall back to whitespace-split `command` for one-time read fidelity. Re-installing a provider migrates the on-disk entry. Marketplace install paths (binary/npx/uvx) build args as `Vec<String>` rather than concatenating into a single string.
- **Page-context sanitizer (.18)** — predicate-based `is_safe_page_context_char` replaces the 62-element char allowlist; deny-list bypass detection adds a separator-stripped joined-form check; 23 tests covering control chars, unicode rejection, separator-bypass attempts, and length boundaries.
- **Stdio gateway admin ack** — `gateway.test`/`add`/`update` require explicit `allow_stdio: true` when the upstream spec uses stdio. Stdio specs spawn local subprocesses, so admin operations against them are gated through `ensure_stdio_admin_ack` to prevent silent process launches via remote dispatch. CLI mirrors with `--allow-stdio` flags; catalog publishes `allow_stdio` as a documented param.
- **Provider prompt idle timeout (.19)** — operator-facing section in `docs/acp/README.md` documenting the 5 s default, `LAB_ACP_PROMPT_IDLE_TIMEOUT_MS` override, and the observable firing behavior (`session_state` Completed + `provider_info` `idle_completion`).
- **ACP docs match landed first-class state (.16)** — README inventories the landed pieces (lab-apis::acp module, dispatch/acp/, registry registration, HTTP routes), enumerates landed protections, and lists remaining gaps (Bridge\* compat, typed CLI shim, provider workspace jail) without claiming deferred work.
- **Pre-existing unreleased work** — earlier commits (`0221b23f` … `4ae40caf`) accumulated in the previous Unreleased section before the epic close-out and ride along with this release: tool-search config + settings UI for gateway-admin, MCP server install modal with gateway selection, marketplace and product docs expansion, dev review-finding fixes.

### Version bumps

- Rust workspace: `0.11.1 → 0.12.0`
- gateway-admin: `0.5.1 → 0.6.0` (bumped during the Unreleased window prior to this release)

---

## [0.11.1] — 2026-04-25

| Commit | Change |
|--------|--------|
| `82478a0b` | chore(release): v0.11.1 — marketplace P1 security follow-up + workspace fs hardening |
| `2f6d76c6` | docs: setup+settings feature design spec + component-development doc update |
| `07ccb54c` | fix(dev): ensure dev_mockup routes survive router.rs refactors |
| `d10b05ec` | fix(dev/nodeinfo): read env from process (dotenvy already loaded .env at startup) |
| `991fcd1b` | feat(dev): extend nodeinfo to return .env values with secrets masked |
| `aea3bb59` | fix(dev): restore dev_mockup handlers and page routes |
| `b1385289` | fix(dev): restore /dev mockup routes + add /dev/api/nodeinfo |
| `265a701e` | feat(dev): add mockup file server at /dev and /dev/:name |
| `3e8db769` | fix(pr29): address review threads — security, fleet, ACP, marketplace, docs |
| `f168964b` | fix(lab-zxx5.32): R2 P3 roll-up — redact_home in errors, log tiering, sync_all, dead Sized |
| `39266dce` | refactor(lab-f1t2): address simplify + review findings on the f1t2 wave |
| `b7f488af` | fix(lab-zxx5.30,lab-zxx5.31): partial-extraction detection + fail-closed walk |
| `7b051062` | fix(lab-zxx5.29): validate node install result shape |
| `12eb0ea0` | fix(lab-zxx5.28): typed error markers restore install taxonomy |
| `ae302ef6` | docs(lab-f1t2.32): document MCP transport auth requirement for fs |
| `86e943eb` | fix(lab-f1t2.26): redact path from deny-list oracle log events |
| `c9be4573` | fix(lab-f1t2.30): reset AttachmentChip thumbUrl at effect start |
| `33db1293` | fix(lab-f1t2.29): reset loading/truncated when picker closes mid-fetch |
| `0e7a569f` | fix(lab-f1t2.24): handle help/schema before workspace_root resolution |
| `6101fdbe` | fix(lab-zxx5.27): P3 roll-up — SSRF edges, per-node cap, redact_home, naming cleanup |
| `3c135072` | docs(lab-f1t2.31): document fs registry uses MCP-filtered slice intentionally |
| `b6386ad9` | fix(lab-f1t2.28): move setSending(true) inside sendingRef try |
| `76962fc3` | fix(lab-f1t2.27): align workspace-picker error kinds with backend |
| `c892efce` | test(lab-f1t2.25): bidirectional parity test for MCP fs catalog |
| `85f019e4` | fix(lab-f1t2.23): case-insensitive credential deny-list |
| `9aaa8c7a` | fix(lab-f1t2.22): reject intra-workspace symlinks in openat2 fallback |
| `40ac16a1` | fix(lab-zxx5): resolve multi-agent review P1+P2 findings |
| `e7ea8528` | refactor(lab-f1t2.20): inline log_dispatch/log_dispatch_preview wrappers |
| `01de323a` | chore: untrack crates/lab/target/ build artifacts |

### Highlights

- **Marketplace P1 security follow-up (lab-zxx5)** — multi-agent review P1+P2 fixes, install_component/agent.install hardening, SSRF blocklist edges, per-node caps, `redact_home` helper applied to errors and log tiering, partial-extraction detection with fail-closed walk, typed install error markers
- **Workspace fs hardening (lab-f1t2)** — security headers via subrouter middleware, intra-workspace symlink rejection in openat2 fallback, case-insensitive credential deny-list with path redaction, MCP transport auth requirement documented, MCP↔canonical fs ActionSpec parity locked, AttachmentChip + chat-input + workspace-picker race elimination, UX polish
- **Dev mockup routes** — mockup file server at `/dev` and `/dev/:name`, `/dev/api/nodeinfo` returning `.env` values with secrets masked, route survival across router.rs refactors
- **Docs** — setup+settings feature design spec, component-development doc update

### Version bumps

- Rust workspace: `0.11.0 → 0.11.1`
- gateway-admin: `0.5.0 → 0.5.1`

---

## [0.11.0] — 2026-04-24

| Commit | Change |
|--------|--------|
| `9d83267b` | chore: bump workspace to 0.11.0 + misc uncommitted work |
| `979bae1a` | feat(lab-zxx5.18): install_component/agent.install security hardening |
| `bbebe993` | refactor(lab-f1t2.18): removeAttachment keys on (kind, path) compound |
| `b41a7315` | ux(lab-f1t2.19): workspace picker polish — truncated reset + kind messages + aria |
| `328664b4` | perf(lab-f1t2.15): dedupe concurrent workspace preview fetches |
| `1c8b9731` | fix(lab-f1t2.16): eliminate chat input + workspace picker + preview races |
| `d077428b` | test(lab-f1t2.11): lock MCP/canonical fs ActionSpec parity |
| `f66823aa` | perf(lab-f1t2.14): eliminate redundant lstat + ASCII fast-path for deny-list |
| `c844d053` | feat(lab-zxx5.16): cherry-pick SSE progress endpoint |
| `b14cbe75` | refactor(lab-f1t2.17): consolidate fs dispatch into single match body |
| `a718f15a` | fix(lab-f1t2.12): apply fs security headers via subrouter middleware |
| `cfeb698a` | feat(lab-f1t2.13): register fs unconditionally when feature-enabled |
| `12666cef` | fix(lab-zxx5.2): route mcp.* actions to mcp_dispatch in marketplace dispatch |
| `8d0b2572` | chore(lab-f1t2): snapshot pre-review-fixes state |
| `7610accd` | feat(lab-zxx5.6): wire real NodeRpcPort + master pending infra + rename device→node |
| `4c7567a1` | feat(lab-zxx5.19): bounded inbound-RPC dispatch + UUIDv4 request ids |
| `7f0f55e4` | fix(lab-zxx5.15): normalize marketplace client path helpers to Result |
| `910037d3` | feat(lab-zxx5.14): Default derives, redact_home helper, plugins.list invariant test |
| `d18eb12b` | feat(lab-ccc9): Phase 3 WS fleet method handlers + MCP demux |
| `1351cad2` | feat(lab-e2tu): SQLite-backed node log persistence with 30-day TTL retention |
| `9300b884` | fix(lab-zxx5.13): map ambiguous_tool kind to 409 Conflict + document |
| `daeb1ef6` | fix: restore compile — add AmbiguousTool variant, fix codex backend Option/Result, update Marketplace/Plugin literals |
| `d77fbeab` | feat(lab-f1t2.1): workspace root resolver + AppState field |
| `462e63f6` | feat(lab-yn60): complete device→node module rename |
| `0564a9e2` | wip(acp): chat-shell + session events + ACP runtime refactor |
| `916ac283` | feat(lab-zxx5.8): add MCP server install modal with gateway selection |
| `20cc45a9` | feat(lab-zxx5.3): stream SHA-256 during binary archive download |
| `453162aa` | fix: commit node module files and resolve device→node rename breakage |
| `ec476ba3` | feat(lab-zxx5.3): implement remote fleet WS install and binary agent download |
| `81901791` | perf(lab-kvhi.16): run config.read + current_pool concurrently in gateway list/get |
| `f16f43a9` | fix(lab-kvhi.14): accumulate reasoning duration across SSE reconnects |
| `a4851368` | feat(lab-zxx5.6): add plugin.cherry_pick dispatch action |
| `21e5f4b5` | feat(lab-zxx5.11): unified marketplace API client + PluginComponent types |
| `e93da3ae` | feat(lab-zxx5.4): delete mcpregistry dispatch surface, migrate to marketplace |
| `094eeba4` | feat(lab-zxx5.3): add ACP agent dispatch actions (agent.list/get/install/uninstall) |
| `9bbfd50c` | feat(lab-zxx5.10): add cherry-pick component selector dialog |
| `ae827055` | fix(lab-zxx5): resolve Wave 1 compile errors and test failures |
| `0c7f4cbc` | feat(lab-bg3e.2): promote doctor to full Bootstrap dispatch service |
| `f504e26a` | fix(gateway-admin): misc correctness + accessibility batch |
| `d2bbdd05` | fix(gateway-admin): prop-spread ordering to prevent consumer clobbering |
| `043920c7` | fix(gateway-admin): file-tree accessibility + dead code + handler ordering |
| `282e18b5` | fix(gateway-admin): prompt-input five correctness fixes |
| `41b1f167` | feat(lab-zxx5.8): add MCP server install modal with gateway selection |
| `e7760dd9` | fix(gateway-admin): shared useCopyTimeout hook to prevent leaked setState-after-unmount |
| `a3de2667` | feat(lab-zxx5.9): add ACP agent install modal with device and scope selection |
| `7a76de00` | fix(gateway-admin): runtime crash + stuck timer + unreachable Cancel |
| `eca9f7d9` | fix(gateway-admin): resolve broken ~/ import aliases in AI components |
| `d8490870` | feat(lab-jwbg.8): ACP service registration — PluginMeta, registry, serve wiring |
| `c2f8bd65` | feat(lab-zxx5.1): add lab-apis/src/acp_registry SDK client |
| `1945e5b3` | fix(lab-zxx5): resolve Wave 0 compile errors and test failures |
| `8a166f14` | feat(lab-jwbg.7): migrate API/ACP surface to dispatch/acp layer |
| `dbf49212` | feat(lab-jwbg.6): dispatch/acp layer — catalog, client, params, dispatch |
| `3ff6b209` | feat(lab-jwbg.5): rewrite AcpSessionRegistry — Arc<Session>, per-subscriber mpsc, ownership |
| `78a8f7f7` | feat(lab-bg3e.1): UiSchema/FieldKind types + PluginMeta.supports_multi_instance for all 23 services |
| `dd707162` | feat(lab-jwbg.3): SQLite persistence layer — AcpPersistence trait + SqliteAcpPersistence |
| `c3e0f350` | feat(lab-zxx5.5): add marketplace.install_component + agent.install RPC methods |
| `791d1196` | feat(bd-security/marketplace-p1): ACP types, fleet WS registry, marketplace UI, Category::Marketplace |
| `f8de5bde` | feat(lab-jwbg.2): migrate ACP types — Bridge* → Acp* in lab-apis |
| `bba30eb2` | feat(lab-zxx5.7): unified marketplace type filter + MCP/ACP item cards |
| `3124a871` | feat(lab-zxx5.5): add fleet WS master→device sender registry |
| `43ad105b` | fix(pr29): catalog filter chips can return to 'all' view |
| `b8ad6306` | feat(lab-zxx5.12): add Category::Marketplace, recategorize marketplace + mcpregistry |
| `35752048` | fix(pr29): address remaining review threads on AI components + docs |
| `9e0383ba` | fix(marketplace): address PR #29 review threads — installPath validation |
| `299eb724` | fix(lab-jwbg.9): eliminate try_write().expect() panic in AcpSessionRegistry |
| `526bf3e1` | feat(lab-jwbg.1): create lab-apis::acp module scaffold |

### Highlights

- **Workspace bumped two minors in one commit** — `9d83267b` jumped `0.9.0 → 0.11.0` directly with no `0.10.x` published; this section accumulates everything done between the `0.9.0` bump and that commit
- **WS fleet runtime + remote install (lab-zxx5.3/.6, lab-ccc9, lab-e2tu)** — real `NodeRpcPort` master pending infra, device→node module rename, remote fleet WS install + binary agent download (streamed SHA-256), plugin.cherry_pick dispatch + cherry-pick component selector dialog, Phase 3 WS fleet method handlers + MCP demux, SQLite-backed node log persistence with 30-day TTL retention, SSE progress endpoint
- **ACP service consolidation (lab-jwbg)** — `acp_registry` SDK client + `lab-apis::acp` scaffold, `dispatch/acp` layer (catalog, client, params, dispatch), API/ACP surface migrated to dispatch layer, `AcpSessionRegistry` rewrite with `Arc<Session>` + per-subscriber mpsc + ownership semantics, SQLite persistence (`AcpPersistence` trait), ACP agent dispatch actions (`agent.list/get/install/uninstall`), MCP server + ACP agent install modals with gateway/device/scope selection, `try_write().expect()` panic eliminated
- **Marketplace consolidation (lab-zxx5.x)** — unified marketplace API client + `PluginComponent` types, `mcpregistry` dispatch surface deleted and migrated to marketplace, `Category::Marketplace` introduced, install_component/agent.install RPC methods, fleet WS master→device sender registry, multi-agent review P1+P2 fixes
- **Workspace fs (lab-f1t2 entry)** — workspace root resolver + AppState field, fs registered unconditionally when feature-enabled, dispatch consolidated into single match body, MCP/canonical fs ActionSpec parity test, deny-list ASCII fast-path
- **Doctor + bootstrap (lab-bg3e)** — doctor promoted to full Bootstrap dispatch service, `UiSchema`/`FieldKind` types + `PluginMeta.supports_multi_instance` for all 23 services
- **Gateway admin AI component pass** — prompt-input five-fix correctness pass, file-tree accessibility, prop-spread ordering, runtime-crash + stuck-timer + unreachable-Cancel fixes, shared `useCopyTimeout` hook, AI components import-alias repair
- **Gateway perf** — `config.read` + `current_pool` run concurrently in gateway list/get, reasoning duration accumulated across SSE reconnects

### Version bumps

- Rust workspace: `0.9.0 → 0.11.0` _(skipped `0.10.x`)_

---

## [0.9.0] — 2026-04-23

| Commit | Change |
|--------|--------|
| `2013dbdd` | feat: AI component library, ACP docs, gateway/marketplace UI refinements — v0.9.0 |
| `7c4fb9f` | fix(lab-kvji.10.1): validate path components in parse_plugin_id |
| `ca66a3b` | fix(lab-kvji.10.3): validate installPath from installed_plugins.json |
| `cd8bfa9` | fix(lab-kvji.10.2): add symlink guards to all filesystem walkers |
| `a9dcd54` | Finalize gateway admin, registry, and auth follow-ups |
| `0a6c846` | feat: add registry metadata curation and admin filters |
| `479bae4` | fix: address latest PR comment |
| `5a75aba` | fix: address follow-up PR comments |
| `227b4ed` | fix: address PR review feedback |
| `fd8aafc` | docs: update fleet websocket runtime docs |
| `8ecda7b` | feat: add websocket fleet runtime |
| `facca22` | docs: add fleet ws runtime design |
| `0cad306` | Finalize remaining gateway admin and registry work |
| `47171c0` | fix: address remaining marketplace and upstream review comments |
| `4392a42` | fix: address gateway plan and docs review comments |
| `867dda3` | fix: address gateway admin design-system review comments |
| `ccafbdb` | fix: address gateway admin registry review comments |
| `91188af` | fix: address gateway admin chat and logs review comments |
| `410acdb` | Finalize remaining chat, marketplace, and deploy updates |
| `38fd124` | fix: address PR comments for gateway policy and browser session auth |
| `997110e` | fix: address PR comments for marketplace client and dialog flows |
| `6ae4bd9` | fix: address PR comments for registry and marketplace dispatch |
| `a51056f` | fix: address PR comments for gateway and registry docs |
| `e5dec3d` | Add gateway ACP, marketplace, and CLI UI updates |
| `9a0f23b` | Address PR review feedback |

### Highlights

- **Marketplace security hardening P1 (lab-kvji.10)** — path traversal via plugin ID blocked at parse time; symlink following eliminated from all four filesystem walkers; `installPath` from `installed_plugins.json` validated against `plugins_root` before use
- **AI component library** — 26 new TSX components under `components/ai/` covering agents, artifacts, attachments, code blocks, reasoning, tool calls, and more
- **Fleet websocket runtime** — initial `feat: add websocket fleet runtime`; ACP provider, session registry, SSE transport, design docs
- **Registry metadata curation** — Lab-owned `_meta["tv.tootie.lab/registry"]` contract, validation, audit fields, server-side metadata filters, typed CLI metadata commands, gateway-admin structured metadata editing
- **Marketplace and upstream hardening** — marketplace client/dispatch cleanup, upstream pool adjustments, browser session auth fixes, large batch of PR-review-driven repairs across gateway, registry, marketplace, chat, and deploy

### Version bumps

- Rust workspace: `0.7.3 → 0.9.0` _(skipped `0.8.x`)_

---

## [0.7.3] — 2026-04-22

| Commit | Change |
|--------|--------|
| `681986c` | feat(gateway-chat-registry-log-ui): marketplace UI, gateway/chat/registry/log component polish, mcpregistry fixes — v0.7.3 |
| `802d67e` | feat(marketplace): route + sidebar nav entry — Marketplace page complete |
| `3674c5b` | feat(marketplace): all UI components — cards, panels, dialogs, modal |
| `120bf6a` | feat(marketplace): types, API client (mock data), and SWR hooks |
| `861e4e8` | feat(gateway-admin): wire listServers to GET /v0.1/servers REST endpoint |
| `de8d173` | fix(registry_v01): normalize error kinds; add owner filter; use ToolError uniformly |
| `ff6185a` | fix(mcpregistry): extract shared sync guards to dispatch layer |
| `4dfd248` | fix(mcpregistry/params): add Tailscale CGNAT range to SSRF blocklist |
| `9892d33` | fix(mcpregistry/store): ON CONFLICT DO UPDATE, jiff, WAL, UTF-8 truncation |
| `c67b839` | fix(lab): remove chrono dep, feature-gate rusqlite/r2d2 under mcpregistry |
| `281dfbd` | fix(log_fmt): replace chrono with jiff for timestamp formatting |
| `af7d12a` | fix(mcpregistry): surface upstream errors properly; add Upstream variant |
| `9ff7ded` | feat(mcpregistry): add sync observability — start/page/finish log events |
| `8e17b84` | fix(registry_v01): use axum 0.8 {param} route syntax instead of :param |
| `388c22e` | fix: squash serve/dispatch warnings (unnecessary qualifications, dead code) |

### Highlights

- **Marketplace UI** — full Marketplace page: types, mock API client, SWR hooks, card/panel/dialog/modal components, route + sidebar nav entry
- **Gateway admin REST wiring** — `listServers` now calls `GET /v0.1/servers`; gateway/registry/log/chat UI components updated (filters, table, detail panel, session sidebar, log console)
- **mcpregistry fixes** — sync guard extraction, SSRF blocklist (Tailscale CGNAT), `ON CONFLICT` upsert, WAL mode, jiff timestamp, upstream error surfacing, sync observability log events
- **Chrono → jiff migration** — removed `chrono` dep from workspace; log formatter uses `jiff`
- **Registry v0.1 API fixes** — axum 0.8 `{param}` route syntax, owner filter, `ToolError` normalization

### Version bumps

- Rust workspace: `0.7.2 → 0.7.3`

---

## [0.7.2] — 2026-04-22

| Commit | Change |
|--------|--------|
| `2caf21b` | feat(lab-h5pm.4): dispatch sync action with RAII AtomicBool rate-limit guard |
| `8233ac5` | feat(registry): use GitHub owner avatar as server image |
| `0d1acba` | feat(gateway-admin): aurora token sweep + eslint enforcement |
| `04a0dbd` | feat(lab-h5pm.2): implement RegistryStore query methods, upsert, and full sync |
| `96ddf66` | feat(lab-h5pm.1): create RegistryStore module skeleton in dispatch layer |

### Highlights

- **RegistryStore (lab-h5pm)** — SQLite-backed MCP server registry with skeleton, query/upsert/full-sync, and dispatch sync action protected by a RAII AtomicBool rate-limit guard
- **GitHub owner avatar** — registry list rows and detail header now pull `https://github.com/<owner>.png` from `server.repository.url`, falling back to `icons[0]` then a `Package` lucide icon
- **Aurora token sweep (product code)** — replaced shadcn-generic tokens (`text-muted-foreground`, `bg-card`, `bg-muted`, `bg-background`, `border-border`, `text-foreground`, `rounded-xl`) with Aurora equivalents across 19 files in `components/` and `app/`
- **ESLint enforcement** — new `no-restricted-syntax` rule bans the same tokens in `className` literals and template elements, scoped to `app/**` and `components/**` with `components/ui/**` exempted as the sanctioned escape hatch
- **Design-system contract** — added Authentication Surfaces section, banned-shadcn-token mapping table, eyebrow drift guidance, typography-ramp override rule, and Display Slot Assignments table
- **Brand icon polish** — gateway form brand chip now renders white-backed with colored border and SVG fill recoloring for stronger contrast
- **Test-compile repairs** — added `proxy_prompts` to `UpstreamConfig` literals across 4 files + `search` to `StoreListParams` literal; all-features tests compile clean

### Version bumps

- Rust workspace: `0.7.1 → 0.7.2`
- gateway-admin: `0.2.1 → 0.2.2`

---

## [0.7.1] — 2026-04-21

| Commit | Change |
|--------|--------|
| `52ef7d4` | refactor(ui): complete Aurora token sweep across all shadcn primitives — v0.7.1 |

### Highlights

- **Aurora token sweep** — complete theming of all `components/ui/` shadcn primitives: toggle, navigation-menu, skeleton, dialog, item, calendar, scroll-area, resizable, badge, checkbox, switch, radio-group, slider, dropdown-menu, select, alert, separator, accordion, progress, tabs, sonner, command, context-menu, menubar
- **Focus ring normalization** — all Radix primitives now use `aurora-accent-primary` rings instead of shadcn `ring-ring/50` defaults
- **Hover state normalization** — all `bg-accent`/`focus:bg-accent`/`hover:bg-accent` replaced with `aurora-hover-bg` across all menu and interactive components
- **Light mode fix** — `--aurora-hover-bg: #dcedf2` added to `.light` class (was dark-only)
- **`text-aurora-text-secondary` purge** — removed all 10 usages of the no-op token (not in `@theme inline`); replaced with `text-aurora-text-muted`
- **`aurora-scrollbar` utility** — added to `globals.css` for Firefox + WebKit scrollbar theming
- **`alert` success variant** — new `success` variant added to `alert.tsx`
- **JsonHighlight** — syntax-colored JSON renderer in `server-detail-panel.tsx`

### Version bumps

- Rust workspace: `0.7.0 → 0.7.1`
- gateway-admin: `0.2.0 → 0.2.1`

---

## [0.7.0] — 2026-04-21

| Commit | Change |
|--------|--------|
| `8cc9a59` | feat(gateway-admin): chat UI, registry enhancements, log toolbar refactor — v0.7.0 |
| `3eaa81c` | docs(observability): document ANSI sanitization, resource_uri redaction, and shell wrapper boundary |
| `762be6e` | feat(observability): add missing identifying fields to MCP/upstream warn events |
| `b09db3f` | feat(observability): normalize startup lifecycle events in lab serve |
| `0203829` | feat(formatter): extract PremiumEventFormatter into log_fmt/ with Axon-style semantic coloring |
| `234f7c4` | fix(security): sanitize log field values + redact upstream credentials |

### Highlights

- Chat UI (`components/chat/`, `app/(admin)/chat/`) and branding lib added to gateway-admin
- Registry: server detail panel expansion, filter sidebar, richer list content
- Log toolbar refactored; `log-filters.tsx` and `log-stream-status.tsx` consolidated
- Observability improvements: startup lifecycle events, MCP/upstream warn fields, ANSI sanitization
- `PremiumEventFormatter` extracted into `log_fmt/` with Axon-style semantic coloring
- Security: log field value sanitization + upstream credential redaction

---

## [0.6.1] — 2026-04-21

| Commit | Change |
|--------|--------|
| `9d1d355` | refactor(cli): wire CLI shims to shared dispatch + add --yes/--dry-run |
| `29e6166` | fix: restore plugins/ to repo |
| `a1058de` | chore: remove stale root plugin files and gh-webhook tool |

### Highlights

- All CLI service shims now delegate to the shared `dispatch/` layer
- `--yes` / `--dry-run` flags wired for destructive actions across all services
- Plugin asset hygiene pass

---

## [0.6.0] — 2026-04-21

| Commit | Change |
|--------|--------|
| `b13fb8a` | feat(auth): browser session + upstream pool + MCP peers |
| `4ddac44` | chore(plugin): restructure plugin assets under plugins/ |

### Highlights
- Browser session cookie management for services requiring login flows
- `dispatch/upstream/pool.rs` — upstream MCP proxy pool with circuit breaker
- MCP peer registry for multi-instance upstream routing

---

## [0.5.1] — 2026-04-21

| Commit | Change |
|--------|--------|
| `beb3de0` | chore(cli): action enum validation + plugin.json simplification |
| `86ed3c5` | feat(lab-aiit.1): stdio install dispatch + security hardening for mcpregistry |

### Highlights
- CLI action enum validated at parse time (unknown actions rejected early)
- mcpregistry stdio install path + SSRF/path-traversal hardening

---

## [0.5.0] — 2026-04-21

| Commit | Change |
|--------|--------|
| `d1a3ea6` | chore: v0.5.0 — gateway-admin redesign, deploy monitor, docs |
| `740ff96` | refactor(lab-5x4t): finish aurora palette sweep |
| `513bd48` | feat(lab-5x4t.5): add --aurora-preview-* tokens |
| `6d7731d` | feat(lab-5x4t.3): migrate components/gateway to aurora tokens |
| `0f2abb7` | feat(lab-5x4t.4): migrate components/logs to aurora tokens |
| `6938158` | feat(lab-5x4t.2): migrate auth login-screen to aurora tokens |
| `3dd6734` | feat(lab-5x4t.1): add --aurora-hover-bg token |
| `0cc38fd` | refactor(lab-x2nj): move aurora tokens to components/aurora/ |
| `b37e766` | fix(lab-abch): activate shadow-aurora-* utilities |

### Highlights
- Full Aurora design token sweep across gateway-admin UI
- Aurora token module extracted to `components/aurora/tokens.ts`
- Deploy monitor scaffolding added

---

## [0.4.1] — 2026-04-21

| Commit | Change |
|--------|--------|
| `aec694f` | chore: bump version to 0.4.1 |
| `55c6c36` | feat(lab-17th.12): register CLI implementation and skill docs |
| `de0505e` | feat(lab-17th.12): register binary, systemd unit, monitor |
| `4ec80d9` | feat(lab-17th.11): axum router handlers and graceful shutdown |
| `2ececa7` | feat(lab-17th.10): flush pipeline with atomic writes and watermark |
| `58e43d7` | feat(lab-17th.9): JSONL notification line enum with atomic append |
| `bd932e4` | feat(lab-17th.8): per-PR debouncer with generation counter |
| `4744429` | feat(lab-17th.7): digest rendering with dynamic fences |
| `64fb70e` | feat(lab-17th.6): GitHub REST client with pagination + SSRF guard |
| `1d2af2a` | feat(lab-17th.5): bounded FIFO delivery-id dedup cache |
| `591b583` | feat(lab-17th.4): typed event parsing with issue_comment PR filter |
| `35372f8` | feat(lab-17th.3): constant-time HMAC-SHA256 signature verification |
| `b7f5aad` | feat(lab-17th.2): config loader with redacted Debug and empty-secret rejection |
| `6c28391` | feat(lab-17th.1): scaffold gh-webhook crate |

### Highlights
- **gh-webhook crate** — full GitHub webhook ingestion pipeline: HMAC verification, event parsing, per-PR debouncer, digest renderer, atomic JSONL append, axum HTTP server
- Bounded FIFO dedup cache for delivery-id replay protection
- GitHub REST client with SSRF guard and 429 retry

---

## [0.4.0] — 2026-04-20

| Commit | Change |
|--------|--------|
| `48ee2db` | feat(lab-eixf.8): sandbox sections + token drift docs |
| `d4f16c9` | feat(lab-eixf.7): migrate Docs page to Aurora |
| `4cf7c99` | feat(lab-eixf.6): migrate Settings page to Aurora |
| `35a4426` | feat(lab-eixf.5): migrate Activity page to Aurora |
| `ffd67c4` | feat(lab-eixf.4): migrate Overview page to Aurora |
| `d6d1c76` | feat(lab-eixf.3): Aurora primitive variants (Card/Badge/Alert) |
| `0e5c410` | simplify: abort checks, deriveGatewayName extraction |
| `ebfbab9` | fix(lab-iwtf.13,19): gateway name validation and option handling |
| `7ac4bc6` | fix(lab-iwtf.7,10,13,15): installServer return type, polling fixes |
| `9c67663` | fix(lab-iwtf.3,4,14,17,18,29): SSRF probe, restart hazard, auth edge cases |
| `d8b71eb` | fix(lab-iwtf.6,12,16): HTTP 422 for SSRF kinds, replay-window fixes |
| `10fc672` | fix(lab-iwtf.2,8): popup user-activation and external-close fixes |
| `ea21977` | fix(lab-iwtf.1,5,9,11): OAuth patch drop, proxy_prompts dedup |
| `f39f119` | feat(cli): richer palette — violet categories, teal action names |
| `806f7f9` | feat(cli): premium palette + catalog/doctor renderers |

### Highlights
- Full Aurora migration for all gateway-admin pages (Overview, Activity, Settings, Docs)
- Aurora primitive component variants (Card, Badge, Alert)
- **mcpregistry security** — SSRF probe, replay-window guard, HTTP 422 error mapping
- OAuth upstream flow fixes (popup activation, external close, proxy_prompts dedup)
- Premium CLI output palette (violet categories, teal actions, semantic colors)
