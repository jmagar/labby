---
date: 2026-05-24 00:15:41 EST
repo: git@github.com:jmagar/lab.git
branch: fix/gateway-oauth-tool-gating
head: 3bc9faac
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
pr: none
---

# Gateway Invoke Disambiguation Session

## User Request

The user wanted Lab tightened so agents can invoke the correct Windows MCP upstream through the gateway when multiple upstreams expose the same raw tool name. They also asked whether the tool path had logging and helpful error messages for agent course correction, then requested an MCP boundary test and this session capture.

## Session Overview

Implemented explicit upstream disambiguation for gateway `invoke`, improved structured error guidance, documented the `ambiguous_tool` envelope, and added an in-process MCP server test that verifies the exact agent-visible JSON returned for duplicate upstream tool names.

## Sequence of Events

1. Investigated the current `invoke` / `tool_execute` path and found that bare duplicate upstream tool names returned `ambiguous_tool`, but the caller only got `valid` names and no retry hint.
2. Added selector support for fully-qualified `upstream::tool` names and an explicit `upstream` argument.
3. Updated MCP `invoke` schema and descriptions so agents can discover the new disambiguation options.
4. Added structured logging fields for failed resolution, including `requested_upstream`.
5. Added an MCP boundary test that runs `LabMcpServer` over an in-process RMCP transport and asserts the error envelope shape.
6. Verified formatting and focused tests.

## Key Findings

- `invoke` previously sent the requested name directly to the upstream pool; qualified names like `steamy-windows-mcp::PowerShell` would not route correctly without stripping the upstream selector before upstream execution.
- `ambiguous_tool` is part of the stable dispatcher error vocabulary, documented in `docs/dev/ERRORS.md`.
- Logs already existed around upstream dispatch, but resolver failure logs did not include the caller's requested upstream selector.
- Running the focused test without `--lib` compiled the `labby` binary test harness and rustc hit OOM; the lib test target passed and covered the new server test.

## Technical Decisions

- Kept duplicate names ambiguous by default instead of picking one by priority or insertion order; this avoids surprising execution on the wrong upstream.
- Supported both `name = "upstream::tool"` and `{ name = "tool", upstream = "upstream" }` because agents may either copy a qualified name from `valid` or use the schema field.
- Returned `valid` plus `hint` in the structured error envelope so agents do not need server logs to retry correctly.
- Used fake upstream catalog entries named `PowerShell` in tests because the original observed collision was between Windows MCP upstreams exposing that tool; the test does not execute a real shell or contact Windows.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `crates/lab/src/dispatch/gateway/manager.rs` | | Add qualified selector / explicit upstream resolution and resolver tests | `resolve_tool_execute_with_upstream` at `crates/lab/src/dispatch/gateway/manager.rs:2245`; tests at `crates/lab/src/dispatch/gateway/manager.rs:3443` |
| modified | `crates/lab/src/mcp/server.rs` | | Add `upstream` schema support, error hints/log fields, raw upstream tool forwarding, and MCP boundary test | schema at `crates/lab/src/mcp/server.rs:1114`; error envelope at `crates/lab/src/mcp/server.rs:1666`; test at `crates/lab/src/mcp/server.rs:3325` |
| modified | `docs/dev/ERRORS.md` | | Document `ambiguous_tool` hint behavior | `docs/dev/ERRORS.md:58` |
| created | `docs/sessions/2026-05-24-gateway-invoke-disambiguation.md` | | Save this session | this file |

## Beads Activity

No bead activity observed for this specific session. `bd list --all --sort updated --reverse --limit 100 --json` and `tail -200 .beads/interactions.jsonl` were checked; the visible recent interactions were older audit/cleanup closures from 2026-05-23 and did not correspond to this gateway invoke disambiguation work.

## Repository Maintenance

- Plans: checked `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/mcp-streamable-http-oauth-proxy.md`. Both remain active/open by their content, so no plan files were moved to `docs/plans/complete/`.
- Beads: read recent Beads state and interactions. No session-specific bead was identified, created, updated, or closed.
- Worktrees/branches: inspected `git worktree list --porcelain` and `git branch -vv`. Current worktree is `/home/jmagar/workspace/lab` on `fix/gateway-oauth-tool-gating`; no stale worktree or branch was removed.
- Stale docs: updated `docs/dev/ERRORS.md` because the implementation now returns an `ambiguous_tool` `hint` in addition to `valid`.
- Existing untracked file: `docs/sessions/2026-05-23-worktree-pr-cleanup.md` was already untracked before this save; it was left untouched.

## Tools and Skills Used

- Skill: `save-to-md`, used to capture this session with repo metadata and maintenance evidence.
- Shell commands: used for repo state, branch/worktree inspection, Beads reads, line-number evidence, formatting, and tests.
- File editing: used `apply_patch` for Rust/docs/session-note changes.
- MCP/app tools: none used for this session.
- Browser tools: none used for this session.
- Subagents/agents: none used for this session.

## Commands Executed

Critical commands and observed results:

```bash
cargo fmt --check
```

Passed after formatting.

```bash
cargo test -p labby --lib invoke_ambiguous_tool_error_envelope_guides_retry --all-features
```

Passed: 1 test passed.

```bash
cargo test -p labby --lib resolve_tool_execute_accepts_ --all-features
```

Passed: 2 tests passed.

```bash
cargo test -p labby --lib resolve_tool_execute_hides_priority_zero_upstreams --all-features
```

Passed: 1 test passed.

```bash
cargo test -p labby invoke_ambiguous_tool_error_envelope_guides_retry --all-features
```

Failed outside `--lib` because compiling the `labby` binary test harness hit `rustc-LLVM ERROR: out of memory`.

## Errors Encountered

- Initial MCP boundary test assertion looked for `kind` at the top level. The real envelope shape is `{ ok, service, action, error: { kind, message, ... } }`; the test was corrected to assert under `error`.
- `cargo fmt --check` initially reported line wrapping differences after adding the test; `cargo fmt` fixed them.
- Running the focused test without `--lib` hit rustc OOM while compiling the binary test target. Reran with `--lib`, which passed and covered the test added to `server.rs`.

## Behavior Changes (Before/After)

- Before: duplicate upstream tool names required a bare tool lookup that returned ambiguity, and the error lacked an explicit retry hint.
- After: agents can pass either `name = "upstream::tool"` or `upstream = "..."` with the raw tool name.
- Before: unknown-tool guidance referenced stale `tool_search`.
- After: unknown-tool guidance tells agents to call `scout`.
- Before: resolver failure logs did not show whether the caller attempted an explicit upstream.
- After: resolver failure logs include `requested_upstream`.

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo fmt --check` | formatted Rust code | passed | passed |
| `cargo test -p labby --lib invoke_ambiguous_tool_error_envelope_guides_retry --all-features` | MCP boundary envelope test passes | 1 passed | passed |
| `cargo test -p labby --lib resolve_tool_execute_accepts_ --all-features` | qualified and explicit upstream resolver tests pass | 2 passed | passed |
| `cargo test -p labby --lib resolve_tool_execute_hides_priority_zero_upstreams --all-features` | priority-zero upstream remains hidden | 1 passed | passed |
| `cargo test -p labby invoke_ambiguous_tool_error_envelope_guides_retry --all-features` | full focused test command passes | rustc OOM compiling bin test harness | failed, rerun with `--lib` |

## Risks and Rollback

- Risk: agents that pass a malformed qualified selector now receive `invalid_param` from selector parsing instead of falling through to unknown lookup.
- Risk: the new MCP boundary test adds helper fixtures in `server.rs`; keep them scoped to tests.
- Rollback: revert changes in `crates/lab/src/dispatch/gateway/manager.rs`, `crates/lab/src/mcp/server.rs`, and `docs/dev/ERRORS.md`.

## Decisions Not Taken

- Did not choose an arbitrary upstream when duplicate raw tool names exist; explicit selection is safer.
- Did not include the full `valid` vector in structured logs; the agent-facing envelope already carries it, and logs keep only `valid_count` plus selector fields.
- Did not create or close Beads for this small fix because no relevant active bead was observed in the session evidence.

## References

- `crates/lab/src/dispatch/gateway/manager.rs`
- `crates/lab/src/mcp/server.rs`
- `docs/dev/ERRORS.md`
- `docs/plans/fleet-ws-plan-lab-n07n.md`
- `docs/plans/mcp-streamable-http-oauth-proxy.md`

## Open Questions

- Whether to add a follow-up Bead for binary-test-target OOM during focused test runs.
- Whether the two active plan files should be reconciled with current gateway state in a separate planning cleanup pass.

## Next Steps

1. Run a broader `cargo test -p labby --lib --all-features` or CI equivalent when memory allows.
2. Decide whether to file a Bead for reducing `labby` binary test harness memory pressure.
3. Commit the gateway disambiguation changes and this session note when ready.
