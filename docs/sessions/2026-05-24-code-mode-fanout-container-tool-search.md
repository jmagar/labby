---
date: 2026-05-24 19:33:47 EDT
repo: git@github.com:jmagar/lab.git
branch: main
head: 9ace94d0
session id: 019e5bf6-659e-7631-bfc0-d72182adffee
transcript: /home/jmagar/.codex/sessions/2026/05/24/rollout-2026-05-24T17-48-55-019e5bf6-659e-7631-bfc0-d72182adffee.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab 9ace94d0 [main]
beads: lab-le0w0, lab-le0w0.1, lab-le0w0.2, lab-le0w0.3, lab-le0w0.4, lab-le0w0.5
---

## User Request

The session started with the gateway Code Mode implementation: it worked, but multiple tool calls were serialized instead of fanning out. Follow-up requests covered whether Code Mode uses JS or TS, rebuilding and installing the debug binary, live `mcporter` testing against `lab.example.com/mcp`, explaining whether `code_search` uses vectors, and fixing container URL normalization for Qdrant/TEI.

## Session Overview

- Fixed Code Mode runner/broker behavior so `Promise.all([...callTool(...)])` can emit multiple tool calls before waiting for results.
- Verified live MCP Code Mode fan-out through `mcporter` against `lab-prod`.
- Confirmed runtime Code Mode is JavaScript via Boa, not TypeScript.
- Built the debug binary, installed it to `bin/labby` and `~/.local/bin/labby`, and restarted/recreated the Lab container.
- Fixed semantic `tool_search` URL resolution so host-loopback Qdrant/TEI URLs normalize to Docker DNS names inside the Lab container.

## Sequence of Events

1. Inspected `crates/lab/src/dispatch/gateway/code_mode.rs` and `crates/lab/src/mcp/server.rs` after the user reported serialized Code Mode calls.
2. Reworked the sandbox JS bridge to return pending `JsPromise`s, track `pending_calls`, and resolve or reject each promise when the parent returns the matching sequence result.
3. Reworked the MCP-side parent broker loop to run pending tool calls concurrently with `FuturesUnordered` while continuing to read additional runner protocol messages.
4. Added a runner test proving two `Promise.all` tool calls are emitted before either result is returned.
5. Ran focused tests, built the debug binary through `just dev-debug`, copied it to the host PATH, and verified host/container versions and hashes.
6. Used `mcporter` against `lab-prod` for read-only live calls: `code_search`, `code_schema`, `invoke`, and `code_execute` fan-out.
7. Investigated `code_search` internals and corrected the initial interpretation of `gateway.scout.get`: the returned config struct did not include env-resolved Qdrant/TEI URLs.
8. Patched `ToolSearchConfig` URL resolution to normalize loopback Qdrant/TEI URLs to Docker DNS names when Lab runs in a container, then attached Lab to the external Axon network.

## Key Findings

- Code Mode runtime is JavaScript, not TypeScript. TypeScript is only generated client/type surface; the sandbox is Boa JS.
- The serialization bug was in both halves of the bridge: JS `callTool` blocked on a result, and the parent broker awaited each tool call before reading the next runner message.
- The new JS side stores promise resolvers in `pending_calls` and returns immediately from `callTool` (`crates/lab/src/dispatch/gateway/code_mode.rs:192`, `crates/lab/src/dispatch/gateway/code_mode.rs:346`).
- The parent side now uses `FuturesUnordered` for concurrent tool execution (`crates/lab/src/mcp/server.rs:3141`).
- `ToolSearchConfig::resolved_qdrant_url()` and `resolved_tei_url()` already used env fallback, but env-visible loopback URLs were wrong inside Docker (`crates/lab/src/config.rs:541`, `crates/lab/src/config.rs:566`).
- Lab had to join the Axon Docker network so `axon-qdrant` and `axon-tei` DNS names resolve (`docker-compose.prod.yml:116`, `docker-compose.prod.yml:141`, `docker-compose.yml:41`).

## Technical Decisions

- Kept `callTool(id, params)` as the JS API and changed its behavior to a real promise instead of adding new batch syntax.
- Enforced `max_tool_calls` on started calls in the parent broker so fan-out cannot bypass the configured tool-call budget.
- Preserved host `.env` loopback URLs for host CLI usage and normalized only when container runtime is detected.
- Used Docker DNS targets `axon-qdrant:6333` and `axon-tei:80` because the existing Axon compose stack already exposes those service/container names.
- Added the Axon network to compose rather than relying on an undocumented manual `docker network connect`.

## Files Changed

| status | path | previous path | purpose | evidence |
| --- | --- | --- | --- | --- |
| modified | `crates/lab/src/dispatch/gateway/code_mode.rs` | | JS runner promises and out-of-order result handling | `rg pending_calls` |
| modified | `crates/lab/src/mcp/server.rs` | | Parent broker fan-out with `FuturesUnordered` | `rg FuturesUnordered` |
| modified | `crates/lab/tests/code_mode_runner.rs` | | Regression test for Promise.all fan-out | `code_mode_runner_fans_out_promise_all_tool_calls` |
| modified | `crates/lab/src/config.rs` | | Container-aware Qdrant/TEI URL normalization and tests | `rg normalize_container_loopback_url` |
| modified | `docker-compose.prod.yml` | | Attach Lab to external Axon network | `docker compose config` |
| modified | `docker-compose.yml` | | Dev overlay declares external Axon network | `docker compose config` |
| modified | `Justfile` | | Dirty at session close; not changed during the final URL-normalization patch | `git status --short` |
| created | `docs/sessions/2026-05-24-code-mode-fanout-container-tool-search.md` | | This session note | current file |
| untracked | `docs/superpowers/plans/2026-05-24-code-mode-dispatch-refactor.md` | | Existing untracked implementation plan observed during save pass | `git status --short` |
| untracked | `docs/sessions/2026-05-24-stdio-parity-merge-deploy.md` | | Existing untracked session note observed during save pass | `git status --short` |
| untracked | `docs/sessions/2026-05-24-workstation-extension-auth-search-crawl.md` | | Existing untracked session note observed during save pass | `git status --short` |

## Beads Activity

Observed bead activity, not changed during the final save pass:

| bead | title | action(s) | final status | why it mattered |
| --- | --- | --- | --- | --- |
| `lab-le0w0` | Code Mode epic | observed closed in `.beads/interactions.jsonl` | closed | Parent Code Mode work item for the feature area touched in this session. |
| `lab-le0w0.1` through `lab-le0w0.5` | Code Mode child tasks | observed closed in `.beads/interactions.jsonl` | closed | Closure reason says Code Mode contract, schema/bindings, brokered policy checks, constrained executor, config/docs, and verification were completed. |

No bead create/edit/close commands were run in this save pass. The `bd list` output was extremely broad and mostly historical; `.beads/interactions.jsonl` provided the relevant recent Code Mode closure evidence.

## Repository Maintenance

- Plans: inspected `docs/plans/` and `docs/superpowers/plans/`. No plan was moved because `docs/superpowers/plans/2026-05-24-code-mode-dispatch-refactor.md` is still untracked and the implementation is not fully aligned with that plan's larger dispatch-refactor scope.
- Beads: read recent bead state and interactions. No bead changes were made because no open directly relevant bead was identified from the sampled evidence.
- Worktrees/branches: inspected registered worktrees and branches. Left `/home/jmagar/workspace/lab/.worktrees/code-mode-dispatch-refactor` and `feat/code-mode-dispatch-refactor` untouched because the worktree is registered and ownership/current purpose was not proven obsolete.
- Stale docs: scanned docs for Code Mode/tool-search references. No docs were updated during the save pass; the immediate runtime mismatch was fixed in code and compose.
- Cleanup skipped: did not remove untracked session docs or the untracked Code Mode plan because they appeared to be user/session artifacts and were outside the direct requested save operation.

## Tools and Skills Used

- Skills: `save-to-md` for this session note; `mcporter` earlier for live MCP calls.
- Shell commands: `rg`, `sed`, `git`, `docker compose`, `docker inspect`, `curl`, `cargo test`, `cargo fmt`, `just dev-debug`, `install`, `sha256sum`, `mcporter`.
- MCP/external CLI: `mcporter` called `lab-prod.code_search`, `lab-prod.code_schema`, `lab-prod.invoke`, and `lab-prod.code_execute`.
- File tools: `apply_patch` for code, compose, and this session note.
- Browser tools/subagents: none used in this session.
- Issues encountered: one local ad-hoc `mcporter` call to `http://127.0.0.1:8765/mcp` failed with protected-resource mismatch because the server advertised `https://lab.example.com/mcp`; live prod calls were used instead.

## Commands Executed

Critical commands and observed results:

```bash
cargo test -p labby code_mode_runner --test code_mode_runner
# passed: 2 tests

cargo test -p labby code_mode --lib
# passed: 15 tests

just dev-debug
# built debug labby, installed target/debug/labby to bin/labby, restarted container

install -D -m 755 target/debug/labby ~/.local/bin/labby
sha256sum target/debug/labby bin/labby ~/.local/bin/labby
# all three hashes matched: 7f6b8e220a3ebe4c10f219862523842b93bccff9d68d6a73b7e1df22be3c2e76

mcporter call lab-prod.code_execute --args '{"code":"const [a, b, c] = await Promise.all([...]);","max_tool_calls":3}' --output text
# returned all three calls, proving live fan-out through prod MCP

cargo test -p labby tool_search_ --lib
# passed: 15 tests, 1 ignored

docker compose -f docker-compose.yml config
# validated after adding the external axon network to the dev overlay

docker compose -f docker-compose.yml exec -T labby-master sh -lc 'getent hosts axon-qdrant axon-tei; curl ...'
# resolved axon-qdrant/axon-tei and reached Qdrant lab-tools plus TEI health
```

## Errors Encountered

- Initial Code Mode implementation serialized calls because both JS and parent broker paths blocked per call. Fixed by returning JS promises and brokering parent tool calls concurrently.
- `gateway.scout.get` showed `qdrant_url:null` and `tei_url:null`; this was initially misread as semantic search disabled. Root cause: the action serializes configured fields, not env-resolved fallback.
- The Lab container had `QDRANT_URL=http://127.0.0.1:53333` and `TEI_URL=http://127.0.0.1:52000`, but container localhost could not reach those services. Fixed by runtime normalization to Docker DNS and compose network attachment.
- First compose validation after adding the prod network failed because the dev overlay had its own root `networks` section and did not declare `axon`. Fixed by declaring `axon` in `docker-compose.yml`.
- Recreating with default compose env put the container on `lab` instead of the previous `jakenet`; manually reconnected `jakenet` after observing `DOCKER_NETWORK` was not set in repo or Lab env files.

## Behavior Changes (Before/After)

| area | before | after |
| --- | --- | --- |
| Code Mode `Promise.all` | Tool calls were serialized through JS and parent broker waits. | Multiple calls can be emitted and executed concurrently, then resolved by sequence. |
| Tool-call limit | Serialized path naturally limited one at a time. | Parent tracks started calls and rejects attempts beyond `max_tool_calls`. |
| Qdrant/TEI in container | Env fallback preserved host-loopback URLs that failed from inside Docker. | Container runtime rewrites loopback Qdrant/TEI URLs to `axon-qdrant` and `axon-tei`. |
| Lab Docker networking | Lab did not share the Axon network through compose. | Compose attaches Lab to external `axon` network. |

## Verification Evidence

| command | expected | actual | status |
| --- | --- | --- | --- |
| `cargo test -p labby code_mode_runner --test code_mode_runner` | Runner tests pass | 2 passed | pass |
| `cargo test -p labby code_mode --lib` | Code Mode lib tests pass | 15 passed | pass |
| `mcporter call lab-prod.code_execute ... Promise.all ...` | Multiple calls returned | 2-call and 3-call fan-out returned results | pass |
| `cargo fmt --all --check` | Formatting clean | exited 0 | pass |
| `cargo test -p labby tool_search_ --lib` | Tool-search/config tests pass | 15 passed, 1 ignored | pass |
| `docker compose -f docker-compose.yml config` | Compose validates | exited 0 after dev network fix | pass |
| `docker inspect labby ... jq keys` | Lab attached to needed networks | `axon`, `jakenet`, `lab` observed after manual reconnect | pass |
| `curl http://axon-qdrant:6333/collections/lab-tools` from container | Qdrant reachable | returned green collection status | pass |
| `curl http://axon-tei:80/health` from container | TEI reachable | exited 0 | pass |

## Risks and Rollback

- Risk: hard-coded Docker DNS defaults assume the Axon stack/service names `axon-qdrant` and `axon-tei`. Rollback: revert the `config.rs` normalization helpers and compose network changes, or set explicit non-loopback `[tool_search]` URLs.
- Risk: `docker-compose.yml` now references external network `axon`. If the Axon stack is absent, compose will fail until the network exists or `AXON_DOCKER_NETWORK` points at the correct network.
- Risk: the manual `docker network connect jakenet labby` restored current runtime state but is not captured as a durable compose setting because `jakenet` exists without compose labels. Durable fix would require declaring the Lab network external when `DOCKER_NETWORK=jakenet` is intended.

## Decisions Not Taken

- Did not make Code Mode TypeScript-native; runtime stays JavaScript because Boa executes JS and the existing generated TS is a client/types layer.
- Did not normalize all service URLs globally; scoped the change to tool-search Qdrant/TEI because that was the observed broken path and has known Docker DNS targets.
- Did not delete the `feat/code-mode-dispatch-refactor` worktree or branch; no safe-obsolete proof was gathered.

## References

- `crates/lab/src/dispatch/gateway/code_mode.rs`
- `crates/lab/src/mcp/server.rs`
- `crates/lab/tests/code_mode_runner.rs`
- `crates/lab/src/config.rs`
- `docker-compose.prod.yml`
- `docker-compose.yml`
- `/home/jmagar/workspace/axon_rust/docker-compose.prod.yaml` for Axon service names and ports.
- `.env` / `~/.labby/.env` Qdrant and TEI URL lines.

## Open Questions

- Whether `DOCKER_NETWORK=jakenet` should be made explicit and durable for this repo's compose invocation, given the current network-label mismatch when trying to use it as a Compose-managed network.
- Whether `gateway.scout.get` should return effective/resolved tool-search URLs in a redacted form so operators do not confuse configured `null` with disabled env fallback again.
- Whether semantic search should log an explicit startup/readiness line showing resolved Qdrant/TEI reachability.

## Next Steps

- Decide whether to make `jakenet` an explicit external Lab network in compose or keep using Compose's managed `lab` network plus manual/operational attachments.
- Consider adding a follow-up bead for redacted effective tool-search config reporting.
- Run a full all-features check before committing if this branch is headed to PR: `just check` or `cargo check --workspace --all-features`.
- If shipping the current runtime, keep the container attached to both `axon` and whichever operator network should expose Lab to sibling services.
