# 2026-05-10 Gateway protected route save fix

## Metadata

- Date: 2026-05-10 20:01:20 EST
- Repository: `git@github.com:jmagar/lab.git`
- Working directory: `/home/jmagar/workspace/lab`
- Branch: `fix/protected-route-edit-state`
- HEAD: `a49d2723`
- Transcript: `/home/jmagar/.claude/projects/-home-jmagar-workspace-lab/c7f3c5ad-9a4d-489b-8768-ed4d125abf5a.jsonl`
- Active plan: none

## User Request

Investigate why adding an MCP server through the gateway UI said it succeeded but the server did not appear in the gateway, then fix it and save the session notes to markdown.

## Session Overview

The investigation found a split-write bug in the gateway admin UI. The upstream gateway creation failed, but the parent save callback swallowed that failure, so the form dialog continued and saved a protected route anyway. This created an orphan `mem0` protected route pointing at a missing upstream, which made the UI report a successful add while `gateway.list`, `gateway.mcp.list`, and `gateway.get mem0` had no matching gateway.

The UI save flow was changed so create/update failures reject back to the dialog. That prevents protected-route saving after a failed gateway save. The existing orphan `mem0` protected route was removed from the live Lab gateway configuration, and focused gateway-admin tests passed.

## Sequence

1. Inspected the live gateway config, API responses, and Docker logs.
2. Confirmed `~/.labby/config.toml` contained a `[[protected_mcp_routes]]` entry for `mem0` but no matching `[[upstream]] name = "mem0"`.
3. Confirmed live API behavior:
   - `gateway.protected_route.list` included `mem0`.
   - `gateway.get mem0` returned a `not_found` envelope.
   - `gateway.list` and `gateway.mcp.list` did not include `mem0`.
4. Found logs showing `gateway.add` for `mem0` failed with `invalid_param`, followed immediately by successful `gateway.protected_route.add` for the same name.
5. Traced the UI flow and found `handleSave` caught `createGateway` / `updateGateway` errors without rethrowing, causing `GatewayFormDialog` to continue after `await onSave(buildInput())`.
6. Patched the gateway list and detail save callbacks to let failures reject to the form dialog.
7. Removed the orphan live protected route for `mem0`.
8. Ran focused gateway-admin tests.

## Key Findings

- The attempted MCP server target was `http://localhost:8888/mcp`.
- The gateway URL validator for custom HTTP gateways requires `https://` and rejects unsafe local/private-style targets for this path.
- No process was listening on port `8888`; direct curls to `127.0.0.1:8888/mcp` and `localhost:8888/mcp` failed with connection refused.
- The live orphan route was caused by frontend sequencing, not by a successfully registered upstream disappearing later.
- Server-side protected route creation still allowed a route to reference a missing upstream; this remains a backend invariant gap worth considering separately.

## Technical Decisions

- Let `GatewayFormDialog` remain the owner of form-level error handling.
- Removed local `try/catch` blocks in parent save callbacks where they swallowed create/update failures.
- Did not create a `mem0` upstream manually because the provided target was invalid for the custom HTTP gateway path and no local service was listening on `8888`.
- Removed the orphan route instead of leaving a misleading protected-route entry in live config.

## Files Modified

- `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
  - Gateway create/update errors now reject through the save callback instead of being swallowed by the parent.
- `apps/gateway-admin/components/gateway/gateway-detail-content.tsx`
  - Gateway edit update errors now reject through the save callback instead of being swallowed by the parent.
- `docs/sessions/2026-05-10-gateway-protected-route-save-fix.md`
  - This session record.

Current dirty worktree at save time:

- `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
- `apps/gateway-admin/lib/gateway-protected-route.test.ts`
- `apps/gateway-admin/lib/gateway-protected-route.ts`

Those dirty files were present at session-save time and were not edited by this markdown save.

## Important Commands And Evidence

Live API cleanup command:

```bash
TOKEN=$(grep '^LAB_MCP_HTTP_TOKEN=' .env ~/.labby/.env 2>/dev/null | tail -1 | cut -d= -f2-)
curl -sS \
  -H "Authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  http://127.0.0.1:8765/v1/gateway \
  --data '{"action":"gateway.protected_route.remove","params":{"confirm":true,"name":"mem0"}}'
```

Cleanup result:

- Returned the removed `mem0` route object.
- Follow-up `gateway.protected_route.list` no longer included `mem0`.
- Follow-up `gateway.get mem0` still returned `not_found`, which is expected because no upstream gateway exists.

Focused verification:

```bash
pnpm exec tsx --test components/gateway/gateway-list-content.test.tsx lib/api/gateway-client.test.ts
```

Result:

- 21 tests passed.
- 0 tests failed.

## Behavior Change

Before:

- Gateway creation could fail.
- Parent save callback caught the error and returned normally.
- The dialog then saved a protected route and closed as if the combined operation succeeded.
- The UI could show success while the gateway was absent from gateway lists.

After:

- Gateway creation/update failure rejects back to `GatewayFormDialog`.
- The protected-route save step does not run after a failed gateway save.
- The user sees the actual gateway save error instead of a misleading success path.

## Risks And Rollback

- The frontend change is intentionally narrow and changes only error propagation from parent save callbacks.
- If rollback is needed, restore the previous local `try/catch` behavior in the two gateway content components, though that would reintroduce the split-write bug.
- The live `mem0` protected route removal affected local machine config through the Lab API. Re-add it only after a valid upstream named `mem0` exists.

## Decisions Not Taken

- No backend validation was added to reject protected routes whose `upstream` does not exist.
- No new `mem0` gateway was registered.
- No broad all-features Rust verification was run for this frontend-only flow.

## Open Questions

- Should `gateway.protected_route.add` reject `upstream` values that do not match an existing configured upstream?
- Should the UI disable or warn on `http://localhost` targets for custom HTTP gateway additions before submitting to the backend?
- Should there be an integration test that explicitly asserts the protected route save does not run when `onSave` rejects?

## Next Steps

- Add backend protected-route upstream validation if the gateway config model should enforce that invariant.
- Add a UI regression test around the dialog sequence if the current test harness can mock `onSave` rejection and protected-route save calls cleanly.
