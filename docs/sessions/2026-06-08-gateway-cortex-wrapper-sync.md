---
date: 2026-06-08 16:03:13 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 588ac80d
session id: abba9d8d-e1f3-46c8-9b06-a5359b0a88d3
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/abba9d8d-e1f3-46c8-9b06-a5359b0a88d3.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab 588ac80dc0ee1541efc55a3ed7fc75c3ce74e334 [main]
beads: lab-vg9of
---

# Gateway stdio lifecycle, Cortex env portability, and binary sync wrappers

## User Request

The session began with a request to check whether Labby was properly multiplexing stdio MCP servers because multiple stdio server processes appeared to be spawning when only one should exist per proxied upstream. Later requests focused on Cortex failing under the Labby gateway, making Cortex's regular `cortex mcp` command portable, and copying Axon's automatic binary sync pattern into Lab, Cortex, and rmcp-template.

## Session Overview

We diagnosed and fixed Labby's stdio MCP lifecycle leak, diagnosed Cortex's stdio startup failure inside the Labby container, made Cortex infer the proper `.cortex` home from a user-owned binary path, and added Axon-style Cargo wrapper binary sync behavior across Lab, Cortex, and rmcp-template. The session ended by wiring the new wrapper check into rmcp-template's pre-release gate and Lab/Cortex lint gates.

## Sequence of Events

1. Investigated Labby's stdio gateway process behavior and confirmed repeated connection tests could spawn excess stdio MCP children.
2. Killed runaway MCP-related processes, shut down Labby and the llama server when memory pressure became urgent, then restarted controlled debugging.
3. Implemented Lab gateway lifecycle fixes for stdio discovery, probe task registration, and ephemeral test pools.
4. Built and installed the Lab release binary to the repo, plugin path, user PATH, and container-mounted runtime path; committed and pushed the Lab stdio lifecycle fix earlier in the session.
5. Investigated Cortex's failed Labby connection test and found the plain `/home/jmagar/.local/bin/cortex mcp` command was looking at `/home/labby/.cortex` inside the container.
6. Fixed Cortex to infer `/home/<user>/.cortex` from a user-home binary path when `$HOME/.cortex` is absent, then verified the gateway test succeeds with the plain command.
7. Compared Axon, Lab, Cortex, and rmcp-template build/install flows and copied Axon's Cargo wrapper pattern into Lab, Cortex, and rmcp-template.
8. Tightened the wrapper implementation after discovering a dependency-local `rust-toolchain.toml` could make `rustup which rustc` resolve Rust 1.56.0 from inside a dependency directory.
9. Added `just test-cargo-wrapper` in all three repos and wired it into rmcp-template's pre-release gate plus Lab/Cortex lint.
10. Ran the save-to-md maintenance pass and wrote this path-limited session artifact.

## Key Findings

- Labby's stdio test path needed an ephemeral discovery lifecycle so connection tests do not leave long-lived stdio children behind.
- Cortex's config loading was not deterministic under Labby's container because the container user had `HOME=/home/labby`, while the mounted runtime config lived under `/home/jmagar/.cortex`.
- The failed Cortex connection was not fixed by a gateway restart alone; `/home/jmagar/.local/bin/cortex` was still an old `cortex 1.8.0` binary until it was replaced with the newly built `1.15.0` binary.
- Axon's closest automatic binary sync pattern lives in `scripts/cargo-rustc-wrapper`, `.cargo/config.toml`, and `Justfile` recipes such as `link-bin`, `build-plugin`, and `sync-container`.
- A dependency crate (`ff-0.13.1`) includes its own `rust-toolchain.toml` pinned to `1.56.0`; wrappers must resolve rustc from the workspace root, not the dependency working directory.
- The latest injected Claude transcript existed but described an older screenshots task, not this Codex session, so it was recorded as stale transcript evidence rather than used as the primary session source.

## Technical Decisions

- Lab gateway connection tests use ephemeral stdio discovery while normal upstream discovery remains long-lived.
- Cortex's env/home fix is general to binaries under `/home/<user>/...`; it is not hardcoded to the Labby container or to `/home/jmagar/.local/bin` specifically.
- The wrapper sync default copies completed binary builds to both `~/.local/bin/<binary>` and `plugins/<service>/bin/<binary>`, matching Axon's behavior.
- The Justfile `link-bin` recipes use symlinks to the release binary for PATH and plugin cache slots, while explicit plugin bundle recipes still install into tracked plugin directories.
- The wrapper check is fast and fake-rustc based, so it was added to rmcp-template's release gate and Lab/Cortex lint without forcing full release builds.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `/home/jmagar/workspace/lab/.cargo/config.toml` | - | Enable repo-local Cargo wrapper. | `git diff -- .cargo/config.toml` |
| modified | `/home/jmagar/workspace/lab/Justfile` | - | Add `test-cargo-wrapper`, `link-bin`, `build-plugin`, and lint dependency wiring. | `just --show lint` |
| modified | `/home/jmagar/workspace/lab/plugins/labby/bin/labby` | - | Local plugin binary artifact changed during release/binary sync work. | `git status --short` |
| created | `/home/jmagar/workspace/lab/scripts/cargo-rustc-wrapper` | - | Copy completed `labby` builds to PATH and plugin bundle destinations. | `bash scripts/test-cargo-rustc-wrapper.sh` |
| created | `/home/jmagar/workspace/lab/scripts/test-cargo-rustc-wrapper.sh` | - | Fast fake-rustc wrapper behavior test. | `just test-cargo-wrapper` |
| modified | `/home/jmagar/workspace/cortex/.cargo/config.toml` | - | Enable repo-local Cargo wrapper with `.cache/cargo` target-dir preserved. | `git -C ../cortex diff -- .cargo/config.toml` |
| modified | `/home/jmagar/workspace/cortex/Justfile` | - | Add release/link/install wrapper flows and lint wrapper check. | `git -C ../cortex diff -- Justfile` |
| modified | `/home/jmagar/workspace/cortex/src/setup.rs` | - | Infer Cortex home from a user-home executable path when `$HOME/.cortex` is absent. | `cargo test setup::tests:: --lib -- --nocapture` |
| modified | `/home/jmagar/workspace/cortex/src/setup_tests.rs` | - | Cover home inference from `.local/bin`, workspace paths, and non-home binaries. | `cargo test setup::tests:: --lib -- --nocapture` |
| created | `/home/jmagar/workspace/cortex/scripts/cargo-rustc-wrapper` | - | Copy completed `cortex` builds to PATH and plugin bundle destinations. | `just test-cargo-wrapper` |
| created | `/home/jmagar/workspace/cortex/scripts/test-cargo-rustc-wrapper.sh` | - | Fast fake-rustc wrapper behavior test. | `just test-cargo-wrapper` |
| modified | `/home/jmagar/workspace/rmcp-template/.cargo/config.toml` | - | Enable repo-local Cargo wrapper in the family template. | `git -C ../rmcp-template diff -- .cargo/config.toml` |
| modified | `/home/jmagar/workspace/rmcp-template/Justfile` | - | Add wrapper test and link-bin flow for the lightweight `rtemplate` binary. | `just test-cargo-wrapper` |
| modified | `/home/jmagar/workspace/rmcp-template/scripts/pre-release-check.sh` | - | Add `cargo wrapper binary sync` to the pre-release gate. | `scripts/pre-release-check.sh --skip-verify --skip-build-plugin` |
| created | `/home/jmagar/workspace/rmcp-template/scripts/cargo-rustc-wrapper` | - | Template wrapper for future MCP servers. | `just test-cargo-wrapper` |
| created | `/home/jmagar/workspace/rmcp-template/scripts/test-cargo-rustc-wrapper.sh` | - | Template wrapper behavior test. | `just test-cargo-wrapper` |
| created | `/home/jmagar/workspace/lab/docs/sessions/2026-06-08-gateway-cortex-wrapper-sync.md` | - | Session artifact. | This file |

## Beads Activity

| id | title | action | final status | why |
|---|---|---|---|---|
| `lab-vg9of` | not queried in detail during save pass | observed closed | closed | Recent interactions show it was closed with reason `Fixed gateway.test stdio reprobe leak; targeted gateway tests pass`, matching the Lab stdio lifecycle work. |

No new bead mutations were made during the save pass. The work spans three repositories and the save-to-md contract requires committing only this generated session artifact, so follow-up tracking was recorded in Next Steps instead of dirtying `.beads/`.

## Repository Maintenance

### Plans

Checked `docs/plans/`. `docs/plans/fleet-ws-plan-lab-n07n.md` is explicitly tied to an open bead and active future phases, so it was not moved. `docs/plans/mcp-streamable-http-oauth-proxy.md` describes broad MCP/OAuth/proxy work and was not clearly completed by this session, so it was not moved.

### Beads

Ran `bd list --all --sort updated --reverse --limit 100 --json` and tailed `.beads/interactions.jsonl`. No bead updates were made in the save pass. Recent relevant evidence was the `lab-vg9of` closure for the stdio reprobe leak.

### Worktrees and branches

Checked `git worktree list --porcelain`, local branches, remote branches, and merge ancestry. Two non-main worktrees were present: `/home/jmagar/workspace/lab/.claude/worktrees/heuristic-roentgen-5e827a` on `pr-98` and `/home/jmagar/workspace/lab/.worktrees/codex/code-mode-artifacts` on `codex/code-mode-artifacts`; neither HEAD was an ancestor of `main`, and one worktree was dirty, so neither was removed.

### Stale docs

No stale documentation was safely updated during the save pass. The most obvious documentation follow-up is to decide whether the new Cargo wrapper pattern should be documented in each repo's Rust or contributor docs after the code changes are committed.

### Transparency

No completed plans were moved. No beads, worktrees, or branches were modified. No docs outside this session artifact were changed during the save pass.

## Tools and Skills Used

- **Skills.** Used `superpowers:systematic-debugging`, `superpowers:test-driven-development`, `superpowers:verification-before-completion`, and `vibin:save-to-md`.
- **Shell commands.** Used `git`, `rg`, `sed`, `cargo`, `just`, `docker`, `curl`, `sha256sum`, `install`, `bd`, `gh`, and standard file/status commands.
- **File tools.** Used patch-based file edits for source, scripts, Justfiles, and this session artifact.
- **MCP tools.** Attempted Lumen semantic search; it failed with `Transport closed`, so exact `rg` and file reads were used instead.
- **External services.** Used the Labby HTTP API through `curl` to run `gateway.test cortex`.
- **Browser tools/subagents.** No browser automation or subagents were used in the final wrapper work.

## Commands Executed

| command | result |
|---|---|
| `cargo test setup::tests:: --lib -- --nocapture` in `/home/jmagar/workspace/cortex` | 38 setup tests passed after adding Cortex home inference tests. |
| `cargo fmt --all --check` in `/home/jmagar/workspace/cortex` | Passed during Cortex verification. |
| `cargo build --release` in `/home/jmagar/workspace/cortex` | Built `cortex 1.15.0`. |
| `install -m 755 /home/jmagar/workspace/cortex/.cache/cargo/release/cortex /home/jmagar/.local/bin/cortex` | Replaced the old PATH binary. |
| `docker exec labby /home/jmagar/.local/bin/cortex --version` | Confirmed the container saw `cortex 1.15.0`. |
| `curl ... /v1/gateway` with `gateway.test cortex` | Returned `tool_count: 1`, `resource_count: 3`, `prompt_count: 12`, `last_error: null`. |
| `bash scripts/test-cargo-rustc-wrapper.sh` in Lab, Cortex, rmcp-template | Passed in all three repos. |
| `cargo rustc ... -- -C extra-filename=-wrapper-smoke` in Lab, Cortex, rmcp-template | Real Cargo wrapper smokes passed with temp install destinations. |
| `just test-cargo-wrapper` in Lab, Cortex, rmcp-template | Passed in all three repos. |
| `scripts/pre-release-check.sh --skip-verify --skip-build-plugin` in rmcp-template | Executed the new wrapper check and it passed; unrelated existing pre-release checks failed. |
| `git diff --check` in Lab, Cortex, rmcp-template | Clean in all three repos after wrapper changes. |

## Errors Encountered

- Labby stdio MCP tests could leave extra stdio server processes; fixed with explicit ephemeral discovery/test lifecycle handling.
- Cortex stdio failed in Labby with `Permission denied (os error 13)` because the process looked under `/home/labby/.cortex` instead of the mounted operator config under `/home/jmagar/.cortex`; fixed by portable home inference and replacing the stale PATH binary.
- A fresh verification initially still showed `cortex 1.8.0`; root cause was the live `/home/jmagar/.local/bin/cortex` had not been replaced even though the repo release artifact had been built.
- `strace` was not installed in the Labby container, so direct file tracing was unavailable.
- Lumen semantic search returned `Transport closed`; exact repo search was used instead.
- Cortex's real wrapper smoke initially failed with `Unrecognized option: 'check-cfg'`; root cause was dependency-local `ff-0.13.1/rust-toolchain.toml` causing `rustup which rustc` to resolve Rust 1.56.0 when run from the dependency directory. The wrappers now resolve rustc from the workspace root.
- rmcp-template's broader pre-release gate has unrelated existing failures in plugin layout, schema docs, OpenAPI docs, template feature smoke, and version sync.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Labby stdio tests | Gateway connection tests could spawn and retain extra stdio child processes. | Test discovery uses ephemeral lifecycle and drains temporary pools. |
| Cortex stdio under Labby | Plain `/home/jmagar/.local/bin/cortex mcp` failed inside the container. | Plain command starts normally and gateway test succeeds. |
| Cortex config home | Only `CORTEX_HOME` or process `HOME` drove `.cortex` lookup. | Existing `$HOME/.cortex` still wins; otherwise a user-home binary path can infer `/home/<user>/.cortex` when that config exists. |
| Lab/Cortex/rmcp-template builds | Build outputs could drift from PATH and plugin binaries. | Cargo wrappers and Justfile recipes sync fresh binaries to PATH/plugin destinations. |
| rmcp-template release gate | Did not explicitly check wrapper sync behavior. | `pre-release-check.sh` now runs `just test-cargo-wrapper`. |
| Lab/Cortex lint | Did not exercise wrapper sync behavior. | Lint gates include `test-cargo-wrapper`. |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo test setup::tests:: --lib -- --nocapture` | Cortex setup tests pass. | 38 passed. | pass |
| `cargo build --release` in Cortex | Release binary builds. | Built `cortex 1.15.0`. | pass |
| `docker exec labby /home/jmagar/.local/bin/cortex --version` | Container sees rebuilt Cortex. | `cortex 1.15.0`. | pass |
| `gateway.test cortex` via Labby HTTP API | Cortex stdio discovery succeeds. | 1 tool, 3 resources, 12 prompts, `last_error: null`. | pass |
| `bash scripts/test-cargo-rustc-wrapper.sh` in all three repos | Wrapper tests pass. | Passed in Lab, Cortex, rmcp-template. | pass |
| Real `cargo rustc` wrapper smoke in all three repos | Cargo invokes wrapper and temp binaries match. | Passed in Lab, Cortex, rmcp-template. | pass |
| `just test-cargo-wrapper` in all three repos | Repo-native wrapper check passes. | Passed in Lab, Cortex, rmcp-template. | pass |
| `scripts/pre-release-check.sh --skip-verify --skip-build-plugin` in rmcp-template | New wrapper check executes. | Wrapper check passed; unrelated checks failed. | warn |
| `git diff --check` in all three repos | No whitespace errors. | Clean. | pass |

## Risks and Rollback

- The Cargo wrappers copy compiled binaries automatically. Roll back by removing `rustc-wrapper = "scripts/cargo-rustc-wrapper"` from `.cargo/config.toml` and deleting the new wrapper scripts.
- Plugin bundle binaries are large and may be Git LFS tracked. Review binary diffs before committing implementation changes.
- Lab and Cortex lint now run the wrapper smoke first. If CI environments lack required shell utilities, either install them or move the check to a release-only gate.
- rmcp-template's pre-release gate currently has unrelated failures; do not treat that full gate as newly broken by the wrapper check without comparing pre-existing state.

## Decisions Not Taken

- Did not keep Cortex gateway configured as `env CORTEX_HOME=... cortex mcp`; the user wanted the regular command to be portable.
- Did not hardcode Cortex to Labby's container paths; the implemented fallback derives from a user-home binary path and only uses the inferred home if `.cortex` exists.
- Did not delete Lab worktrees or branches because merge ancestry and dirty-state evidence did not prove they were safe to remove.
- Did not move plan files because neither observed plan was clearly completed.
- Did not wire `test-cargo-wrapper` into every plain `test` command; it was added to rmcp-template pre-release and Lab/Cortex lint as requested.

## References

- `/home/jmagar/workspace/axon/scripts/cargo-rustc-wrapper`
- `/home/jmagar/workspace/axon/Justfile`
- `/home/jmagar/workspace/lab/CLAUDE.md`
- `/home/jmagar/workspace/cortex/src/setup.rs`
- `/home/jmagar/workspace/rmcp-template/scripts/pre-release-check.sh`
- `/home/jmagar/.claude/projects/-home-jmagar-workspace-lab/abba9d8d-e1f3-46c8-9b06-a5359b0a88d3.jsonl` (read during save pass; observed to describe an older screenshots session)

## Open Questions

- Should the new wrapper behavior be documented in each repo's Rust/contributor docs after the implementation changes are committed?
- Should follow-up beads be created in each affected repository once the implementation commits are being prepared?
- Should the rmcp-template unrelated pre-release failures be fixed before landing the wrapper pattern into the template?
- Should active plugin cache sync also cover Codex plugin cache paths, or remain aligned with Axon's current Claude-cache behavior?

## Next Steps

- Review, stage, commit, and push the implementation changes in `/home/jmagar/workspace/lab`, `/home/jmagar/workspace/cortex`, and `/home/jmagar/workspace/rmcp-template` separately so unrelated WIP stays out of each commit.
- In Lab, inspect `plugins/labby/bin/labby` before committing because it is a binary artifact.
- In Cortex, include both the portable home inference fix and wrapper sync changes in the implementation commit if they are accepted together.
- In rmcp-template, decide whether to first fix the existing plugin layout/schema/OpenAPI/template/version failures or land the wrapper pattern separately with the known failures documented.
- After implementation commits, run the relevant release/lint gates again and verify installed binaries in `~/.local/bin` and plugin paths.
