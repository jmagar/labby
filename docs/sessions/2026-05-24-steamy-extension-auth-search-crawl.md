---
date: 2026-05-24 17:46:05 EST
repo: git@github.com:jmagar/axon.git
branch: main
head: a5d01683
working directory: /home/jmagar/workspace/axon_rust
worktree: /home/jmagar/workspace/axon_rust
---

# Steamy WSL Extension Auth And Search Crawl Verification

## User Request

Use `steamy-wsl` and systematically debug the Chrome extension / crawl-with-search behavior. Follow up by configuring the bearer token and testing the real authenticated path, not just DOM loading.

## Session Overview

- Repaired the `steamy-wsl` SSH session's missing Windows interop registration so Windows `.exe` commands could run from WSL over SSH.
- Confirmed branded Chrome 148 ignores unpacked extension command-line loading, matching Chromium's Chrome 137+ behavior.
- Switched to WSL Chromium, loaded the Axon extension, configured the bearer token in extension storage, and tested authenticated extension calls against the live Axon API.
- Found `/v1/search` initially returned partial crawl enqueue because SQLite job queue writes were failing with `code: 522 disk I/O error`.
- Restarted the Axon container, then verified search returned two results and queued two crawl jobs with zero rejected crawls.

## Sequence of Events

1. SSH to `steamy-wsl` worked, but `/mnt/c/Windows/System32/*.exe` failed with `exec format error`.
2. Checked `binfmt_misc`, found `WSLInterop` missing, and registered it for the current session with `/init`.
3. Launched a separate Windows Chrome debug profile, discovered port `9222` was already owned by `C:\chrome-debug`, and moved isolated testing to a new port.
4. Confirmed branded Chrome 148 did not load the unpacked Axon extension via `--load-extension`.
5. Launched WSL Chromium headless with `--load-extension`, verified the Axon service worker and extension UI pages loaded.
6. Configured `chrome.storage.local` with the Axon URL, bearer token, and `autoScrapeEnabled: true`.
7. Ran authenticated `checkApi`, `scrapeWithAxon`, `searchWithAxon`, and the extension command path.
8. Investigated failed crawl enqueue, checked logs, DB integrity, disk space, file ownership, and process ownership.
9. Restarted `axon`, then reran authenticated server and extension search tests successfully.

## Key Findings

- `steamy-wsl` SSH was not the blocker. The blocker was missing `WSLInterop` under `/proc/sys/fs/binfmt_misc`, which prevented Windows executables from launching through the SSH session.
- Branded Chrome 148 did not load the unpacked extension using `--load-extension`; Chrome APIs showed no loaded extensions, and extension page targets resolved to `chrome-error://chromewebdata/`.
- WSL Chromium successfully loaded the extension as `chrome-extension://ejkokbgfbfkjjdfdcglplnflmckepkje/background.js`.
- Authenticated extension API check succeeded after setting `axonUrl` and `axonToken` in `chrome.storage.local`.
- The real crawl-with-search failure was runtime queue state: Axon logs showed SQLite `code: 522 disk I/O error` on crawl, extract, embed, and ingest job claims/enqueues.
- `~/.axon/jobs.db` passed `PRAGMA quick_check` and `PRAGMA integrity_check`; disk space and file ownership were not the immediate problem.
- Restarting the Axon container cleared the runtime queue failure and restored all-search-result crawl enqueue.

## Technical Decisions

- Used WSL Chromium for extension testing because Chrome's official guidance says `--load-extension` continues to work in Chromium / Chrome for Testing, not branded Chrome 137+.
- Used a fresh Chromium profile and a separate debug port to avoid changing the user's personal Chrome state.
- Configured the extension through `chrome.storage.local` over CDP so the test exercised the same extension API code path.
- Restarted only the `axon` container after confirming DB integrity and no second local writer, preserving Qdrant, TEI, and Chrome sibling services.

## Files Changed

| status | path | previous path | purpose | evidence |
| --- | --- | --- | --- | --- |
| created | `docs/sessions/2026-05-24-steamy-extension-auth-search-crawl.md` |  | Session documentation | This file |

Pre-existing dirty files observed and left untouched:

- `docs/sessions/2026-05-24-claude-plugin-monitor-live-test.md`
- `docs/sessions/2026-05-24-pr136-palette-monitor-merge.md`

## Beads Activity

No bead activity was performed during this save-to-md pass.

Observed tracker context:

- `bd list --all --sort updated --reverse --limit 50 --json` returned older closed review/performance issues and current `axon_rust-b0u9*` activity from prior work.
- `.beads/interactions.jsonl` tail showed previous `axon_rust-b0u9*` status transitions on 2026-05-24, but no new bead was created, edited, or closed for this debugging pass.

## Repository Maintenance

- Plans: reviewed `docs/plans/` and `docs/plans/complete/`. No plan was moved because the active open plans listed there were not clearly completed by this session.
- Beads: read recent Beads state and interactions; no tracker mutation was made because this session was operational verification/debugging, not a planned bead closeout.
- Worktrees and branches: inspected `git worktree list --porcelain`, local branches, and remote branches. No cleanup was performed because active registered worktrees exist for `work/async-prepared-session-ingest`, `feat/axon-status-trim`, and `feat/rest-api-canonical-contracts`.
- Stale docs: no source or workflow docs were updated. The only durable update from this pass is this session note.
- Dirty state: left the two pre-existing session docs untouched and added this session doc.

## Tools And Skills Used

- Skill: `save-to-md` for session capture format and maintenance checklist.
- Skill: `superpowers:systematic-debugging` earlier in the debugging flow to isolate symptoms, gather evidence, and avoid guessing.
- Skill: `chrome` earlier in the debugging flow to drive Chrome/Chromium via CDP.
- Shell / SSH: used `ssh steamy-wsl`, PowerShell through WSL interop, Docker CLI, `curl`, `sqlite3`, `lsof`, `df`, `git`, `gh`, and `bd`.
- Browser tooling: Chrome DevTools Protocol through Windows PowerShell helpers and direct Node WebSocket CDP calls from `steamy-wsl`.
- External docs: Chromium Extensions announcement about `--load-extension` removal in branded Chrome builds.

## Commands Executed

Critical commands and observed results:

```bash
ssh steamy-wsl 'test -e /proc/sys/fs/binfmt_misc/WSLInterop && echo yes || echo no'
# Initially no; after registration, yes.
```

```bash
printf ':WSLInterop:M::MZ::/init:PF\n' | sudo tee /proc/sys/fs/binfmt_misc/register >/dev/null
# Registered Windows interop for the current WSL session.
```

```bash
ssh steamy-wsl 'chromium-browser --headless=new --remote-debugging-port=9336 ... --load-extension=/mnt/c/Users/jmaga/Desktop/axon-extension-test'
# WSL Chromium loaded the Axon extension service worker.
```

```bash
curl -H "Authorization: Bearer <redacted>" http://127.0.0.1:8001/healthz
# 200 ok
```

```bash
curl -H "Authorization: Bearer <redacted>" -H 'Content-Type: application/json' \
  -d '{"query":"rust async traits","limit":2}' \
  http://127.0.0.1:8001/v1/search
# Before restart: 1 crawl queued, 1 crawl rejected with SQLite code 522.
# After restart: 2 crawls queued, 0 rejected.
```

```bash
sqlite3 ~/.axon/jobs.db 'PRAGMA quick_check; PRAGMA integrity_check;'
# ok
# ok
```

```bash
docker restart axon
# axon returned healthy; follow-up search queued all crawl jobs.
```

## Errors Encountered

- Windows interop missing in SSH session: `cmd.exe` / `powershell.exe` returned `exec format error`. Resolved for the session by registering `WSLInterop`.
- Port conflict: Windows Chrome debug port `9222` was already owned by an existing `C:\chrome-debug` profile. Avoided by using separate ports for isolated testing.
- Branded Chrome extension load failure: Chrome 148 ignored command-line unpacked extension loading. Resolved by testing with WSL Chromium instead.
- Auth was initially not configured in the extension. Resolved by setting `axonUrl`, `axonToken`, and `autoScrapeEnabled` in `chrome.storage.local`.
- Axon queue failure: `/v1/search` returned partial auto-crawl status because job enqueues failed with SQLite `code: 522 disk I/O error`. DB integrity and disk checks passed; restarting the Axon container cleared the runtime failure.

## Behavior Changes

Before:

- Extension shell could be loaded in WSL Chromium, but authenticated behavior had not been tested.
- `/v1/search` returned results but did not enqueue a crawl for every search result because one enqueue failed with SQLite disk I/O.

After:

- Authenticated extension API check reports online and token accepted.
- Extension scrape path returns `live_scrape` for `https://example.com/`.
- Extension search command for `rust async traits --limit 2` reports `2 results` and `2 crawls queued`.

## Verification Evidence

| command | expected | actual | status |
| --- | --- | --- | --- |
| `curl /healthz` with bearer | Authenticated health check succeeds | `200`, body `ok` | pass |
| `curl /v1/scrape` with bearer and `https://example.com/` | Live scrape succeeds | `200`, backend `live_scrape` | pass |
| Extension `checkApi()` after storage token config | API online and token accepted | `apiStatus: Online`, status text `Axon API reachable and token accepted.` | pass |
| Extension `scrapeWithAxon("https://example.com/")` | Scrape returns page markdown | Prefix `Example Domain` | pass |
| Extension `searchWithAxon("rust async traits", { limit: 2 })` before restart | Two crawls expected | One queued, one rejected with SQLite `code: 522 disk I/O error` | fail, root cause found |
| `sqlite3 ~/.axon/jobs.db 'PRAGMA quick_check; PRAGMA integrity_check;'` | DB integrity OK | `ok`, `ok` | pass |
| `docker restart axon` then server `/v1/search` | Two results and two crawl jobs | `auto_crawl_status: queued`, `crawl_job_count: 2`, `rejected_count: 0` | pass |
| Extension search command after restart | UI command path reports crawls queued | `[[success:2 results]] [[info:2 crawls queued]]` | pass |

## Risks And Rollback

- The WSLInterop registration was a session repair and may not persist after a WSL restart. Persistent repair should be handled separately if the problem recurs.
- Restarting `axon` interrupted the running API process briefly. Rollback is another `docker restart axon` or full compose restart from `~/.axon/compose` if needed.
- Temporary Chromium profiles and processes were cleaned up after tests.

## Decisions Not Taken

- Did not manually install the unpacked extension in branded Chrome because the goal was automated smoke testing and Chrome 148 no longer honors the command-line load path.
- Did not mutate Beads or move plan files because this session did not clearly complete any tracked plan or bead.
- Did not replace `jobs.db` because integrity checks passed and restart resolved the immediate runtime queue failure.

## References

- Chromium Extensions announcement: https://groups.google.com/a/chromium.org/g/chromium-extensions/c/1-g8EFx2BBY
- Chrome extension files tested: `apps/chrome-extension/manifest.json`, `apps/chrome-extension/background.js`, `apps/chrome-extension/popup-api.js`, `apps/chrome-extension/popup-actions.js`.
- Runtime DB: `~/.axon/jobs.db`.
- Docker service: `axon`.

## Open Questions

- Whether the SQLite `code: 522 disk I/O error` was caused by stale WAL/runtime state, a transient filesystem issue, or a specific concurrency path in job queue handling.
- Whether `WSLInterop` should be registered persistently on `steamy-wsl`.

## Next Steps

- Add or update a follow-up issue for investigating recurring SQLite job queue `code: 522 disk I/O error` if it appears again in logs.
- Consider adding a health check or watchdog metric that detects queue read/write failures instead of only reporting process health.
- If `steamy-wsl` loses Windows interop after restart, add a durable WSL/systemd repair instead of relying on per-session registration.
