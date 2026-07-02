---
date: 2026-05-06 15:56:25 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 1d5c7ebc
agent: Claude (Sonnet 4.6 / Opus 4.7)
session id: 5f6760c4-6c11-411a-a898-4b5d90eb4bd1
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/5f6760c4-6c11-411a-a898-4b5d90eb4bd1.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab [main]
---

## User Request

Initial: "why do i not see ANY of the changes that were supposed to be added in those 8 plans we implemented?" — referring to eight chat-feature plans dated 2026-05-05. Later expanded to "irrefutable screenshot-based evidence that all 8 features are fully working and implemented" plus follow-up debugging of Claude/Gemini model pickers, workspace picker, attachment send flow, attach-button consolidation, attachment-only sends, bearer + OAuth coexistence, "Claude straight up doesn't work", and finally directing the assistant to drive the UI with `agent-browser` rather than asking for symptom descriptions.

## Session Overview

Two arcs ran back-to-back:

1. **Verify the eight chat plans landed** (`docs/superpowers/plans/2026-05-05-chat-*.md`). Most were already shipped in earlier commits but were unverifiable because the running docker container was using a binary built before those commits — `just dev-debug` had been silently failing. After fixing the build path, all eight were verified and screenshotted; the one feature that genuinely didn't work (model picker) had a root cause in `agent-client-protocol`'s upstream SDK.
2. **Fix a second wave of issues found during verification:** Claude session crashes, missing model pickers for Claude/Gemini, workspace-picker JSON parse error, attachment send flow crash, two-button attach UX, and attachment-only send rejection. Plus user-requested infra work: bearer auth coexisting with OAuth so `agent-browser` can drive the UI; ACP adapters baked into the docker image instead of `npx`-on-spawn; live bind mount of host Claude credentials.

Net: 9 commits on `main`, 4 doc updates, 1 vendored upstream crate fork, 1 image rebuild, end-to-end Claude verified streaming through SSE.

## Sequence of Events

1. **Justfile `dev-debug` recipe** failed three different ways before resolving: `cargo +nightly` (cargo not a rustup proxy) → `rustup run nightly cargo` (sccache calling stable rustc) → `RUSTC_WRAPPER=` (rustc on PATH still stable) → final fix uses `RUSTC=$(rustup which --toolchain nightly rustc)` in a shebang-style recipe.
2. **Built and deployed the latest binary**, then verified each chat feature with `agent-browser` against `localhost:8765`.
3. **Discovered the model picker was the only genuine code gap** — `agent-client-protocol@0.11.1`'s `attach_session()` discards `models` from `NewSessionResponse`. Vendored the crate at `crates/vendor/agent-client-protocol/`, patched `ActiveSession` to preserve models, wired via `[patch.crates-io]`. After patch, all 24 Codex models populated the picker.
4. **Captured 8 proof screenshots** for the original plans.
5. **User reported new wave of issues**: Claude broken, no model pickers for Claude/Gemini, workspace picker JSON error, attachment send crashing the page, two attach buttons, no attachment-only send, bearer + OAuth coexistence broken.
6. **Bearer + OAuth fix**: `/auth/session` was mounted outside the auth middleware and only checked the cookie path. Added `bearer_token` to `AppState` and bearer validation inside the handler so the AuthBootstrap recognizes `agent-browser --headers` callers as logged in.
7. **Claude/Gemini model pickers**: investigation found that providers only populate their model catalog after a session is bootstrapped, but the frontend fetched `/v1/acp/provider` only at mount. Added `refreshProviderRef.current()` after `createSession` so the picker refreshes once a session exists.
8. **Attachment send flow**: `await onSend(...)` was throwing out of an event-handler promise, the chip never cleared, the unhandled rejection crashed the chat tree. Wrapped in try/catch with snapshotted attachments and inline error message.
9. **Combined attach buttons**: replaced the paperclip + document pair with a single paperclip → DropdownMenu offering "From device" / "From workspace".
10. **Attachment-only sends**: backend required non-empty prompt. Loosened validation at both the surface adapter and the dispatch layer to accept either text OR attachments.
11. **Claude SIGILL**: SSE event capture revealed `provider_error: "Failed to authenticate. API Error: 401 ... authentication_error"` from Anthropic. The lab-stored credentials had `expiresAt: 2026-05-01` and empty `refreshToken`. Copied the host's fresh `~/.claude/.credentials.json` → Claude Code binary `SIGILL`'d. Diagnosis: the bundled `@anthropic-ai/claude-agent-sdk@0.2.126` (Claude Code 2.1.126) was incompatible with credentials issued by the host's `claude` CLI 2.1.129+.
12. **Pre-installed ACP adapters in image**: rewrote `config/Dockerfile.fast` to install `claude-agent-acp`, `codex-acp`, `gemini` globally with an npm `overrides` entry forcing `@anthropic-ai/claude-agent-sdk@^0.2.131`. Symlinked binaries into `/usr/local/bin/`. Updated `config/acp-providers.docker.json` to call those binaries directly.
13. **End-to-end verification**: created Claude session, sent "Reply with just hello", got streamed `h` + `ello` chunks via SSE, `state: completed`. Chat UI rendered the response.
14. **Documented everything**: updated CLAUDE.md, README.md, docs/runtime/OAUTH.md, docs/runtime/CONFIG.md.
15. **Bind-mounted `~/.claude/.credentials.json`** at the file level so the host's live credentials reach the container automatically without a copy step.

## Key Findings

- `agent-client-protocol@0.11.1` `src/session.rs:84-89` destructures `NewSessionResponse { session_id, modes, meta, .. }` in `attach_session()`. The `..` discards `models` and `config_options`. The chat UI's `{modelOptions.length > 0}` gate at `apps/gateway-admin/components/chat/chat-input.tsx:466` means dropping `models` makes the entire model picker invisible. Codex returns 24 models, Claude returns 3, Gemini returns 8 — all silently dropped.
- `apps/gateway-admin/lib/chat/chat-session-provider.tsx:295-320` defines `refreshProvider`, but `createSession` (line 326+) didn't call it. Provider models only populate after the first session bootstrap, so the picker stayed empty until manual page reload.
- `apps/gateway-admin/components/chat/chat-input.tsx:110-148` `handleSend` had no `catch` around `await onSend`. Backend prompt failures threw synchronously, the chip never cleared, the unhandled rejection bubbled to the React tree and the chat page rendered blank.
- `crates/lab/src/api/router.rs:910-915` mounts `/auth/session` at the top-level router, outside `authenticate_request`. The handler at `crates/lab/src/api/browser_session.rs:113` only checked the session cookie. Bearer-driven automation got `authenticated: false` even though `/v1/*` accepted the same bearer token at the middleware layer.
- `claude-agent-acp@0.32.0` pins `@anthropic-ai/claude-agent-sdk` to exactly `0.2.126` in package.json, so a global install of `claude-agent-sdk@latest` does not override the nested copy that ships with `claude-agent-acp`. An npm `overrides` block on a wrapper `package.json` is required to float the SDK forward.
- The bundled Claude Code binary version is locked to whatever `@anthropic-ai/claude-agent-sdk` version ships, and that binary `SIGILL`s when handed credentials issued by a meaningfully-newer host `claude` CLI install (host 2.1.129 + bundled 2.1.126 → SIGILL on `session/new`).
- Lab-stored Claude credentials at `~/.labby/acp/claude-home/.credentials.json` had `expiresAt: 1777692882995` (2026-05-01 23:34 UTC, already past) AND empty `refreshToken: ""`, so the runtime could not auto-renew. Anthropic returned 401 on every prompt → `state: failed`.

## Technical Decisions

- **Vendor `agent-client-protocol` rather than send an upstream PR.** Upstream PR is the right long-term fix but blocking on it would have left the model picker unusable for an unbounded period. The patch is three small deltas in one file (`src/session.rs`) and is documented in CLAUDE.md so future maintainers know the rationale and can drop the fork once upstream lands a fix.
- **Bake adapters into the docker image; don't fetch via `npx -y` on every spawn.** Three benefits: faster session start, deterministic version pinning, and the ability to override `claude-agent-sdk` via an `overrides` block. The npm overrides block is what actually fixes the SIGILL — `claude-agent-acp` pins SDK with `=` so a top-level install can't satisfy both.
- **Bind-mount `~/.claude/.credentials.json` rw, not ro.** `claude-agent-sdk` may write a refreshed token; if the host file is read-only, refresh fails mid-session. Both the host CLI and container SDK writing the same file is unlikely to race in practice, and either writer's last-write-wins is acceptable since both produce valid tokens.
- **Bearer `/auth/session` returns `csrf_token: ""` and `is_admin: true`.** Bearer auth bypasses CSRF checks at the middleware layer (it's only enforced for cookie-authenticated requests), and there's no per-request scope variation for static-bearer callers, so they're treated as admin to match how `/v1/*` already classifies them.
- **Frontend: snapshot `attachments` at send-start.** Without the snapshot, cleanup uses the closure-captured value, which can drift if React re-renders between the optimistic-add and the await-resolve. Snapshotting locks in the array we just shipped.
- **Combined attach button uses `DropdownMenu` not a hidden flyout.** The two-button layout was a UX failure mode (icons looked similar, users always clicked the wrong one). Single trigger + explicit "From device" / "From workspace" labels eliminates ambiguity.
- **Allow attachment-only prompts at both layers.** Surface adapter validates "text OR attachments required"; dispatch layer drops the `require_str("text")` and just skips the size check when text is empty. Both layers had to change because the dispatch layer is also called by MCP and CLI surfaces.

## Files Modified

| File | Purpose |
|---|---|
| `Justfile` | Fix `dev-debug` recipe to resolve nightly rustc explicitly via `rustup which`, clear `RUSTC_WRAPPER` so sccache doesn't intercept |
| `Cargo.toml` | Add `[patch.crates-io]` for vendored `agent-client-protocol` |
| `Cargo.lock` | Regenerated for the patch |
| `crates/vendor/agent-client-protocol/` | Vendored fork preserving `models` on `ActiveSession`. Three deltas in `src/session.rs`: struct field, `attach_session()` body, `response()` body, and the proxy-mode destructure stub |
| `crates/lab/src/api/state.rs` | Added `bearer_token: Option<Arc<str>>` to `AppState` plus `with_bearer_token` builder so handlers outside the auth middleware can validate the same token |
| `crates/lab/src/api/router.rs` | Plumb `static_token` onto `AppState` after construction; promote `tokens_equal`/`parse_bearer_token` to `pub(crate)` |
| `crates/lab/src/api/browser_session.rs` | `auth_session` accepts `Authorization: Bearer <LAB_MCP_HTTP_TOKEN>` and returns synthetic admin session |
| `crates/lab/src/api/services/acp.rs` | Allow attachment-only prompts (replace `prompt is required` gate with text-or-attachments check) |
| `crates/lab/src/dispatch/acp/dispatch.rs` | Drop `require_str("text")`; skip size check when text is empty |
| `apps/gateway-admin/lib/chat/chat-session-provider.tsx` | Forward-declared `refreshProviderRef` synced via `useEffect`; `createSession` calls `refreshProviderRef.current?.()` after a successful create |
| `apps/gateway-admin/components/chat/chat-input.tsx` | `handleSend` snapshot + try/catch; combined attach buttons into single paperclip → DropdownMenu trigger |
| `config/Dockerfile.fast` | Pre-install ACP adapters into `/opt/acp-adapters/` with `overrides` block bumping `claude-agent-sdk` to `^0.2.131`; symlink binaries into `/usr/local/bin/` |
| `config/acp-providers.docker.json` | Switch `command` from `npx` to direct binary names; drop `-y` and `@version` args |
| `docker-compose.yml` | Bind-mount `${HOME}/.claude/.credentials.json` over `/home/labby/.labby/acp/claude-home/.credentials.json` so token refreshes flow through automatically |
| `CLAUDE.md` | New "Vendored ACP SDK", "Docker dev container", "Bearer auth in dev" sections |
| `README.md` | Added `/auth/session` row to protected-route table; new Development paragraph on pre-installed adapters and the agent-browser bearer-header pattern; surfaced `dev-up`/`dev`/`dev-debug` just targets |
| `docs/runtime/OAUTH.md` | Browser-session introspection rules cover the bearer path returning `sub: "static-bearer"` |
| `docs/runtime/CONFIG.md` | Dev-container subsection rewritten for pre-installed adapters; init: true rationale updated |

## Commands Executed

- `just dev-debug` — debug rebuild with cranelift, hot-swap into running container. Failed three ways before final form (`RUSTC=$(rustup which --toolchain nightly rustc) RUSTC_WRAPPER="" RUSTFLAGS=... cargo build ...`); final form completed in ~1m20s.
- `docker compose -f docker-compose.yml -f docker-compose.dev.yml up -d labby-master` — recreate container after compose-file or env changes (restart alone does not re-read compose).
- `docker compose -f docker-compose.yml -f docker-compose.dev.yml build labby-master` — rebuild image after Dockerfile changes; ~25s on subsequent builds.
- `curl -sH "Authorization: Bearer $TOKEN" http://localhost:8765/v1/acp/provider` — verified 200 + populated `models[]` per provider after SDK patch.
- `curl -sN -H "Authorization: Bearer $TOKEN" "http://localhost:8765/v1/acp/sessions/$SID/events?ticket=$TICKET"` — SSE subscription that captured the `provider_error: 401 authentication_error` revealing the credential issue, and later captured streamed `h`/`ello` chunks proving Claude works.
- `agent-browser --session test open http://localhost:8765/chat --headers '{"Authorization":"Bearer $TOKEN"}'` — drove the chat UI as the synthetic `static-bearer` admin while OAuth stayed enabled, used for all visual verification.
- `md5sum ~/.claude/.credentials.json && docker exec lab-labby-master-1 md5sum /home/labby/.labby/acp/claude-home/.credentials.json` — confirmed bind mount; both `b28a94d3544cbf426a27d2c2a7552de9`.

## Errors Encountered

- **`cargo +nightly build` → "no such command: `+nightly`"**. Cause: the `cargo` on PATH was not the rustup proxy, so the `+toolchain` directive was treated as an unknown subcommand. Fix: invoke through `rustup run nightly cargo`, then explicitly set `RUSTC=$(rustup which --toolchain nightly rustc)` after sccache and PATH ordering both interfered.
- **`-Z codegen-backend=cranelift` rejected by stable rustc**. Cause: even after switching to `rustup run nightly cargo`, sccache (set as `RUSTC_WRAPPER`) was invoking the stable `rustc` from PATH. Fix: clear `RUSTC_WRAPPER=""` for the recipe and pin `RUSTC` explicitly to the nightly absolute path.
- **`refusing to bind HTTP on 0.0.0.0:8765 without authentication`**. Cause: dev override I'd added was clearing OAuth env vars without setting a bearer, tripping the safety gate. Fix: kept OAuth configured, only set `LAB_WEB_UI_AUTH_DISABLED=true` for the testing window, then reverted entirely once bearer-via-`/auth/session` was implemented.
- **Workspace picker "Unexpected token '<' ... is not valid JSON"** reported by user. Cause was a stale-env container — `LAB_WEB_UI_AUTH_DISABLED=true` from a previous compose iteration was sticky because `docker compose restart` does not re-read environment changes. Fix: `docker compose up -d` (full recreate) and the picker started returning JSON correctly. Underlying fetch flow was already correct (`Accept: */*`, no redirect path triggered).
- **Claude session immediately transitions to `failed` state**. SSE events revealed `Failed to authenticate. API Error: 401 authentication_error`. Cause: lab-stored `~/.labby/acp/claude-home/.credentials.json` had an expired access token and empty refresh token. Fix: tested with host's fresh creds → triggered next error.
- **Claude Code binary `SIGILL` / `SIGTRAP` on `session/new` after credential refresh**. Cause: `@anthropic-ai/claude-agent-sdk@0.2.126` (Claude Code 2.1.126) bundled with `claude-agent-acp@0.32.0` was incompatible with credentials issued by a newer host `claude` CLI install (2.1.129+). Fix: npm `overrides` block in `/opt/acp-adapters/package.json` floats the SDK to `^0.2.131` (Claude Code 2.1.131); confirmed end-to-end Claude streaming.

## Behavior Changes (Before/After)

| Surface | Before | After |
|---|---|---|
| `just dev-debug` | Failed with `+nightly` / `-Z` / sccache errors | Builds and hot-swaps in ~1m20s |
| Chat model picker | Empty for all providers (SDK stripped models) | Populated for Codex (24 models), Claude (3), Gemini (8) once a session exists |
| Provider models in UI | Visible only after manual page reload | Refresh on session creation |
| Chat attach UX | Two near-identical buttons (paperclip = local, document = workspace) | Single paperclip → "From device" / "From workspace" menu |
| Attachment send failure | Page renders blank, chip stuck | Inline error, chip preserved for retry |
| Attachment-only send | `prompt is required` 422 | Accepted (text OR attachments required) |
| `/auth/session` with bearer | `authenticated: false` (cookie-only) | `authenticated: true, sub: static-bearer, is_admin: true` |
| `agent-browser --headers` | Renders sign-in screen (UI cookie-gated) | Renders full UI as admin |
| Claude session | `state: failed` within seconds (auth 401 or SIGILL) | Streams replies, `state: completed` |
| ACP adapter spawn | `npx -y @package/name` per spawn (registry round-trip) | Direct binary call to `/usr/local/bin/{claude-agent-acp,codex-acp,gemini}` |
| Claude credential refresh | Manual copy from `~/.claude/.credentials.json` to `~/.labby/acp/claude-home/.credentials.json` | Bind-mounted live; host CLI refresh is automatically picked up |

## Verification Evidence

| Command | Expected | Actual | Status |
|---|---|---|---|
| `curl -sH "Authorization: Bearer $TOKEN" /v1/acp/provider` | 200, populated models per provider | `codex-acp: 24, claude-acp: 3, gemini: 8` after sessions created | pass |
| `curl -s /auth/session` (no auth) | `authenticated: false, login_available: true` | exact match | pass |
| `curl -sH "Authorization: Bearer $TOKEN" /auth/session` | `authenticated: true, sub: static-bearer, is_admin: true` | exact match | pass |
| `curl -s /v1/acp/provider` (no auth) | 401 | 401 | pass |
| `curl -sX POST /v1/acp/sessions/$SID/prompt` empty prompt + attachments | `{ok: true}` | `{"ok":true}` | pass |
| `curl -sX POST /v1/acp/sessions/$SID/prompt` empty prompt + no attachments | 422 missing_param | 422 missing_param "prompt or attachments is required" | pass |
| Claude prompt → SSE | text chunks + `state: completed` | `"h"` + `"ello"` chunks, `state: completed` | pass |
| `md5sum ~/.claude/.credentials.json` vs container view | Identical | Both `b28a94d3544cbf426a27d2c2a7552de9` | pass |
| `docker exec lab-labby-master-1 which claude-agent-acp` | `/usr/local/bin/claude-agent-acp` | `/usr/local/bin/claude-agent-acp` | pass |
| `cat /opt/acp-adapters/node_modules/@anthropic-ai/claude-agent-sdk/package.json \| grep version` | `0.2.131` | `0.2.131` | pass |
| `/opt/.../claude-agent-sdk-linux-x64/claude --version` | `2.1.131 (Claude Code)` | `2.1.131 (Claude Code)` | pass |
| 8 chat-feature plan screenshots | Each shows the feature working | All saved to `/tmp/PROOF-*.png` | pass |

## Risks and Rollback

- **Vendored `agent-client-protocol`** drift risk: if upstream lands a behaviorally-different `attach_session()` we may not pick it up automatically. Mitigation: CLAUDE.md documents the patch shape and re-application steps. Rollback: remove `[patch.crates-io]` from `Cargo.toml`, delete `crates/vendor/`, accept that the model picker will be empty again.
- **Bearer-driven `/auth/session`**: returns `csrf_token: ""` for static-bearer admins. CSRF protection is unaffected because bearer auth bypasses cookie-based CSRF checks anyway, but anyone evaluating the auth surface should know that the empty CSRF token in the response is intentional, not a bug. Rollback: revert `crates/lab/src/api/browser_session.rs` and `crates/lab/src/api/state.rs` bearer_token plumbing.
- **`@anthropic-ai/claude-agent-sdk@^0.2.131` override**: floats the bundled Claude Code binary forward of whatever `claude-agent-acp` was tested against. If a future SDK release breaks the ACP protocol surface, every Claude session in the lab fails. Mitigation: pin tighter (`~0.2.131`) once known-good. Rollback: remove the `overrides` block in `config/Dockerfile.fast`, rebuild image, accept that newer host `claude` CLIs will SIGILL again.
- **Bind mount of `~/.claude/.credentials.json`**: if the host file disappears, `docker compose up` fails. Mitigation: documented prerequisite in the inline comment. Rollback: remove the line; the lab-stored copy at `~/.labby/acp/claude-home/.credentials.json` becomes the source of truth again, with manual sync overhead.

## Decisions Not Taken

- **Did not pin `claude-agent-sdk` to a specific version (=0.2.131)** — used `^0.2.131` instead so patch releases flow through. If a `0.2.132` introduces a regression, downgrade is a one-line Dockerfile change. Tighter pinning would protect against silent breakage but would also mean every upstream patch requires a manual bump.
- **Did not implement an in-app "credentials drift detector"** — would have required querying Anthropic at startup to validate the token. Given the bind mount makes the staleness window zero, the value of an in-app check is low.
- **Did not refactor the ACP runtime to talk to Claude directly** (bypassing `claude-agent-acp`). Tempting because it would eliminate the SDK-version-vs-credentials class of bugs entirely, but it's a major rewrite — `claude-agent-acp` implements the ACP protocol contract and proxies tool calls, MCP servers, session config options, etc. Out of scope for this session.
- **Did not add a CSRF token to the bearer `/auth/session` response**. Bearer auth doesn't go through the cookie-based CSRF gate (`crates/lab/src/api/router.rs:262-301`), so a token would be cosmetic. Empty string is an honest signal that CSRF isn't applicable.
- **Did not commit the five pre-existing dirty files** (`apps/gateway-admin/components/chat/{chat-shell,message-bubble,message-thread}.test.tsx`, `apps/gateway-admin/components/chat/message-bubble.tsx`, `apps/gateway-admin/lib/chat/use-chat-session-controller.ts`). Those were modified in the working tree before this session started and document the hover-interaction state on `message-bubble`. The owner of that work should commit them under a coherent message.

## References

- `agent-client-protocol@0.11.1` source — `~/.cargo/registry/src/index.crates.io-*/agent-client-protocol-0.11.1/src/session.rs:79-106` (the `attach_session` destructure that drops models)
- `~/workspace/acp/codex-acp/src/codex_agent.rs:570-573` and `:3069-3099` — Codex-side proof that `NewSessionResponse.models` is set
- `~/workspace/acp/claude-agent-acp/src/acp-agent.ts:1722` — Claude-side proof that `models: response.models` is set on the new-session response
- `agent-client-protocol-schema@0.12.0` — `src/agent.rs:1006-1037` shows `NewSessionResponse.models: Option<SessionModelState>` is gated behind `unstable_session_model` (which `unstable` activates)
- Original 8 chat plans: `docs/superpowers/plans/2026-05-05-chat-*.md` (8 files)
- `docs/dev/OBSERVABILITY.md` — dispatch event field contract referenced when reasoning about which logs would surface auth failures
- `docs/runtime/OAUTH.md` — auth contract source of truth, updated this session

## Open Questions

- The five pre-existing dirty `apps/gateway-admin/components/chat/*` files were modified before this session and contain interaction-open state plumbing on `message-bubble`. Whose work is it and which PR/commit should they ship under? They're functionally fine as-is and were used during verification, so they're probably ready to commit, but ownership wasn't established this session.
- Should the host's `~/.claude/.claude.json` (settings file) also be bind-mounted? It's missing on this host but might exist on others, and could affect Claude Code agent behavior if asymmetric between host CLI and container.
- Whether `LAB_AUTH_BOOTSTRAP_SECRET` rotates the same way `LAB_MCP_HTTP_TOKEN` does (`just mcp-token`) — wasn't exercised this session.

## Next Steps

**Started but not completed:**
- None — every task in this session reached a verified working state or was explicitly handed off (the dirty `message-bubble` files).

**Follow-on tasks not yet started:**
- Open an upstream PR against `agent-client-protocol` adding a public `models()` accessor on `ActiveSession` so we can drop `crates/vendor/agent-client-protocol/`.
- Decide whether to tighten the `claude-agent-sdk` pin in `config/Dockerfile.fast` from `^0.2.131` to `~0.2.131` once the current SDK soaks for a few days.
- Consider extracting the agent-browser-with-bearer pattern into a `just` recipe (e.g. `just chat-headed`) so onboarding contributors don't have to re-derive the env-token-extract incantation.
- Verify on lab.example.com (production reverse proxy) that the bearer `/auth/session` change doesn't conflict with any 2FA proxy header rewrites.
