---
date: 2026-05-31 09:48:29 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: be4f2f94193b0564b397ae0eba0bc08d6eb1ca0d
session id: e16ac36b-6690-4b12-b0b4-e6bcafb04dbb
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/e16ac36b-6690-4b12-b0b4-e6bcafb04dbb.jsonl
working directory: /home/jmagar/workspace/lab
beads: lab-yuc0q
---

# Gateway web UI "all servers down" — root cause and health-aware projection

## User Request
Four chained requests in one session: (1) fix the `mise WARN missing: pnpm@9.15.9`; (2) debug a `labby` log storm of `403 Forbidden: Host header is not allowed` from the `syslog` upstream; (3) "is this a cors thing… also all of the servers are disconnected in the gateway"; (4) investigate the web UI showing every server down (via `/vibin:webwright`), then make the gateway projection health-aware, deploy it, and clean up.

## Session Overview
- Resolved the mise warning: the `lab` project pins `pnpm = "9.15.9"` in `~/workspace/lab/.mise.toml`; installed that exact version (`mise install pnpm@9.15.9`).
- Root-caused the syslog 403 spam: labby's upstream pointed at the legacy host `syslog.example.com`, but the renamed backend (`cortex` container) only allowlists `cortex.example.com` in rmcp's DNS-rebinding guard. The on-disk config had already been repointed to `cortex`; the running container held a stale in-memory `syslog` upstream. A container restart cleared the zombie reprobe loop.
- Root-caused the web UI "all servers down": the gateway runs **lazy discovery** (catalog empty until first `search`/`execute`), and `server_view_from_upstream` derived `connected` solely from `exposed_tool_count > 0`, so at rest the fleet rendered all-Disconnected even though servers were reachable (health heartbeats succeeding).
- Implemented a health-aware fix in `projection.rs`, added two unit tests, built with `just dev-debug`, hot-swapped the container, and verified the UI shows 24/26 healthy at rest.

## Sequence of Events
1. Diagnosed the mise warning to the project-level `.mise.toml` pin and installed `pnpm@9.15.9`; verified `mise current pnpm` → `9.15.9`.
2. Reproduced the syslog 403 with `curl` (unauth → 401 at Cloudflare OAuth; authed → 403 "Host header is not allowed" from the rmcp backend); traced the string to `rmcp .../streamable_http_server/tower.rs:241` and `syslog-mcp`/`cortex` `with_allowed_hosts`.
3. Found the deployed backend is the `cortex` container (`CORTEX_PUBLIC_URL=https://cortex.example.com`, no `CORTEX_ALLOWED_HOSTS`); confirmed `cortex.example.com/mcp` returns 200 with the bearer.
4. Discovered the on-disk `~/.labby/config.toml` upstream was already `cortex`; the running container still reprobed a stale `syslog` upstream. `lab gateway reload` only refreshes catalogs, not the pool. `docker restart labby` cleared the zombie.
5. Used `/vibin:webwright` (Playwright via uv venv + firefox) to load `http://localhost:8765`; captured that the fleet data is server-rendered (RSC) from `POST /v1/gateway {action:gateway.list}`, showing `26 total / 0 healthy / all Disconnected`.
6. Traced the metric to `projection.rs:250` (`connected = exposed_tool_count > 0`) reading a global catalog that is empty under lazy discovery ("discovery deferred until first use" log line). Confirmed: a `search` tool call warmed discovery → 21/26 connected, 143 tools.
7. Made `server_view_from_upstream` health-aware (mirroring `server_view_from_virtual_server`), added two tests, ran `cargo check`/`nextest`/`clippy`.
8. `just dev-debug` failed once on a concurrent agent's incomplete `mod code_mode_types;` (missing file); retried after the file appeared; build + `docker compose restart` succeeded.
9. Verified at rest: container `gateway.list` 24/26 connected; UI headline "Healthy Connections: 24" (was 0). Removed the 78M `outputs/labby-ui-debug/` investigation workspace.

## Key Findings
- `rmcp` host-allowlist rejection text originates at `crates/.../streamable_http_server/tower.rs:241` (`forbidden_response("Forbidden: Host header is not allowed")`); the backend allowlist is built from `CORTEX_PUBLIC_URL` only — `syslog.example.com` is not included.
- The host `lab` CLI builds its **own** in-process `GatewayManager` (`crates/lab/src/cli/gateway.rs:357` `build_manager`), so `lab gateway list` never reflects the running container — an earlier "20 connected" check measured the wrong process.
- `connected` for upstreams was `summary.exposed_tool_count > 0 || resources || prompts` (`crates/lab/src/dispatch/gateway/projection.rs:250`), reading the global catalog via `cached_upstream_summary` (`crates/lab/src/dispatch/upstream/pool.rs:2218`) — empty under lazy discovery.
- The virtual-server path already honored health: `connected = service_known && enabled && (peer_connected || health_connected)` (`projection.rs:331`). The upstream path ignored health — the asymmetry that caused the regression.
- Lazily-seeded upstreams default to `tool_health: Healthy`, `tool_last_error: None` (`pool.rs` `lazy_upstream_entry`); failed ones flip to `Unhealthy{consecutive_failures}` + recorded error. `UpstreamHealth::is_routable()` is true until the circuit breaker opens (≥3 failures, `types.rs:213`).

## Technical Decisions
- Made the upstream projection health-aware rather than disabling lazy discovery or eager-connecting all upstreams: `connected = exposing_capabilities || (last_error.is_none() && tool_health.is_routable())`. This keeps lazy discovery's startup benefit while making the fleet view reflect reachability.
- Used the already-computed operator-visible `last_error` for the health gate so benign capability errors (e.g. "method not found" for prompts/resources) do not mark a server down.
- Accepted optimistic semantics (seeded/unprobed → connected until a probe records a failure) because the user explicitly wanted "not everything Disconnected until first use." Logged the alternative (explicit tri-state) as bead lab-yuc0q.
- For the syslog issue, chose Option A (repoint labby to the canonical `cortex.example.com/mcp`) over Option B (add `CORTEX_ALLOWED_HOSTS=syslog.example.com`); the repoint was already on disk, so the remaining fix was clearing stale runtime state.

## Files Changed
| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | crates/lab/src/dispatch/gateway/projection.rs | — | Health-aware `connected` in `server_view_from_upstream` (+ health lookup) | `git diff --stat`: +15/−1 |
| modified | crates/lab/src/dispatch/gateway/manager.rs | — | Two new tests: `lazily_seeded_healthy_upstream_reports_connected_before_first_use`, `errored_upstream_reports_disconnected_even_when_circuit_closed` | `git diff --stat`: +43 |
| modified | bin/labby (binary, untracked-as-diff) | — | Overwritten by `just dev-debug` (nightly+cranelift debug build) and hot-swapped into the container | `install -D -m 755 target/debug/labby bin/labby` |

Non-repo state changes: `mise install pnpm@9.15.9` (toolchain cache, not a repo file). Investigation workspace `outputs/labby-ui-debug/` created then removed (untracked, 78M).

NOTE: the working tree contains many other dirty files (`code_mode.rs`, `code_mode_preamble.rs`, `code_mode_types.rs`, `dispatch.rs`, `pool.rs`, `types.rs`, `server.rs`, `cli/gateway.rs`, `gateway.rs`, `tests/*`, `docs/dev/CODE_MODE.md`, `docs/code-mode-cloudflare-enhancements.md`) from a **concurrent agent** editing this shared repo during the session — they are NOT part of this session's logical change. The `aurora-design-system` files were dirty before the session began.

## Beads Activity
| id | title | action | status | why |
|---|---|---|---|---|
| lab-yuc0q | Gateway UI: distinct 'Idle / not yet probed' state vs 'Disconnected' | created (P3, task) | open | Tracks the optional tri-state UI enhancement so the optimistic at-rest semantics can be made explicit later |

No other bead was created, closed, claimed, or commented during the session.

## Repository Maintenance
- Plans: `docs/plans/fleet-ws-plan-lab-n07n.md` (`Status: open`) and `docs/plans/mcp-streamable-http-oauth-proxy.md` are unrelated to this session and not clearly complete — left in place, not moved to `docs/plans/complete/`.
- Beads: created lab-yuc0q for the known optional follow-up (evidence: `bd create` returned `✓ Created issue: lab-yuc0q`). No completed beads to close were observed for this session.
- Worktrees/branches: `git worktree list --porcelain` shows the single `main` worktree at be4f2f94; `git branch -vv` shows only `main` tracking `origin/main`. Nothing stale to remove.
- Stale docs: did not edit shared docs — `docs/dev/CODE_MODE.md` is being actively modified by a concurrent agent, so a broad stale-docs pass was unsafe. No gateway health-display doc was updated; the behavior change is captured here and in lab-yuc0q.
- Transparency: only `projection.rs` and `manager.rs` are this session's source changes; all other dirty files belong to a concurrent agent and were left untouched.

## Tools and Skills Used
- Shell/Bash: reproduction `curl`s, `docker`/`docker compose`, `cargo check`/`nextest`/`clippy`, `git`, `mise`, `bd`, `just dev-debug`. Issue: one transient cargo "Blocking waiting for file lock" (concurrent build) — resolved by waiting.
- File tools (Read/Edit/Write): read `projection.rs`/`pool.rs`/`types.rs`/`view_models.rs`/frontend adapter; edited `projection.rs` and `manager.rs`; wrote this session note.
- Skills: `superpowers:systematic-debugging` (four-phase root-causing), `vibin:webwright` (Playwright browser investigation), `vibin:save-to-md` (this note).
- Browser: Playwright (firefox via `~/.cache/ms-playwright`, uv venv, `playwright==1.59`) to render the UI and capture RSC network calls.
- MCP/HTTP client: raw `httpx` streamable-HTTP MCP client against the container `/mcp` to trigger lazy discovery.
- Advisor: consulted before committing to the subject-scoping interpretation; it correctly redirected to read `upstream_summary` and check exposure-at-rest, which changed the conclusion.

## Commands Executed
| command | result |
|---|---|
| `mise install pnpm@9.15.9` | installed; `mise current pnpm` → `9.15.9` |
| `curl -H "Authorization: $AUTH" https://syslog.example.com/mcp` (initialize) | HTTP 403 `Forbidden: Host header is not allowed` |
| `curl … https://cortex.example.com/mcp` (initialize) | HTTP 200, MCP init result |
| `docker restart labby` | cleared stale `syslog` reprobe loop |
| `cargo check --workspace --all-features` | Finished, clean |
| `cargo nextest run … -E 'test(lazily_seeded…) or test(errored_upstream…)'` | 2 passed |
| `just dev-debug` | first run failed (missing `code_mode_types.rs`); retry built + restarted container |
| `curl … POST /v1/gateway {action:gateway.list}` (after deploy) | 24/26 connected at rest |
| `bd create --title=… --priority=3` | `✓ Created issue: lab-yuc0q` |

## Errors Encountered
- Webwright `explore_01.py` hung on `wait_until="networkidle"` and deadlocked calling `resp.text()` inside the response handler during `goto`. Root cause: SPA holds a long-lived SSE connection; reading bodies mid-navigation blocks. Fix: `domcontentloaded` + capture metadata only, fetch bodies after load (`explore_02.py`).
- `just dev-debug` first build: `error[E0583]: file not found for module code_mode_types`. Root cause: concurrent agent added `mod code_mode_types;` to `gateway.rs` before creating the file. Fix: container untouched (`set -euo pipefail` aborted before restart); retried once the file existed.

## Behavior Changes (Before/After)
| area | before | after |
|---|---|---|
| `syslog` upstream | repeating `403 Host header is not allowed` reprobe spam | removed (config repointed to `cortex`; zombie cleared by restart) |
| Gateway UI fleet at rest | 26 total / 0 healthy / all "Disconnected" | 26 total / 24 healthy; disabled servers (chrome-devtools, neo4j-memory) correctly excluded |
| `server_view_from_upstream.connected` | `exposed_tool_count > 0` only | `exposing_capabilities || (no error && circuit closed)` |

## Verification Evidence
| command | expected | actual | status |
|---|---|---|---|
| `mise current pnpm` | 9.15.9 | 9.15.9 | pass |
| authed `curl cortex.example.com/mcp` | 200 MCP init | 200 init result | pass |
| `cargo check --workspace --all-features` | clean | Finished, no errors | pass |
| `nextest` two new tests | pass | 2 passed | pass |
| container `gateway.list` after deploy | most servers connected | 24/26 connected | pass |
| UI overview after deploy | non-zero healthy | "Healthy Connections: 24" | pass |

## Risks and Rollback
- Optimistic at-rest semantics: an enabled-but-unreachable upstream shows connected until its first use records a failure. Mitigation/visibility: lab-yuc0q proposes an explicit tri-state.
- `bin/labby` is now a nightly+cranelift debug build (from `dev-debug`), not a release binary. Rollback: rebuild release (`just build-release`) or `git checkout` the source and rebuild; revert the `projection.rs`/`manager.rs` change to restore prior behavior.
- The source change is uncommitted; the working tree also holds a concurrent agent's edits. Commit scope must be limited to `projection.rs` and `manager.rs`.

## Decisions Not Taken
- Did not disable gateway-wide tool_search mode or force eager discovery — would defeat the lazy-discovery design and broaden scope.
- Did not add `CORTEX_ALLOWED_HOSTS=syslog.example.com` to the cortex container (Option B) — the canonical repoint to `cortex.example.com` was already in config.
- Did not edit shared docs (`CODE_MODE.md` etc.) — under active concurrent edit.

## References
- rmcp host guard: `crates/lab/docs/references/mcp-rust-sdk/repo.md` (and vendored rmcp `streamable_http_server/tower.rs:241`).
- `crates/lab/src/dispatch/gateway/projection.rs`, `crates/lab/src/dispatch/gateway/manager.rs`, `crates/lab/src/dispatch/upstream/pool.rs`, `crates/lab/src/dispatch/gateway/view_models.rs`.
- syslog→cortex history: `~/workspace/syslog-mcp/docs/sessions/2026-05-08-oauth-implementation-syslog-axon.md`.

## Open Questions
- Does the user want the explicit tri-state UI (lab-yuc0q) or is the optimistic boolean sufficient?
- Should the cortex deployment also add `CORTEX_ALLOWED_HOSTS=syslog.example.com` to keep the legacy alias usable for other clients, or is the canonical host the only supported entry point now?

## Next Steps
- Commit this session's source change with a path-limited stage: `git add -- crates/lab/src/dispatch/gateway/projection.rs crates/lab/src/dispatch/gateway/manager.rs` then commit — explicitly excluding the concurrent agent's dirty files. Verify with `git diff --cached --name-only` before committing.
- Run the full suite once the concurrent agent's tree compiles cleanly: `just test` (`cargo nextest run --workspace --all-features`).
- Decide on lab-yuc0q (tri-state) vs keeping the optimistic boolean.
- If a release artifact is needed, rebuild `bin/labby` via `just build-release` (the current binary is a debug build).
