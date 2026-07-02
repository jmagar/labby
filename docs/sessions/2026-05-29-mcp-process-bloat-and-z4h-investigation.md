---
date: 2026-05-29 21:14:40 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: f116216c
session id: 5723b17d-326a-4655-9f5f-34db90c2a66a
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/5723b17d-326a-4655-9f5f-34db90c2a66a.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab f116216c [main]
beads: none (no bead activity this session)
---

# MCP process bloat investigation and zsh4humans cleanup

## User Request

The session began as `/lavra-design` to "integrate rmcp-mux into the lab gateway's upstream MCP proxy" so stdio upstreams become shared, supervised, multiplexed processes. During brainstorming the user asked to **verify the gateway actually only spawns one stdio server** ("I'm pretty sure it's not working correctly"), which pivoted the entire session into a process-bloat investigation, a host cleanup, and a zsh4humans (z4h) teardown discussion.

## Session Overview

- Researched rmcp-mux (architecture, library API, config) via `axon ask`.
- Empirically disproved the premise that motivated the rmcp-mux task: the **gateway was never the duplication source**. The real cause was a **stale codex plugin cache** spawning bare `lab mcp` full-gateway clones, plus **leaked orphaned `codex-acp` process trees**.
- Cleaned up the host: reaped **93 orphan-tree processes** and **64 idle z4h tmux servers**, preserving the container gateway, all live `claude` sessions, and the live codex/copilot sessions.
- Diagnosed why z4h spawns one tmux server per interactive shell and applied a permanent fix to `~/.zshrc` (`zstyle ':z4h:' start-tmux no`).
- Began planning a migration off z4h (antidote / manual / oh-my-zsh); the user invoked `/save-to-md` before selecting a target.
- **Conclusion: rmcp-mux is not needed.** The problem was a stale-cache + process-leak, not a gateway/multiplexing gap. No repo code was changed.

## Sequence of Events

1. Ran `axon ask` four times to gather rmcp-mux architecture, library API (`MuxConfig`/`run_mux_server`/`spawn_mux_server`/`MuxHandle`/`check_health`), TOML config, and the problem it solves.
2. Read the gateway integration target: `crates/lab/src/dispatch/upstream/pool.rs` (`connect_stdio_upstream`, `TokioChildProcess`), `process_guard.rs`, `config.rs` (`UpstreamConfig`), confirming the pool already holds one connection per stdio upstream and fans out in-process.
3. Launched `lavra-brainstorm`; surfaced that gateway-internal sharing was already solved and external sharing was the real target. User clarified external front-doors connect over HTTP, collapsing the socket-exposure scope.
4. On user request, audited running processes: found ~10–11 `claude-in-mobile` servers, traced each leaf to its root.
5. Traced parents: 9–10 came from `lab mcp` stdio servers parented by Zed's `codex-acp`; 1 from `copilot`; 0 from `labby serve`.
6. User corrected: no stdio lab is configured in Zed; codex `[mcp_servers.labby]` uses `url = https://lab.example.com/mcp`. Tracked the bare `lab mcp` emitter to the **codex plugin cache**.
7. Confirmed root cause: the lab plugin migrated stdio→HTTP, but `~/.codex/plugins/cache/labby-marketplace/lab/local/.mcp.json` (dated 2026-05-08) still declares `command: lab, args: ["mcp"]`; all `codex-acp` were orphaned (PPID 1, 1–2.5 days old).
8. User uninstalled the plugin; verified running processes persisted (uninstall cannot reap orphans).
9. Performed guarded host cleanup (orphan trees, then idle tmux servers), after discovering and fixing a zsh word-splitting bug in the kill scripts.
10. Investigated z4h tmux sprawl and `gitstatusd`; applied `~/.zshrc` fix; killed two leftover MCP subtrees (codex's `lab mcp`, copilot's `claude-in-mobile`) on user request.
11. Started z4h migration planning; user invoked `/save-to-md`.

## Key Findings

- **Gateway dedup works.** `labby serve` (container `ebf1328f7fd5`) held exactly **1** `claude-in-mobile`. `UpstreamPool` (`crates/lab/src/dispatch/upstream/pool.rs:3676` `connect_stdio_upstream`) keeps one child per stdio upstream and multiplexes all gateway surfaces in-process; the single-client limit is already solved within the gateway.
- **Real duplication source = stale codex plugin cache.** `~/.codex/plugins/cache/labby-marketplace/lab/local/.mcp.json` and `~/.codex/plugins/cache/jmagar-lab/lab/0.5.3/.mcp.json` still use bare `lab mcp` (stdio), while the repo source `plugins/lab/.mcp.json` and the current Claude caches use `type: http, url: ${server_url}/mcp`. Each bare `lab mcp` loads the full `~/.labby/config.toml` and clones every stdio upstream.
- **Process leak.** All `codex-acp` processes were orphaned at PPID 1 (1–2.5 days old) from dead Zed sessions; their `lab mcp` → `claude-in-mobile` (+ shadcn, repomix, magic, github-chat, open-design, zsh-tool) descendants accumulated unbounded.
- **zsh word-splitting bug.** The Bash tool runs **zsh**, which does not word-split unquoted `$VAR`. Initial kill loops passed a single newline-joined blob to `kill` (`illegal pid`), so the first two cleanup attempts silently failed. Fixed with `while read -r` loops.
- **z4h tmux sprawl.** `~/.zshrc` never set `zstyle ':z4h:' start-tmux`, so z4h used its default `integrated` mode — re-exec every interactive shell inside its own private per-PID tmux server (`/tmp/z4h-tmux-1000-<pid>`). ~90 leaked over ~3 days.
- **gitstatusd is benign.** 37 processes, ~74 MB, **0 orphaned** — one per live p10k shell, reaped correctly on shell exit.
- **z4h is effectively unmaintained** (romkatv scaled back OSS). fzf 0.73.1 supports `fzf --zsh`, giving a clean replacement for z4h's `^R`/`^T`/`M-c` bindings; the only z4h feature with a real replacement cost is SSH config teleport (coverable by chezmoi, already installed).

## Technical Decisions

- **Do not build rmcp-mux.** The motivating premise (gateway spawns duplicate stdio children) was empirically false; the cure is refreshing the stale codex plugin cache + reaping orphans, not embedding a multiplexer.
- **Preserve, don't blanket-kill.** Cleanup explicitly protected: the container gateway, every tmux server with a live agent or attached client, the running shell's own ancestry chain, and the live codex/copilot sessions. Kill sets were filtered against a computed self-chain, a docker-cgroup check, and a live-agent regex.
- **`start-tmux no` over a shared session.** Chose to disable z4h's tmux integration entirely (vs. one shared `z4h` session) since agent shells don't benefit from it and it was the leak source; documented the shared-session alternative inline.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `~/.zshrc` (outside repo) | — | Added `zstyle ':z4h:' start-tmux no` before `z4h init` to stop per-shell tmux servers | Edit applied at the `# Initialize z4h` block (lines ~74–82) |
| created | `docs/sessions/2026-05-29-mcp-process-bloat-and-z4h-investigation.md` | — | This session log | this file |

No repository source files were modified. All other changes were host-environment state (process kills, tmux server kills) and the out-of-repo `~/.zshrc` edit.

## Beads Activity

No bead activity observed. The `/lavra-design` pipeline pivoted to investigation before any epic/child beads were created. `bd ready` was read for maintenance context only (top items: `lab-kvji`, `lab-vg7y3`, `lab-qmjlk`, `lab-hjhnu.3`, `lab-hjhnu.1`) — none were created, edited, or closed this session.

## Repository Maintenance

- **Plans:** `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/mcp-streamable-http-oauth-proxy.md` are unrelated to this session and not completed by it — left in place (no move).
- **Beads:** No bead state changed (see above). No follow-up beads created because the actionable items live outside this repo (codex plugin cache, copilot/codex session restarts, `~/.zshrc`).
- **Worktrees/branches:** `git worktree list` shows only `~/workspace/lab` on `main` at `f116216c`; `git status` clean and level with `origin/main`. No cleanup needed.
- **Stale docs:** No repo docs were contradicted by this session (the work was host-environment, not code). None updated.
- **Transparency:** The only repo write is this session artifact. The `~/.zshrc` change is outside the repo and is recorded here, not committed.

## Tools and Skills Used

- **Shell (Bash tool, zsh):** process auditing (`ps`, `/proc/<pid>/cgroup`, `pgrep -P`), guarded `kill` loops, config greps, `tmux -S … kill-server`. Issue: zsh non-word-splitting caused two failed kill attempts before switching to `while read` loops.
- **File tools:** `Read` (`~/.zshrc`), `Edit` (`~/.zshrc`), `Write` (this log).
- **Skills:** `lavra:lavra-design` → `lavra:lavra-brainstorm` (pipeline invoked, then superseded by live investigation); `save-to-md` (this artifact).
- **External CLI:** `axon ask` (4 queries on rmcp-mux) — all succeeded.
- **AskUserQuestion:** used several times; multiple calls were interrupted/rejected by the user when the working premise was still wrong, which is what drove the deeper root-cause tracing.
- No MCP tool calls, subagents, or browser tools were used.

## Commands Executed

| command | result |
|---|---|
| `axon ask "..."` ×4 | rmcp-mux architecture/API/config summaries |
| `ps -eo pid,ppid,etime,rss,cmd \| grep claude-in-mobile` | 10–11 leaf servers found |
| per-leaf parent-chain trace | 9–10 ← `lab mcp` ← `codex-acp` (Zed); 1 ← `copilot`; 0 ← `labby serve` |
| `cat ~/.codex/plugins/cache/labby-marketplace/lab/local/.mcp.json` | `command: lab, args: ["mcp"]` (stale, 2026-05-08) |
| orphan reap (`while read` + `kill -KILL`) | `killed=93 skipped=0` |
| tmux cleanup loop | `killed=64 kept=28` |
| leftover MCP subtree kills | host `lab mcp` 0, host `claude-in-mobile` 0; codex+copilot sessions preserved |
| `Edit ~/.zshrc` | added `zstyle ':z4h:' start-tmux no` |

## Errors Encountered

- **Silent kill failures (root cause: zsh word-splitting).** First two cleanup passes sent a newline-joined PID blob to `kill` → `illegal pid`, killing nothing. Resolved by rewriting kill logic with `while read -r pid` loops fed by newline-separated lists; subsequent pass reaped 93 processes.
- **False-positive process matches.** `ps | grep '[c]odex-acp'` matched the running `ps`/`grep` command's own argv (via the `syslog agent-command wrap` shell), inflating counts. Resolved by mapping each leaf to its cgroup/owner instead of trusting raw grep counts.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| host `claude-in-mobile` | 10 | 0 (container keeps 1) |
| host `lab mcp` stdio | 11 | 0 |
| orphaned `codex-acp` trees | 10 | 0 |
| idle z4h tmux servers | ~64 | 0 (41 live kept) |
| new interactive shells | spawn a private tmux server each | no tmux server (z4h `start-tmux no`) |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| post-reap `ps` for host `lab mcp` / `claude-in-mobile` | 0 / 0 | 0 / 0 | pass |
| container `claude-in-mobile` node leaves (cgroup `docker-ebf1328f`) | 1 | 1 | pass |
| live codex `2060762` + copilot `2562682` after subtree kills | alive | alive | pass |
| `git status` after session | clean on `main` | clean, level with `origin/main` | pass |

## Risks and Rollback

- **`~/.zshrc` change** affects only new interactive shells; running shells are unaffected. Rollback: remove the `zstyle ':z4h:' start-tmux no` line. Low risk (it is a documented z4h option).
- **Process kills** were one-way but scoped to leaked orphans and idle tmux servers; live work was preserved by explicit guards. No rollback applicable.

## Decisions Not Taken

- **Embed rmcp-mux** (the original task) — rejected; premise was false, cheaper root-cause fixes exist.
- **Expose mux Unix sockets / `rmcp_mux_proxy` shims for external front-doors** — rejected; front-doors use the HTTP gateway.
- **Kill the live codex/copilot sessions** — declined; only their leftover MCP child subtrees were reaped.
- **Blanket `tmux kill-server` on all sockets** — rejected; would have killed ~34 live `claude` sessions. Used a live-agent/attached-client guard instead.

## References

- rmcp-mux: github.com/vetcoders/rmcp-mux (v0.3, library-first)
- `crates/lab/src/dispatch/upstream/pool.rs` (`connect_stdio_upstream`), `process_guard.rs`, `CLAUDE.md`
- `~/.codex/plugins/cache/labby-marketplace/lab/local/.mcp.json` (stale stdio variant)
- `plugins/lab/.mcp.json` (current HTTP variant)

## Open Questions

- Should the lab marketplace's per-service plugins (`glances`, `arcane`, `unraid`, etc.) that ship `command: lab, args: ["mcp", "--services", ...]` also migrate to the HTTP gateway URL, or is stdio intended for them?
- Does codex need a cache-invalidation step so local-source marketplace plugins re-sync after the repo `.mcp.json` changes transport?

## Next Steps

- **Stop recurrence (host, outside repo):** refresh/reinstall the lab plugin in codex so its cache regenerates from the HTTP `.mcp.json`; purge stale stdio caches (`~/.codex/plugins/cache/.../lab/.../`, `~/.claude/.../lab/57805b…/`); restart the live codex CLI and copilot when convenient.
- **z4h migration (pending user choice):** pick antidote (recommended) / manual / oh-my-zsh; then write a new `~/.zshrc`, test in an isolated subshell with a backup, and swap only after verification. SSH teleport replacement via chezmoi.
- **Repo:** no follow-up code work from this session. rmcp-mux design is shelved as not-needed.
- **Immediate:** none blocking; this log is the only repo change.
