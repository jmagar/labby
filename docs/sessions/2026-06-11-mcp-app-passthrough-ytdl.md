---
date: 2026-06-11 10:05:34 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: d53ae992
session_id: 7e8cae3b-4275-4f88-80f0-f18559958db7
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/7e8cae3b-4275-4f88-80f0-f18559958db7.jsonl
working_directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
github_issue: 115
beads: none mutated
---

# MCP App passthrough and ytdl widget exposure

## User Request

Investigate GitHub issue 115 and finish implementing the MCP App passthrough work. The user then reported several live failures:

- spawn guard rejected `/home/jmagar/workspace/synapse2/plugins/synapse2/bin/synapse` even though `disable_spawn_guard = true`
- MCP Apps failed to load through the public gateway
- `ytdl-mcp` MCP App resources did not pass through Labby
- Labby's native Code Mode `search` and `execute` MCP UIs still needed to keep working
- UI-bearing upstream tools such as `youtube_search_ui` needed to be exposed as top-level raw MCP tools while ordinary upstream tools remained hidden

The final user request before this note was to stage, commit, and push the implementation directly to `main`.

## Outcome

Implemented and pushed the MCP App passthrough fixes to `main`:

- Commit: `d53ae992 Expose upstream MCP App tools through gateway`
- Branch: `main`
- Remote: `origin/main`

The pushed change makes upstream MCP App tools visible and callable as top-level tools when they advertise an MCP App UI resource, while preserving Code Mode hiding for ordinary upstream raw tools.

## Sequence of Events

1. **Spawn guard failure**
   - Confirmed `/home/jmagar/.labby/config.toml` already had:
     - `disable_spawn_guard = true`
     - `extra_stdio_commands = ["synapse", "ytdl-mcp", "claude", "axon"]`
   - Root cause was stale gateway runtime prefs.
   - Fixed by reloading the gateway with `labby gateway reload --json`.

2. **MCP App load failure**
   - Public MCP endpoint `mcp.example.com` failed with `Forbidden: Host header is not allowed`.
   - Added `mcp.example.com` and `lab.example.com` to `[mcp].allowed_hosts` in `/home/jmagar/.labby/config.toml`.
   - Restarted the `labby` container.
   - Verified public `resources/read ui://lab/code-mode/search` returned `text/html;profile=mcp-app`.

3. **Runtime binary drift**
   - Diffed the relevant MCP App files and confirmed the repo already had prior passthrough code.
   - Found the mounted runtime binary `/home/jmagar/workspace/lab/bin/labby` was older than the checkout.
   - Rebuilt and hot-swapped with `just dev-debug`.

4. **ytdl resource passthrough failure**
   - Direct `ytdl-mcp` stdio smoke showed:
     - tools: `youtube_download`, `youtube_probe`, `youtube_search`, `youtube_search_ui`
     - `youtube_search_ui` advertised `_meta.ui.resourceUri = ui://ytdl-mcp/youtube-search.html`
     - direct `resources/read` worked
   - Through Lab, `execute` returned the UI resource URI but `resources/read` failed with:
     - `unknown UI resource: ui://ytdl-mcp/youtube-search.html`
   - Root cause: Lab only resolved upstream UI resource ownership from cached `resources/list` data. If the host read the `ui://` resource before Lab had listed upstream resources, the owner was unknown.
   - Fix: fallback-route `ui://<authority>/...` to upstream `<authority>` and log the owner resolution path.

5. **Observability added**
   - Added structured logging around MCP App capture, opt-in, mirroring, owner lookup, and missing-owner failures.
   - This makes it visible when a tool result carries a UI resource URI, when it is mirrored into `_meta`, and how Lab resolves the owner for a passthrough resource read.

6. **Labby native UIs verified**
   - Verified the native Code Mode MCP App resources still work:
     - `ui://lab/code-mode/search`
     - `ui://lab/code-mode/execute`
   - Verified both are still advertised by `search` and `execute` in `tools/list`.

7. **Top-level UI tool exposure**
   - Confirmed the old behavior hid every raw upstream tool when Code Mode was enabled.
   - Implemented the intended exception:
     - UI-bearing upstream tools are promoted into `tools/list`.
     - Ordinary upstream raw tools remain hidden.
     - UI-bearing raw tool calls are allowed through the raw call gate.
   - Live proof:
     - `youtube_search_ui` listed: true
     - `youtube_probe` listed: false
     - `youtube_download` listed: false
     - `youtube_search_ui` raw call returned `_meta.ui.resourceUri`
     - passthrough `resources/read` returned `text/html;profile=mcp-app`

8. **Commit and push**
   - Created commit `95468ae5 Expose upstream MCP App tools through gateway`.
   - Initial push was rejected because `origin/main` advanced.
   - Rebased cleanly onto `origin/main`.
   - Pushed final commit as `d53ae992 Expose upstream MCP App tools through gateway`.

## Files Changed

| status | path | purpose |
|---|---|---|
| modified | `crates/lab/src/dispatch/gateway/code_mode/execute.rs` | Capture MCP App UI resource URIs from Code Mode execution and log capture/opt-in state |
| modified | `crates/lab/src/dispatch/upstream/pool/resources_read.rs` | Route `ui://<upstream>/...` reads by URI authority when cache lookup is absent; add passthrough test |
| modified | `crates/lab/src/dispatch/upstream/pool/tools.rs` | Detect and allow healthy UI-bearing upstream tools while raw tools are hidden |
| modified | `crates/lab/src/dispatch/upstream/pool.rs` | Re-export UI tool detection helper |
| modified | `crates/lab/src/mcp/call_tool.rs` | Permit raw calls to UI-bearing upstream tools when Code Mode hides ordinary raw tools |
| modified | `crates/lab/src/mcp/call_tool_codemode.rs` | Mirror MCP App UI metadata and log mirroring |
| modified | `crates/lab/src/mcp/handlers_tools.rs` | Promote UI-bearing upstream tools into top-level `tools/list` without exposing ordinary raw tools |
| modified | `crates/lab/src/mcp/handlers_tools/tests.rs` | Add regression coverage for UI-bearing upstream tool promotion |
| created | `docs/sessions/2026-06-11-mcp-app-passthrough-ytdl.md` | This session artifact |

## Verification Evidence

| command or check | result |
|---|---|
| `cargo fmt --all` | passed |
| `cargo test -p labby read_upstream_ui_resource --all-features` | passed |
| `cargo test -p labby list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden --all-features` | passed |
| `just dev-debug` | rebuilt `target/debug/labby`, installed `bin/labby`, restarted container |
| direct `ytdl-mcp` stdio smoke | `youtube_search_ui` advertised `ui://ytdl-mcp/youtube-search.html`; direct resource read worked |
| Lab streamable HTTP MCP smoke | native `search` and `execute` UIs listed and readable |
| Lab streamable HTTP MCP smoke | `youtube_search_ui` listed and callable; non-UI ytdl raw tools stayed hidden |
| Lab passthrough resource read | `ui://ytdl-mcp/youtube-search.html` returned `text/html;profile=mcp-app`, length about 404 KB |
| `git diff --check` | clean before code commit |
| `git push origin main` | pushed final commit `d53ae992` |

## Errors and Fixes

| symptom | cause | fix |
|---|---|---|
| spawn guard still rejected Synapse stdio path | stale gateway runtime prefs | `labby gateway reload --json` |
| public MCP App load failed | host guard rejected `mcp.example.com` | added allowed hosts and restarted Labby |
| rebuild seemed necessary despite source fix | mounted `bin/labby` was stale | `just dev-debug` |
| `ytdl-mcp` returned UI URI but resource read failed | resource owner lookup depended on a prior cached `resources/list` | fallback owner lookup from `ui://` URI authority |
| first live smoke parser failed on large HTML | script mishandled SSE framing | adjusted smoke parser |
| `gateway list --json` jq assumed `.upstreams[]` object shape | current CLI output is a top-level array | used direct MCP calls for proof |
| `youtube_search_ui` listed but raw call initially failed | call gate still required `LAB_CODE_MODE_WIDGET_CALLBACKS` | allowed UI-bearing tools through the raw call gate |
| first push rejected | `origin/main` advanced | fetched, rebased cleanly, pushed |

## Repository Maintenance

- **Git state**: after the implementation push, `/home/jmagar/workspace/lab` was clean on `main` at `d53ae992`, matching `origin/main`.
- **Worktrees**: inspected with `git worktree list --porcelain`; only the main worktree was present.
- **Branches/remotes**: inspected with `git branch -vv` and `git branch -r -vv`; `main` tracked `origin/main` at `d53ae992`.
- **Plans**: inspected plan locations. `docs/plans/complete/mcp-streamable-http-oauth-proxy.md` was already archived under `complete/`; `docs/plans/fleet-ws-plan-lab-n07n.md` looked unrelated/ambiguous and was left untouched.
- **Beads**: inspected bead state; no current-session bead mutation was made. The user framed this work around GitHub issue 115 rather than a bead.
- **Stale docs found**: `docs/dev/CODE_MODE.md` and `docs/services/GATEWAY.md` still describe raw upstream tools as hidden with no UI-tool exception. That is now stale after `d53ae992`. It was not edited in this save-to-md step because this commit is intentionally path-limited to the generated session artifact.

## Follow-ups

1. Update `docs/dev/CODE_MODE.md` to document that UI-bearing upstream MCP tools are promoted into `tools/list` and callable as top-level tools even when ordinary raw tools are hidden.
2. Update `docs/services/GATEWAY.md` to mention the same exception for upstream MCP App tools.
3. Consider adding a small follow-up bead for the doc refresh if this is not handled immediately after the save-session commit.

## References

- GitHub issue: `#115`
- Implementation commit: `d53ae992 Expose upstream MCP App tools through gateway`
- Transcript: `/home/jmagar/.claude/projects/-home-jmagar-workspace-lab/7e8cae3b-4275-4f88-80f0-f18559958db7.jsonl`
