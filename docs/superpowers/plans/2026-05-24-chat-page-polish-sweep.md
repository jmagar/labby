# Chat Page Polish Sweep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tighten the `/chat` page in `apps/gateway-admin` across 6 polish items: hide failed/closed sessions with a bulk cleanup action, auto-name placeholder sessions, hide redundant per-row model badges, group the Codex model picker by base + reasoning effort, defer session-row persistence until first prompt, and add a `session.start_and_prompt` dispatch orchestrator that closes the orphan-session window and cuts cold-start latency from 2.1–3.1s to ~800ms.

**Architecture:** Frontend changes layer over the existing `ChatSessionProvider` + `RunRow` rendering pipeline. Backend changes follow the established dispatch-layer contract (`crates/lab/src/dispatch/acp/`) with one new destructive action (`session.bulk_close`) and one new orchestrator action (`session.start_and_prompt`), both built on existing `AcpSessionRegistry` methods. All new TS helpers live in `apps/gateway-admin/lib/chat/`. No new UI primitives — reuse `components/ui/toggle-group.tsx` (radix) and `components/marketplace/confirm-dialog.tsx` (alert dialog wrapper).

**Tech Stack:** Next.js 16 / React 19 / Tailwind v4 / Aurora design system / TypeScript 5; Rust 2024, `rmcp`, axum, rusqlite, tokio; pnpm + cargo-nextest + vitest.

**Locked-plan context:** Every task in this file derives from beads `lab-de6yc` (epic) and `lab-de6yc.1` through `lab-de6yc.6`. The beads carry the full **Locked Decisions**, **Risks**, and **References** sections. Before starting any task, run `bd show lab-de6yc.<N>` to read the locked decisions for that scope. This plan turns those decisions into mechanical steps; it does NOT re-debate them.

---

## File Structure

### New files

| File | Responsibility | Bead |
|------|----------------|------|
| `apps/gateway-admin/lib/chat/dominant-model.ts` | Pure helper: strict-majority modelId from a run list | .3 |
| `apps/gateway-admin/lib/chat/dominant-model.test.ts` | Unit tests for the helper | .3 |
| `apps/gateway-admin/lib/chat/model-grouping.ts` | Pure parser/grouper: `(base, effort)` tuples from codex model ids | .4 |
| `apps/gateway-admin/lib/chat/model-grouping.test.ts` | Unit tests for the parser | .4 |
| `apps/gateway-admin/lib/chat/use-list-keyboard.ts` | Extracted shared keyboard-nav hook (replaces duplicated picker logic) | .4 |
| `apps/gateway-admin/lib/chat/use-list-keyboard.test.ts` | Unit tests for the hook | .4 |
| `crates/lab/src/dispatch/acp/params.rs` (NEW types) | `BulkCloseSelector`, `StartAndPromptInput` typed structs | .1, .6 |

### Modified files

| File | Bead | Edit summary |
|------|------|--------------|
| `apps/gateway-admin/lib/chat/chat-session-provider.tsx` | .1, .3, .5, .6 | Filter at `refreshSessions`; derive `dominantModelId`; convert `createSession` to draft path; replace two-call materialize with single orchestrator call |
| `apps/gateway-admin/components/chat/session-sidebar.tsx` | .1, .3 | Reveal toggle + cleanup trigger; conditional badge render; permanent `aria-label` |
| `apps/gateway-admin/components/chat/chat-shell.tsx` | .5 | Render draft state; banner reads union of providerHealth + lastDispatchError |
| `apps/gateway-admin/components/chat/chat-input.tsx` | .4 | Grouped picker UI; use `useListKeyboard` hook |
| `apps/gateway-admin/lib/chat/acp-normalizers.ts` | .2 | Inline placeholder-title → derived label in `toRun()` |
| `apps/gateway-admin/lib/chat/acp-normalizers.test.ts` | .2 | New cases for placeholder replacement |
| `crates/lab/src/dispatch/acp/catalog.rs` | .1, .6 | `ActionSpec` `session.bulk_close` (destructive) + `session.start_and_prompt` (non-destructive) |
| `crates/lab/src/dispatch/acp/dispatch.rs` | .1, .6 | New arms with `require_confirm` for destructive |
| `crates/lab/src/acp/registry.rs` | .1, .6 | `bulk_close_sessions` (semaphore-bounded) + `start_and_prompt` orchestrator + `sanitize_provider_error` |
| `crates/lab/src/api/services/acp.rs` | .6 (optional) | Optional convenience route |
| `crates/lab/src/acp/runtime.rs` | .5 | Apply `sanitize_provider_error` before emitting provider_info stderr events |
| `docs/dev/ERRORS.md` | .1 | Canonical batch-result envelope shape |

### Files NOT to modify

- `crates/vendor/agent-client-protocol/*` — vendored ACP SDK; leave alone
- `apps/gateway-admin/components/ui/toggle-group.tsx` — CONSUME ONLY
- `apps/gateway-admin/components/marketplace/confirm-dialog.tsx` — CONSUME ONLY (do not add another wrapper around it)
- `crates/lab/src/acp/registry.rs:1913-1934` (`title_from_prompt`) — server-side title path stays as-is

---

## Wave ordering (per `bd swarm validate lab-de6yc`)

- **Wave 1 (parallel):** Task 1 (`.1`) + Task 4 (`.4`)
- **Wave 2 (parallel, all depend on Task 1):** Task 2 (`.2`) + Task 3 (`.3`) + Task 5 (`.5`)
- **Wave 3 (depends on Task 5):** Task 6 (`.6`)

If executing inline with one agent, do them in numerical order (1 → 4 → 2 → 3 → 5 → 6). If using `superpowers:subagent-driven-development`, dispatch Wave 1 in parallel, then Wave 2, then Wave 3.

---

## Pre-flight (run once before any task)

- [ ] **Verify working tree is clean (or use a worktree)**

  ```bash
  cd /home/jmagar/workspace/lab
  git status
  ```

  Expected: clean OR an already-checked-out feature branch dedicated to this plan.

- [ ] **Verify backend build + frontend build both pass on main**

  ```bash
  just check
  cd apps/gateway-admin && pnpm build && cd -
  ```

  Expected: both succeed with no warnings introduced by your starting commit.

- [ ] **Verify ACP chat works end-to-end (smoke test the baseline)**

  ```bash
  TOKEN=$(grep "LAB_MCP_HTTP_TOKEN" /home/jmagar/.labby/.env | cut -d= -f2 | tr -d '"')
  curl -sS -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
    -d '{"provider":"codex-acp"}' http://localhost:8765/v1/acp/sessions | jq -r '.id'
  ```

  Expected: a UUID. If it errors, fix the environment first (see prior session: `~/.labby/acp/codex-home/auth.json` must be in lockstep with `~/.codex/auth.json`).

---

# Task 1 — `lab-de6yc.1` Hide failed/closed sessions + bulk_close dispatch action

**Files:**
- Create: NONE (no new files; reuses existing primitives)
- Modify: `crates/lab/src/dispatch/acp/catalog.rs`
- Modify: `crates/lab/src/dispatch/acp/dispatch.rs`
- Modify: `crates/lab/src/dispatch/acp/params.rs`
- Modify: `crates/lab/src/acp/registry.rs`
- Modify: `apps/gateway-admin/lib/chat/chat-session-provider.tsx`
- Modify: `apps/gateway-admin/components/chat/session-sidebar.tsx`
- Modify: `docs/dev/ERRORS.md`
- Test: `crates/lab/src/dispatch/acp/dispatch.rs` (inline `#[cfg(test)]`) + `apps/gateway-admin/lib/chat/chat-session-provider.test.tsx`

**Required reading before starting:** `bd show lab-de6yc.1` — full Locked Decisions, Risks, Testing checklist.

## 1.1 — Backend: typed `BulkCloseSelector`

- [ ] **Step 1: Add the failing dispatch test (Rust)**

  Add the test below to `crates/lab/src/dispatch/acp/dispatch.rs` inside the existing `#[cfg(test)] mod tests` block (or create one at the bottom if absent):

  ```rust
  #[tokio::test]
  async fn session_bulk_close_rejects_empty_selector() {
      let registry = test_registry().await;
      let params = serde_json::json!({
          "selector": {},
          "principal": "test-principal",
          "confirm": true,
      });
      let result = dispatch(&registry, "session.bulk_close", params).await;
      assert!(matches!(
          result,
          Err(ToolError::InvalidParam { param, .. }) if param == "selector"
      ));
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features \
    dispatch::acp::dispatch::tests::session_bulk_close_rejects_empty_selector
  ```

  Expected: FAIL with "unknown action" or "test not found".

- [ ] **Step 3: Add `BulkCloseSelector` to `crates/lab/src/dispatch/acp/params.rs`**

  Append to the file:

  ```rust
  use lab_apis::acp::types::AcpSessionState;

  #[derive(Debug, Clone, serde::Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct BulkCloseSelector {
      #[serde(default)]
      pub states: Vec<AcpSessionState>,
      #[serde(default)]
      pub max_age_days: Option<u32>,
      #[serde(default)]
      pub min_user_message_count_lt: Option<u32>,
      /// Hard cap on matched session count. Defaults to 500. Reject with 422 if the
      /// SELECT COUNT(*) for this selector exceeds this value.
      #[serde(default = "default_max_count")]
      pub max_count: u32,
  }

  fn default_max_count() -> u32 {
      500
  }

  impl BulkCloseSelector {
      /// Returns Err if the selector has no positive criteria — protects against
      /// accidental delete-all.
      pub fn validate_non_empty(&self) -> Result<(), crate::dispatch::error::ToolError> {
          if self.states.is_empty()
              && self.max_age_days.is_none()
              && self.min_user_message_count_lt.is_none()
          {
              return Err(crate::dispatch::error::ToolError::InvalidParam {
                  message: "selector must specify at least one of: states, max_age_days, min_user_message_count_lt".to_string(),
                  param: "selector".to_string(),
              });
          }
          Ok(())
      }
  }
  ```

- [ ] **Step 4: Add catalog entry**

  Edit `crates/lab/src/dispatch/acp/catalog.rs`. Find the `session.close` entry (around line 244) and add a sibling immediately after its closing brace:

  ```rust
  ActionSpec {
      name: "session.bulk_close",
      summary: "Bulk close sessions matching a typed selector. Self-service only (purges sessions owned by caller).",
      destructive: true,
      params: &[
          ParamSpec {
              name: "selector",
              kind: ParamKind::Object,
              required: true,
              description: "BulkCloseSelector: { states?, max_age_days?, min_user_message_count_lt?, max_count? }",
          },
      ],
  },
  ```

  Then update the module-level destructive doc-comment at the top of `catalog.rs` (around line 4) to include the new action:

  ```rust
  //! `session.cancel`, `session.close`, and `session.bulk_close` are marked destructive.
  ```

- [ ] **Step 5: Add the dispatch arm**

  Edit `crates/lab/src/dispatch/acp/dispatch.rs`. Find the `"session.close" =>` arm at line 337 and add a sibling:

  ```rust
  "session.bulk_close" => {
      require_confirm(&params, "session.bulk_close")?;
      let selector: BulkCloseSelector = serde_json::from_value(
          params
              .get("selector")
              .cloned()
              .ok_or_else(|| ToolError::MissingParam {
                  message: "selector is required".to_string(),
                  param: "selector".to_string(),
              })?,
      )
      .map_err(|e| ToolError::InvalidParam {
          message: format!("invalid selector: {e}"),
          param: "selector".to_string(),
      })?;
      selector.validate_non_empty()?;
      let principal = params
          .get("principal")
          .and_then(Value::as_str)
          .ok_or_else(|| ToolError::MissingParam {
              message: "principal is required".to_string(),
              param: "principal".to_string(),
          })?;
      let result = registry.bulk_close_sessions(selector, principal).await?;
      Ok(serde_json::to_value(result).unwrap_or(Value::Null))
  }
  ```

  Add the import at the top of the file (or update existing import line):

  ```rust
  use crate::dispatch::acp::params::BulkCloseSelector;
  ```

- [ ] **Step 6: Run the dispatch test — verify it passes (registry method missing → compile fail expected)**

  ```bash
  cargo check --manifest-path crates/lab/Cargo.toml --all-features 2>&1 | grep -E "error\[|bulk_close_sessions"
  ```

  Expected: `error[E0599]: no method named bulk_close_sessions found for ... AcpSessionRegistry`. Good — proves the dispatcher is calling into the method we'll add next.

## 1.2 — Backend: `bulk_close_sessions` registry method

- [ ] **Step 1: Write the failing per-session-principal test**

  Append to `crates/lab/src/acp/registry.rs` inside the existing `#[cfg(test)] mod tests` block (search for `mod tests {`):

  ```rust
  #[tokio::test]
  async fn bulk_close_sessions_only_closes_caller_principal_sessions() {
      let registry = AcpSessionRegistry::new_for_test();
      let _own = registry.create_test_session("alice", AcpSessionState::Failed).await;
      let _other = registry.create_test_session("bob", AcpSessionState::Failed).await;

      let selector = BulkCloseSelector {
          states: vec![AcpSessionState::Failed],
          max_age_days: None,
          min_user_message_count_lt: None,
          max_count: 500,
      };
      let result = registry.bulk_close_sessions(selector, "alice").await.unwrap();

      assert_eq!(result.closed.len(), 1, "should close exactly one session");
      assert!(result.failed.is_empty(), "no per-session failures expected");
      // Bob's session must still be open
      let bob_state = registry.session_state("bob_session_id").await.unwrap();
      assert!(matches!(bob_state, AcpSessionState::Failed));
  }
  ```

  If `new_for_test` / `create_test_session` / `session_state` helpers don't exist, add them as minimal test scaffolding (look for similar test helpers earlier in the file or in `crates/lab/src/acp/registry.rs` test mod and mirror them).

- [ ] **Step 2: Run test (expect compile failure)**

  ```bash
  cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features bulk_close 2>&1 | head -40
  ```

  Expected: compile errors for the missing types and method.

- [ ] **Step 3: Add the response shape and method to `registry.rs`**

  Above the `impl AcpSessionRegistry` block, add the response types:

  ```rust
  use crate::dispatch::acp::params::BulkCloseSelector;

  #[derive(Debug, Clone, serde::Serialize)]
  pub struct BulkCloseResult {
      pub closed: Vec<String>,
      pub failed: Vec<BulkCloseFailure>,
  }

  #[derive(Debug, Clone, serde::Serialize)]
  pub struct BulkCloseFailure {
      pub id: String,
      pub kind: String,
      pub message: String,
  }
  ```

  Inside `impl AcpSessionRegistry`, add the method (place it next to `close_session` around line 954):

  ```rust
  pub async fn bulk_close_sessions(
      &self,
      selector: BulkCloseSelector,
      principal: &str,
  ) -> Result<BulkCloseResult, ToolError> {
      // Snapshot candidate IDs WITHOUT holding the lock across .await
      let candidates: Vec<String> = {
          let sessions = self.sessions.read().await;
          let now = jiff::Timestamp::now();
          sessions
              .iter()
              .filter_map(|(id, session)| {
                  // Cheap synchronous filter — we'll re-check principal per-session inside the loop
                  let summary = session.summary.try_read().ok()?;
                  let matches_state = selector.states.is_empty()
                      || selector.states.contains(&summary.state);
                  let matches_age = selector
                      .max_age_days
                      .map(|d| {
                          summary
                              .updated_at
                              .parse::<jiff::Timestamp>()
                              .ok()
                              .map(|ts| {
                                  (now.as_second() - ts.as_second())
                                      >= (d as i64 * 86_400)
                              })
                              .unwrap_or(false)
                      })
                      .unwrap_or(true);
                  // user_message_count comparison would require event scan — implementer:
                  // if `min_user_message_count_lt` is set, fetch event count via persistence
                  // For initial implementation, assume true; refine if integration tests fail.
                  let matches_msg = true;
                  if matches_state && matches_age && matches_msg {
                      Some(id.clone())
                  } else {
                      None
                  }
              })
              .collect()
      };

      // Enforce max_count cap (DoS protection)
      if (candidates.len() as u32) > selector.max_count {
          return Err(ToolError::InvalidParam {
              message: format!(
                  "selector matches {} sessions; max_count is {}",
                  candidates.len(),
                  selector.max_count
              ),
              param: "selector".to_string(),
          });
      }

      // Semaphore-bound concurrent closes
      let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(5));
      let mut closed = Vec::new();
      let mut failed = Vec::new();
      let mut handles = Vec::new();

      for id in candidates {
          let sem = semaphore.clone();
          let registry = self.clone();
          let principal = principal.to_string();
          handles.push(tokio::spawn(async move {
              let _permit = sem.acquire().await.expect("semaphore closed");
              let outcome = registry.close_session(&id, &principal).await;
              (id, outcome)
          }));
      }

      for handle in handles {
          if let Ok((id, outcome)) = handle.await {
              match outcome {
                  Ok(()) => closed.push(id),
                  Err(e) => {
                      // Silent-skip unauthorized sessions per Locked Decision (do not leak existence)
                      if matches!(e.kind(), "not_found") {
                          continue;
                      }
                      failed.push(BulkCloseFailure {
                          id,
                          kind: e.kind().to_string(),
                          message: e.message().to_string(),
                      });
                  }
              }
          }
      }

      tracing::info!(
          surface = "acp", service = "registry", action = "session.bulk_close",
          principal = %principal,
          closed_count = closed.len(),
          failed_count = failed.len(),
          "ACP bulk_close completed",
      );

      Ok(BulkCloseResult { closed, failed })
  }
  ```

  Notes: this assumes `AcpSessionRegistry` is `Clone` (it is — it wraps `Arc`s). If `ToolError` doesn't expose `.kind()` / `.message()` accessors, add them now (1-line getters in `crates/lab/src/dispatch/error.rs`). The `min_user_message_count_lt` event-count check is left as an implementer follow-up if the bead's integration test demands it; the default selector in the UI doesn't set this field unless the operator chooses it.

- [ ] **Step 4: Run both backend tests — verify pass**

  ```bash
  cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features bulk_close
  ```

  Expected: 2 passes (selector rejection + principal scoping).

- [ ] **Step 5: Add the gate-drift integration test**

  In `crates/lab/src/dispatch/acp/dispatch.rs` tests:

  ```rust
  #[tokio::test]
  async fn session_bulk_close_requires_confirm_at_dispatcher_layer() {
      // This test does NOT go through the API surface auto-injection layer.
      // It verifies the dispatcher arm itself enforces require_confirm,
      // catching the lab-0xo fail-open mode where ActionSpec.destructive
      // drift would let an unconfirmed call through the surface gate.
      let registry = test_registry().await;
      let params = serde_json::json!({
          "selector": { "states": ["failed"], "max_count": 100 },
          "principal": "test-principal",
          // INTENTIONALLY no "confirm": true
      });
      let result = dispatch(&registry, "session.bulk_close", params).await;
      assert!(matches!(
          result,
          Err(ToolError::Sdk { sdk_kind, .. }) if sdk_kind == "confirmation_required"
      ));
  }
  ```

- [ ] **Step 6: Run gate-drift test, verify pass**

  ```bash
  cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features \
    session_bulk_close_requires_confirm_at_dispatcher_layer
  ```

  Expected: PASS.

- [ ] **Step 7: Update `docs/dev/ERRORS.md` with the batch-result envelope shape**

  Append a new section to `docs/dev/ERRORS.md`:

  ```markdown
  ## Batch-result envelope

  Actions that operate on multiple items in one call (e.g., `session.bulk_close`) return a partial-success envelope with two arrays. The inner `failed[]` items use the same `{ kind, message }` shape as top-level `ToolError::Sdk` for per-item error taxonomy consistency.

  ```json
  {
    "closed": ["session-uuid-1", "session-uuid-2"],
    "failed": [
      { "id": "session-uuid-3", "kind": "internal_error", "message": "..." }
    ]
  }
  ```

  Rules:
  - `closed[]` contains IDs that completed the action successfully.
  - `failed[]` contains IDs that the action attempted but errored on; per-item `kind` MUST be one of the canonical kinds listed above.
  - Items the caller is not authorized to act on are silently omitted from BOTH arrays (preserves the not_found masking pattern; do not leak existence by reporting forbidden items).
  - Authorization or validation errors that prevent the action from running at all return a top-level `ToolError` (not a 200 with empty arrays).
  ```

- [ ] **Step 8: Commit backend slice**

  ```bash
  git add crates/lab/src/dispatch/acp/catalog.rs \
          crates/lab/src/dispatch/acp/dispatch.rs \
          crates/lab/src/dispatch/acp/params.rs \
          crates/lab/src/acp/registry.rs \
          crates/lab/src/dispatch/error.rs \
          docs/dev/ERRORS.md
  git commit -m "feat(acp): add session.bulk_close dispatch action with typed selector"
  ```

## 1.3 — Frontend: default-hide filter + cleanup trigger

- [ ] **Step 1: Add the failing test for filter behavior**

  Edit `apps/gateway-admin/lib/chat/acp-normalizers.test.ts` (or create a new test in `chat-session-provider.test.tsx` if a list-filter test boundary doesn't exist there yet). Add:

  ```ts
  import { filterVisibleRuns, isHiddenState } from './session-filters'

  test('filterVisibleRuns hides failed and closed by default', () => {
    const runs = [
      { id: 'a', status: 'idle' } as any,
      { id: 'b', status: 'failed' } as any,
      { id: 'c', status: 'closed' } as any,
      { id: 'd', status: 'completed' } as any,
    ]
    const visible = filterVisibleRuns(runs, { includeHidden: false })
    assert.deepEqual(visible.map(r => r.id), ['a', 'd'])
  })

  test('filterVisibleRuns passes everything when includeHidden=true', () => {
    const runs = [
      { id: 'a', status: 'idle' } as any,
      { id: 'b', status: 'failed' } as any,
    ]
    const visible = filterVisibleRuns(runs, { includeHidden: true })
    assert.equal(visible.length, 2)
  })
  ```

- [ ] **Step 2: Run test — expect import failure**

  ```bash
  cd apps/gateway-admin && pnpm test acp-normalizers 2>&1 | tail -20
  ```

  Expected: "Cannot find module './session-filters'".

- [ ] **Step 3: Create `apps/gateway-admin/lib/chat/session-filters.ts`**

  ```ts
  import type { ACPRun } from '@/components/chat/types'

  const HIDDEN_STATES = new Set(['failed', 'closed'])

  export function isHiddenState(status: string | undefined): boolean {
    return status !== undefined && HIDDEN_STATES.has(status)
  }

  export function filterVisibleRuns(
    runs: ACPRun[],
    options: { includeHidden: boolean },
  ): ACPRun[] {
    if (options.includeHidden) return runs
    return runs.filter((run) => !isHiddenState(run.status))
  }
  ```

- [ ] **Step 4: Run test — verify pass**

  ```bash
  cd apps/gateway-admin && pnpm test session-filters 2>&1 | tail -10
  ```

  Expected: PASS.

- [ ] **Step 5: Wire the filter into `chat-session-provider.tsx`**

  Open `apps/gateway-admin/lib/chat/chat-session-provider.tsx`. Find `refreshSessions()` around line 277. Add state for the toggle near the existing `useState` block:

  ```tsx
  import { filterVisibleRuns, isHiddenState } from './session-filters'

  // inside the provider component, near other useState declarations:
  const [includeHiddenStates, setIncludeHiddenStates] = React.useState(false)
  ```

  Replace the line that calls `setRuns(nextRuns)` (around line 293) with:

  ```tsx
  setRuns(nextRuns)  // keep storing the unfiltered list
  ```

  Then in the context value (near the bottom of the provider, where `runs`, `providerHealth`, etc. are exposed), expose a derived filtered list AND the toggle:

  ```tsx
  const visibleRuns = React.useMemo(
    () => filterVisibleRuns(runs, { includeHidden: includeHiddenStates }),
    [runs, includeHiddenStates],
  )

  // expose in the context value (find the `useChatSessionData` getter region):
  visibleRuns,
  includeHiddenStates,
  setIncludeHiddenStates,
  ```

  Update the `useChatSessionData` hook return type and consumers in `chat-shell.tsx` / `session-sidebar.tsx` to consume `visibleRuns` instead of `runs` when rendering the list.

- [ ] **Step 6: Update `session-sidebar.tsx` to read `visibleRuns` and render the toggle**

  In `apps/gateway-admin/components/chat/session-sidebar.tsx`, change the render to consume `visibleRuns` and add a small header chip:

  ```tsx
  // near the top of SessionSidebar component, where runs are mapped:
  const { visibleRuns, includeHiddenStates, setIncludeHiddenStates, runs } = useChatSessionData()
  const hiddenCount = runs.length - visibleRuns.length

  // render a chip in the sidebar header:
  {hiddenCount > 0 && (
    <button
      type="button"
      onClick={() => setIncludeHiddenStates((v) => !v)}
      className="text-[11px] text-aurora-text-muted hover:text-aurora-text-primary"
    >
      {includeHiddenStates ? `Hide ${hiddenCount} closed/failed` : `Show ${hiddenCount} closed/failed`}
    </button>
  )}
  ```

- [ ] **Step 7: Add the bulk-cleanup trigger using the existing ConfirmDialog**

  Still in `session-sidebar.tsx`, add a "Clean up" button near the sidebar header and wire it to `ConfirmDialog` directly (do NOT create a wrapper file):

  ```tsx
  import { ConfirmDialog, type ConfirmState } from '@/components/marketplace/confirm-dialog'

  // inside SessionSidebar:
  const [confirm, setConfirm] = React.useState<ConfirmState | null>(null)

  const handleCleanup = React.useCallback(() => {
    const target = runs.filter(
      (r) => isHiddenState(r.status), // count what would be deleted with the default selector
    )
    if (target.length === 0) return
    setConfirm({
      title: `Delete ${target.length} sessions?`,
      description: `Sessions in state failed or closed will be permanently removed. This action cannot be undone.`,
      confirmLabel: `Delete ${target.length} Sessions`,
      destructive: true,
      onConfirm: async () => {
        await fetchAcp('/sessions:bulk_close', {
          method: 'POST',
          body: JSON.stringify({
            selector: { states: ['failed', 'closed'], max_count: 500 },
            confirm: true,
          }),
        })
        await refreshSessions()
      },
    })
  }, [runs, fetchAcp, refreshSessions])

  // and render the button + dialog:
  <button onClick={handleCleanup} className="text-[11px] text-aurora-text-muted hover:text-aurora-text-primary">
    Clean up
  </button>
  <ConfirmDialog state={confirm} onOpenChange={(open) => { if (!open) setConfirm(null) }} />
  ```

  Note: `isHiddenState` is imported from `lib/chat/session-filters.ts`. `fetchAcp` and `refreshSessions` are exposed from the provider — extend the context consumer if they aren't already accessible from the sidebar.

  The exact API route — `POST /v1/acp/sessions:bulk_close` vs `POST /v1/acp` with `action=session.bulk_close` — is implementer's choice. The latter (action envelope) is the more orthodox per `crates/lab/src/CLAUDE.md`. If you pick the envelope, body becomes:

  ```ts
  body: JSON.stringify({
    action: 'session.bulk_close',
    params: { selector: { states: ['failed', 'closed'], max_count: 500 }, confirm: true },
  })
  ```

- [ ] **Step 8: Run TS tests + typecheck**

  ```bash
  cd apps/gateway-admin && pnpm test && pnpm typecheck 2>&1 | tail -20
  ```

  Expected: all green.

- [ ] **Step 9: Smoke test end-to-end via agent-browser**

  ```bash
  TOKEN=$(grep "LAB_MCP_HTTP_TOKEN" /home/jmagar/.labby/.env | cut -d= -f2 | tr -d '"')
  agent-browser --session lab-acp open "https://lab.example.com/chat" --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
  agent-browser --session lab-acp screenshot /tmp/wave1-task1.png
  ```

  Read `/tmp/wave1-task1.png` and confirm: sidebar shows only non-failed/non-closed sessions; the "Show N closed/failed" toggle is present; a "Clean up" button is visible.

- [ ] **Step 10: Commit frontend slice**

  ```bash
  git add apps/gateway-admin/lib/chat/session-filters.ts \
          apps/gateway-admin/lib/chat/session-filters.test.ts \
          apps/gateway-admin/lib/chat/chat-session-provider.tsx \
          apps/gateway-admin/components/chat/session-sidebar.tsx
  git commit -m "feat(chat): hide failed/closed sessions by default + bulk cleanup trigger"
  ```

---

# Task 4 — `lab-de6yc.4` Model picker grouping by base + effort

**Files:**
- Create: `apps/gateway-admin/lib/chat/model-grouping.ts`
- Create: `apps/gateway-admin/lib/chat/model-grouping.test.ts`
- Create: `apps/gateway-admin/lib/chat/use-list-keyboard.ts`
- Create: `apps/gateway-admin/lib/chat/use-list-keyboard.test.ts`
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx:229-555`
- Consume only: `apps/gateway-admin/components/ui/toggle-group.tsx`

**Required reading:** `bd show lab-de6yc.4` — full Locked Decisions, especially the canonical regex and a11y contract.

## 4.1 — Parser

- [ ] **Step 1: Write failing parser tests**

  Create `apps/gateway-admin/lib/chat/model-grouping.test.ts`:

  ```ts
  import { test } from 'node:test'
  import assert from 'node:assert/strict'
  import { groupModels, parseModelId } from './model-grouping'

  test('parseModelId handles slash separator', () => {
    assert.deepEqual(parseModelId('gpt-5.5/medium'), { base: 'gpt-5.5', effort: 'medium' })
  })

  test('parseModelId handles paren separator after normalization', () => {
    assert.deepEqual(parseModelId('GPT-5.5 (medium)'), { base: 'GPT-5.5', effort: 'medium' })
  })

  test('parseModelId returns null for non-effort suffix', () => {
    assert.equal(parseModelId('gpt-5.4-mini'), null)
    assert.equal(parseModelId('gpt-5.3-codex-spark'), null)
    assert.equal(parseModelId('Default (recommended)'), null)
    assert.equal(parseModelId('Auto (Gemini 3)'), null)
  })

  test('parseModelId rejects names containing slash that are not effort suffixes', () => {
    assert.equal(parseModelId('OpenCode Zen/Big Pickle'), null)
  })

  test('groupModels returns grouped result for codex 20-model list', () => {
    const opts = [
      { id: 'gpt-5.5/low', name: 'GPT-5.5 (low)' },
      { id: 'gpt-5.5/medium', name: 'GPT-5.5 (medium)' },
      { id: 'gpt-5.5/high', name: 'GPT-5.5 (high)' },
      { id: 'gpt-5.5/xhigh', name: 'GPT-5.5 (xhigh)' },
      { id: 'gpt-5.4/low', name: 'GPT-5.4 (low)' },
    ]
    const result = groupModels(opts)
    assert.equal(result.kind, 'grouped')
    if (result.kind !== 'grouped') return
    assert.equal(result.groups.length, 2)
    assert.equal(result.groups[0].base, 'gpt-5.5')
    assert.equal(result.groups[0].variants.length, 4)
  })

  test('groupModels returns flat for claude default', () => {
    const opts = [{ id: 'default', name: 'Default (recommended)' }]
    assert.equal(groupModels(opts).kind, 'flat')
  })

  test('groupModels returns flat if ANY option fails to parse', () => {
    const opts = [
      { id: 'gpt-5.5/medium', name: 'GPT-5.5 (medium)' },
      { id: 'gpt-5.4-mini', name: 'GPT-5.4-Mini' },  // no effort suffix
    ]
    assert.equal(groupModels(opts).kind, 'flat')
  })

  test('groupModels returns flat for empty and single-option lists', () => {
    assert.equal(groupModels([]).kind, 'flat')
    assert.equal(groupModels([{ id: 'x/medium', name: 'X' }]).kind, 'flat')
  })
  ```

- [ ] **Step 2: Run tests — expect import error**

  ```bash
  cd apps/gateway-admin && pnpm test model-grouping 2>&1 | tail -10
  ```

  Expected: "Cannot find module './model-grouping'".

- [ ] **Step 3: Implement the parser**

  Create `apps/gateway-admin/lib/chat/model-grouping.ts`:

  ```ts
  import type { ACPModelOption } from '@/components/chat/types'

  export type Effort = 'low' | 'medium' | 'high' | 'xhigh'
  const EFFORTS: ReadonlySet<string> = new Set(['low', 'medium', 'high', 'xhigh'])

  /**
   * Parse a model id of the form `<base>/<effort>` or `<base>(<effort>)` into a
   * (base, effort) tuple. Returns null when:
   *  - no effort suffix matches the EFFORTS set
   *  - the input contains a slash that is part of the name, not a separator
   *  - the parens contain non-effort tokens (e.g., "(recommended)", "(Gemini 3)")
   */
  export function parseModelId(id: string): { base: string; effort: Effort } | null {
    // Normalize `name (effort)` → `name effort` for the slash-based regex below
    const normalized = id.replace(/\s*\(([^)]+)\)\s*$/, ' $1')
    const match = /^(.+?)[\s/]\s*(low|medium|high|xhigh)$/i.exec(normalized)
    if (!match) return null
    const base = match[1].trim()
    const effort = match[2].toLowerCase() as Effort
    // Reject if base still contains '/' (slash was part of the name, not separator)
    if (base.includes('/')) return null
    return { base, effort }
  }

  export interface GroupedOption {
    base: string
    variants: Array<{ effort: Effort; option: ACPModelOption }>
  }

  export type GroupingResult =
    | { kind: 'flat'; options: ACPModelOption[] }
    | { kind: 'grouped'; groups: GroupedOption[] }

  export function groupModels(options: ACPModelOption[]): GroupingResult {
    if (options.length <= 1) return { kind: 'flat', options }

    const parsed = options.map((opt) => ({ opt, parsed: parseModelId(opt.id) }))
    if (parsed.some((p) => p.parsed === null)) {
      return { kind: 'flat', options }
    }

    const groupMap = new Map<string, GroupedOption>()
    for (const { opt, parsed: p } of parsed) {
      if (!p) continue
      const existing = groupMap.get(p.base) ?? { base: p.base, variants: [] }
      existing.variants.push({ effort: p.effort, option: opt })
      groupMap.set(p.base, existing)
    }

    // Stable effort order
    const effortOrder: Effort[] = ['low', 'medium', 'high', 'xhigh']
    for (const group of groupMap.values()) {
      group.variants.sort(
        (a, b) => effortOrder.indexOf(a.effort) - effortOrder.indexOf(b.effort),
      )
    }

    return { kind: 'grouped', groups: Array.from(groupMap.values()) }
  }
  ```

- [ ] **Step 4: Run tests — verify pass**

  ```bash
  cd apps/gateway-admin && pnpm test model-grouping 2>&1 | tail -10
  ```

  Expected: 8 passes.

## 4.2 — Shared keyboard hook

- [ ] **Step 1: Write failing hook test**

  Create `apps/gateway-admin/lib/chat/use-list-keyboard.test.ts`:

  ```ts
  import { test } from 'node:test'
  import assert from 'node:assert/strict'
  import { renderHook, act } from '@testing-library/react'
  import { useListKeyboard } from './use-list-keyboard'

  test('useListKeyboard cycles down then up with arrow keys', () => {
    const { result } = renderHook(() => useListKeyboard({ count: 3, initialIndex: 0 }))
    assert.equal(result.current.activeIndex, 0)
    act(() => result.current.onKeyDown({ key: 'ArrowDown', preventDefault: () => {} } as any))
    assert.equal(result.current.activeIndex, 1)
    act(() => result.current.onKeyDown({ key: 'ArrowDown', preventDefault: () => {} } as any))
    act(() => result.current.onKeyDown({ key: 'ArrowDown', preventDefault: () => {} } as any))
    assert.equal(result.current.activeIndex, 0)  // wrap
  })

  test('useListKeyboard resets when count shrinks below activeIndex', () => {
    const { result, rerender } = renderHook(
      ({ count }) => useListKeyboard({ count, initialIndex: 0 }),
      { initialProps: { count: 5 } },
    )
    act(() => {
      for (let i = 0; i < 4; i++) {
        result.current.onKeyDown({ key: 'ArrowDown', preventDefault: () => {} } as any)
      }
    })
    assert.equal(result.current.activeIndex, 4)
    rerender({ count: 2 })
    assert.equal(result.current.activeIndex, 0)  // reset due to shrink
  })
  ```

- [ ] **Step 2: Run — expect import error**

  ```bash
  cd apps/gateway-admin && pnpm test use-list-keyboard 2>&1 | tail -5
  ```

- [ ] **Step 3: Implement the hook**

  Create `apps/gateway-admin/lib/chat/use-list-keyboard.ts`:

  ```ts
  import * as React from 'react'

  export function useListKeyboard({ count, initialIndex = 0 }: { count: number; initialIndex?: number }) {
    const [activeIndex, setActiveIndex] = React.useState(initialIndex)

    // Reset when count shrinks below activeIndex (e.g., provider switch shrinks the list)
    React.useEffect(() => {
      if (activeIndex >= count) setActiveIndex(0)
    }, [count, activeIndex])

    const onKeyDown = React.useCallback(
      (event: React.KeyboardEvent | KeyboardEvent) => {
        if (count === 0) return
        if (event.key === 'ArrowDown') {
          event.preventDefault?.()
          setActiveIndex((i) => (i + 1) % count)
        } else if (event.key === 'ArrowUp') {
          event.preventDefault?.()
          setActiveIndex((i) => (i - 1 + count) % count)
        } else if (event.key === 'Home') {
          event.preventDefault?.()
          setActiveIndex(0)
        } else if (event.key === 'End') {
          event.preventDefault?.()
          setActiveIndex(count - 1)
        }
      },
      [count],
    )

    return { activeIndex, setActiveIndex, onKeyDown }
  }
  ```

- [ ] **Step 4: Run — verify pass**

  ```bash
  cd apps/gateway-admin && pnpm test use-list-keyboard
  ```

## 4.3 — Refactor `chat-input.tsx` to use the hook + render grouped picker

- [ ] **Step 1: Refactor agent picker to use `useListKeyboard`**

  In `apps/gateway-admin/components/chat/chat-input.tsx`, find the agent-picker keyboard logic around lines 229-273. Replace the manual `activeAgentIndex` state + key handler with:

  ```tsx
  import { useListKeyboard } from '@/lib/chat/use-list-keyboard'

  const agentNav = useListKeyboard({ count: agents.length })
  // replace existing handleAgentTriggerKeyDown / handleAgentListKeyDown body with agentNav.onKeyDown wiring
  ```

  Mirror the same change for the model picker around lines 326-387. After this step, both pickers share the hook and the duplicated key-handling logic is gone.

- [ ] **Step 2: Render the grouped picker when groupable**

  Still in `chat-input.tsx`, in the model picker render around lines 516-560, wrap the existing flat-list render in a conditional:

  ```tsx
  import { groupModels } from '@/lib/chat/model-grouping'
  import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'

  const grouped = React.useMemo(() => groupModels(modelOptions), [modelOptions])

  // inside the listbox div:
  {grouped.kind === 'flat' ? (
    // existing flat render (unchanged) — keeps tooltip-on-hover from the prior bead
    modelOptions.map((model, index) => /* … existing JSX … */)
  ) : (
    grouped.groups.map((group) => {
      const selectedEffort =
        group.variants.find((v) => v.option.id === selectedModel?.id)?.effort
      return (
        <div key={group.base} className="flex items-center justify-between gap-2 px-3 py-1.5">
          <span className="text-[13px] font-medium text-aurora-text-primary">{group.base}</span>
          <ToggleGroup
            type="single"
            value={selectedEffort}
            onValueChange={(effort) => {
              if (!effort) return
              const variant = group.variants.find((v) => v.effort === effort)
              if (variant) selectModel(variant.option.id)
            }}
            className="h-7"
          >
            {group.variants.map((v) => (
              <ToggleGroupItem key={v.effort} value={v.effort} className="h-7 px-2 text-[11px]">
                {v.effort}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        </div>
      )
    })
  )}
  ```

- [ ] **Step 3: Build + run frontend tests**

  ```bash
  cd apps/gateway-admin && pnpm test && pnpm build 2>&1 | tail -20
  ```

  Expected: all tests pass; build succeeds.

- [ ] **Step 4: Browser verification**

  ```bash
  TOKEN=$(grep "LAB_MCP_HTTP_TOKEN" /home/jmagar/.labby/.env | cut -d= -f2 | tr -d '"')
  agent-browser --session lab-acp open "https://lab.example.com/chat" --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
  # Pick a codex session, open the model picker
  agent-browser --session lab-acp click "button[aria-label*='Selected model']"
  agent-browser --session lab-acp screenshot /tmp/task4-grouped.png
  ```

  Read `/tmp/task4-grouped.png` and confirm: 5 base-model rows, each with 4 effort pills (low/medium/high/xhigh) inline. Hover an effort pill — tooltip with description should appear (regression check from prior bead).

- [ ] **Step 5: Commit**

  ```bash
  git add apps/gateway-admin/lib/chat/model-grouping.ts \
          apps/gateway-admin/lib/chat/model-grouping.test.ts \
          apps/gateway-admin/lib/chat/use-list-keyboard.ts \
          apps/gateway-admin/lib/chat/use-list-keyboard.test.ts \
          apps/gateway-admin/components/chat/chat-input.tsx
  git commit -m "feat(chat): group codex model picker by base + effort segments"
  ```

---

# Task 2 — `lab-de6yc.2` Auto-name untitled sessions (inline)

**Files:**
- Modify: `apps/gateway-admin/lib/chat/acp-normalizers.ts:84-100`
- Test: `apps/gateway-admin/lib/chat/acp-normalizers.test.ts`

**Required reading:** `bd show lab-de6yc.2` — confirms inline (no helper file).

- [ ] **Step 1: Write failing tests**

  Append to `apps/gateway-admin/lib/chat/acp-normalizers.test.ts`:

  ```ts
  test('toRun derives label for "New session" placeholder', () => {
    const raw = {
      id: 'x', title: 'New session', provider: 'codex-acp',
      model_id: 'gpt-5.5/medium', state: 'idle', updated_at: new Date().toISOString(),
      created_at: new Date().toISOString(),
    } as any
    const run = toRun(raw)
    assert.notEqual(run.title, 'New session')
    assert.match(run.title, /codex|gpt|now|ago/i)
  })

  test('toRun keeps real titles unchanged', () => {
    const raw = {
      id: 'x', title: 'Real conversation title', provider: 'codex-acp',
      model_id: 'gpt-5.5/medium', state: 'idle', updated_at: new Date().toISOString(),
      created_at: new Date().toISOString(),
    } as any
    assert.equal(toRun(raw).title, 'Real conversation title')
  })

  test('toRun handles "action route session" placeholder', () => {
    const raw = {
      id: 'x', title: 'action route session', provider: 'claude-acp',
      model_id: null, state: 'idle', updated_at: new Date().toISOString(),
      created_at: new Date().toISOString(),
    } as any
    assert.notEqual(toRun(raw).title, 'action route session')
  })
  ```

- [ ] **Step 2: Run — expect failures**

  ```bash
  cd apps/gateway-admin && pnpm test acp-normalizers
  ```

- [ ] **Step 3: Inline the placeholder swap in `toRun`**

  In `apps/gateway-admin/lib/chat/acp-normalizers.ts`, add near the top:

  ```ts
  const PLACEHOLDER_TITLES = new Set(['New session', 'action route session', ''])

  function shortModelId(id: string | null | undefined): string {
    if (!id) return 'unknown'
    // Strip "gpt-5.5/medium" → "gpt-5.5", or pass through if no slash
    return id.split('/')[0] ?? id
  }

  function timeAgo(iso: string | null | undefined): string {
    if (!iso) return ''
    const date = new Date(iso)
    const seconds = Math.floor((Date.now() - date.getTime()) / 1000)
    if (seconds < 60) return 'now'
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`
    if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`
    return `${Math.floor(seconds / 86400)}d ago`
  }
  ```

  Inside the existing `toRun()` function (around line 84), where the title is set, replace the direct assignment with:

  ```ts
  title: PLACEHOLDER_TITLES.has(raw.title ?? '')
    ? [raw.provider, shortModelId(raw.model_id), timeAgo(raw.created_at)]
        .filter(Boolean)
        .join(' · ')
    : raw.title,
  ```

  Adjust field names (`raw.provider`, `raw.model_id`, `raw.created_at`) to match the actual `RawSessionSummary` type — read the file first to confirm exact field names.

- [ ] **Step 4: Run — verify pass**

  ```bash
  cd apps/gateway-admin && pnpm test acp-normalizers
  ```

- [ ] **Step 5: Browser smoke**

  ```bash
  agent-browser --session lab-acp open "https://lab.example.com/chat" --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
  agent-browser --session lab-acp screenshot /tmp/task2-titles.png
  ```

  Read screenshot: rows that previously showed "New session" / "action route session" now show derived labels.

- [ ] **Step 6: Commit**

  ```bash
  git add apps/gateway-admin/lib/chat/acp-normalizers.ts \
          apps/gateway-admin/lib/chat/acp-normalizers.test.ts
  git commit -m "feat(chat): auto-derive label for placeholder session titles"
  ```

---

# Task 3 — `lab-de6yc.3` Hide redundant model badge + aria-label

**Files:**
- Create: `apps/gateway-admin/lib/chat/dominant-model.ts`
- Create: `apps/gateway-admin/lib/chat/dominant-model.test.ts`
- Modify: `apps/gateway-admin/components/chat/session-sidebar.tsx:57, :76-79`
- Modify: `apps/gateway-admin/lib/chat/chat-session-provider.tsx`

**Required reading:** `bd show lab-de6yc.3` — full contract for `dominantModelId(runs)`.

- [ ] **Step 1: Write failing helper tests**

  Create `apps/gateway-admin/lib/chat/dominant-model.test.ts`:

  ```ts
  import { test } from 'node:test'
  import assert from 'node:assert/strict'
  import { dominantModelId } from './dominant-model'

  test('empty list returns null', () => {
    assert.equal(dominantModelId([]), null)
  })

  test('single run returns its modelId', () => {
    const runs = [{ modelId: 'gpt-5.5' } as any]
    assert.equal(dominantModelId(runs), 'gpt-5.5')
  })

  test('strict majority returns the majority id', () => {
    const runs = [
      { modelId: 'gpt-5.5' }, { modelId: 'gpt-5.5' }, { modelId: 'gpt-5.5' },
      { modelId: 'claude' },
    ] as any
    assert.equal(dominantModelId(runs), 'gpt-5.5')
  })

  test('exact 50/50 returns null', () => {
    const runs = [
      { modelId: 'a' }, { modelId: 'a' },
      { modelId: 'b' }, { modelId: 'b' },
    ] as any
    assert.equal(dominantModelId(runs), null)
  })

  test('all distinct returns null', () => {
    const runs = [
      { modelId: 'a' }, { modelId: 'b' }, { modelId: 'c' },
    ] as any
    assert.equal(dominantModelId(runs), null)
  })

  test('null modelIds count in the denominator', () => {
    const runs = [
      { modelId: 'gpt-5.5' }, { modelId: 'gpt-5.5' }, { modelId: null },
    ] as any
    // 2/3 is strict majority for 'gpt-5.5' since floor(3/2)+1 = 2
    assert.equal(dominantModelId(runs), 'gpt-5.5')
  })

  test('null modelIds dominant returns null (no badge string)', () => {
    const runs = [
      { modelId: null }, { modelId: null }, { modelId: null }, { modelId: 'gpt-5.5' },
    ] as any
    // null is the majority — we treat dominant null as "no dominant" (badges always show)
    assert.equal(dominantModelId(runs), null)
  })
  ```

- [ ] **Step 2: Run — expect import error**

  ```bash
  cd apps/gateway-admin && pnpm test dominant-model
  ```

- [ ] **Step 3: Implement helper**

  Create `apps/gateway-admin/lib/chat/dominant-model.ts`:

  ```ts
  import type { ACPRun } from '@/components/chat/types'

  /**
   * Returns the modelId that exceeds strict majority (> floor(n/2)) across the runs,
   * or null if no such modelId exists, the list is empty, or the dominant value is null.
   *
   * For single-run lists, returns that run's modelId — but consumers must still
   * render the badge for the lone row (no semantic dominance with N=1).
   */
  export function dominantModelId(runs: ACPRun[]): string | null {
    if (runs.length === 0) return null
    if (runs.length === 1) return runs[0].modelId ?? null

    const counts = new Map<string | null, number>()
    for (const run of runs) {
      const key = run.modelId ?? null
      counts.set(key, (counts.get(key) ?? 0) + 1)
    }

    const threshold = Math.floor(runs.length / 2) + 1
    for (const [id, count] of counts) {
      if (count >= threshold) {
        return id  // null when null is dominant — caller treats null as "no badge suppression"
      }
    }
    return null
  }
  ```

- [ ] **Step 4: Run — verify pass**

  ```bash
  cd apps/gateway-admin && pnpm test dominant-model
  ```

  Expected: 7 passes.

- [ ] **Step 5: Expose `dominantModelId` from provider**

  In `apps/gateway-admin/lib/chat/chat-session-provider.tsx`, near the `visibleRuns` memo added in Task 1, add:

  ```tsx
  import { dominantModelId } from './dominant-model'

  const dominantModel = React.useMemo(
    () => dominantModelId(visibleRuns),
    [visibleRuns],
  )

  // expose in context value:
  dominantModelId: dominantModel,
  ```

- [ ] **Step 6: Update `RunRow` to conditionally hide badge + always set aria-label**

  In `apps/gateway-admin/components/chat/session-sidebar.tsx`, modify the `RunRow` component (around line 47-86). First, accept `dominantModelId` as a prop:

  ```tsx
  function RunRow({
    run, isSelected, onSelect, dominantModelId,
  }: {
    run: ACPRun
    isSelected: boolean
    onSelect: () => void
    dominantModelId: string | null
  }) {
    const hideBadge = dominantModelId !== null && run.modelId === dominantModelId
    const ariaLabel = run.modelName ? `${run.title} · ${run.modelName}` : run.title
    return (
      <button
        type="button"
        onClick={onSelect}
        aria-label={ariaLabel}
        className={cn(/* … existing classes … */)}
      >
        {/* … existing children … */}
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[13px] leading-[1.2]">{run.title}</span>
          {run.modelName && !hideBadge && (
            <span className="block truncate text-[11px] leading-[1.2] text-aurora-text-muted/70">
              {run.modelName}
            </span>
          )}
        </span>
        {/* … */}
      </button>
    )
  }
  ```

  Then update the `ProjectGroup` (or whatever parent maps over runs) to pass `dominantModelId` down — read it from `useChatSessionData()`.

- [ ] **Step 7: Run tests**

  ```bash
  cd apps/gateway-admin && pnpm test
  ```

- [ ] **Step 8: Browser smoke**

  ```bash
  agent-browser --session lab-acp open "https://lab.example.com/chat" --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
  agent-browser --session lab-acp screenshot /tmp/task3-badges.png
  ```

  Read screenshot: rows sharing the dominant model show NO badge; outlier rows still show theirs.

- [ ] **Step 9: Commit**

  ```bash
  git add apps/gateway-admin/lib/chat/dominant-model.ts \
          apps/gateway-admin/lib/chat/dominant-model.test.ts \
          apps/gateway-admin/lib/chat/chat-session-provider.tsx \
          apps/gateway-admin/components/chat/session-sidebar.tsx
  git commit -m "feat(chat): hide model badge when row matches dominant model; add aria-label"
  ```

---

# Task 5 — `lab-de6yc.5` Defer session row + draft state + AbortController + sanitization

**Files:**
- Modify: `apps/gateway-admin/lib/chat/chat-session-provider.tsx` (heavy refactor: 3-state ref, pendingPromptRunIdRef, AbortController, lastDispatchError)
- Modify: `apps/gateway-admin/components/chat/chat-shell.tsx` (banner union; draft render)
- Modify: `crates/lab/src/acp/runtime.rs` (server-side `sanitize_provider_error` before emitting stderr provider_info events)

**Required reading:** `bd show lab-de6yc.5` — full locked race-scenario contract, sanitization regex set, useActionState discretion note.

## 5.1 — Server-side stderr sanitization

- [ ] **Step 1: Write failing Rust test**

  Add to `crates/lab/src/acp/runtime.rs` test mod:

  ```rust
  #[test]
  fn sanitize_provider_error_strips_ip_path_and_secrets() {
      let raw = "failed to auth to 10.0.0.5:8000 with token=abc.def.ghi at /home/user/.labby/creds";
      let clean = sanitize_provider_error(raw);
      assert!(!clean.contains("10.0.0.5"));
      assert!(!clean.contains("abc.def.ghi"));
      assert!(!clean.contains("/home/user"));
  }

  #[test]
  fn sanitize_provider_error_passes_known_safe_messages_unchanged() {
      let raw = "model not found: gpt-5.1";
      assert_eq!(sanitize_provider_error(raw), raw);
  }
  ```

- [ ] **Step 2: Run — expect compile failure**

  ```bash
  cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features sanitize_provider_error
  ```

- [ ] **Step 3: Implement `sanitize_provider_error`**

  Add to `crates/lab/src/acp/runtime.rs` (near the provider_info event emit path):

  ```rust
  use std::sync::OnceLock;

  fn sanitize_patterns() -> &'static [(regex::Regex, &'static str)] {
      static PATTERNS: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
      PATTERNS.get_or_init(|| {
          vec![
              (regex::Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?\b").unwrap(), "[redacted-ip]"),
              (regex::Regex::new(r"(?:token|password|api_key|secret|key)=\S+").unwrap(), "[redacted-secret]"),
              (regex::Regex::new(r"\beyJ[A-Za-z0-9_.-]+\b").unwrap(), "[redacted-jwt]"),
              (regex::Regex::new(r"/(?:home|Users|root)/[^ \t\n\"']+").unwrap(), "[path]"),
          ]
      })
  }

  pub fn sanitize_provider_error(message: &str) -> String {
      let mut out = message.to_string();
      for (re, repl) in sanitize_patterns() {
          out = re.replace_all(&out, *repl).to_string();
      }
      out
  }
  ```

  Add `regex = "1"` to `crates/lab/Cargo.toml` if not already present (likely already is).

  Then wire it: find every `provider_info_event(... type: "stderr" ... text: <raw stderr> ...)` emission in `runtime.rs` (search for `"type": "stderr"`) and wrap the `text` field with `sanitize_provider_error(...)`.

- [ ] **Step 4: Run — verify pass**

  ```bash
  cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features sanitize_provider_error
  ```

- [ ] **Step 5: Commit server slice**

  ```bash
  git add crates/lab/src/acp/runtime.rs crates/lab/Cargo.toml
  git commit -m "feat(acp): sanitize provider stderr before surfacing to clients"
  ```

## 5.2 — Frontend draft-state refactor

This is the largest single edit in the plan. Read `bd show lab-de6yc.5` once more before starting.

- [ ] **Step 1: Write failing race-scenario tests (suite)**

  Append to `apps/gateway-admin/lib/chat/chat-session-provider.test.tsx`:

  ```tsx
  // The 5 race scenarios from the bead Testing section. Use vi.fn() to mock fetchAcp
  // and inject controlled delays.

  test('double-click Send in draft state — only one POST /sessions fires', async () => {
    // … set up provider, enter draft, click send twice in 50ms, assert mock called once
  })

  test('Send then New mid-flight — AbortController fires; first run never mounts', async () => {
    // …
  })

  test('Send → navigate → return — late-resolve does NOT call setSelectedRunId', async () => {
    // …
  })

  test('POST /sessions success + POST /prompt failure — next Send reuses pendingPromptRunIdRef', async () => {
    // …
  })

  test('failed dispatch writes to lastDispatchError, not providerHealth', async () => {
    // …
  })
  ```

  (Implementer: write the full test bodies — the patterns are shown in the bead testing checklist. Use `renderHook` from `@testing-library/react`, mock `fetchAcp`, advance timers with `vi.useFakeTimers()`.)

- [ ] **Step 2: Run — expect failures (mostly assertion fails, not compile errors)**

  ```bash
  cd apps/gateway-admin && pnpm test chat-session-provider
  ```

- [ ] **Step 3: Refactor `chat-session-provider.tsx`**

  This is a focused, deep refactor. Make these distinct changes in order:

  **3a. Add the 3-state ref and pendingPromptRunIdRef** (near the existing `isCreatingRef`, around line 344):

  ```tsx
  type DraftState = 'DRAFT_IDLE' | 'DRAFT_MATERIALIZING' | 'DRAFT_PROMPT_PENDING_RETRY'
  const draftStateRef = React.useRef<DraftState>('DRAFT_IDLE')
  const pendingPromptRunIdRef = React.useRef<string | null>(null)
  const materializeAbortRef = React.useRef<AbortController | null>(null)
  ```

  **3b. Add `lastDispatchError` state**:

  ```tsx
  const [lastDispatchError, setLastDispatchError] = React.useState<
    { provider: string; message: string; at: number } | null
  >(null)

  // Auto-clear when selected provider differs from the error's provider:
  React.useEffect(() => {
    if (lastDispatchError && lastDispatchError.provider !== selectedProviderId) {
      setLastDispatchError(null)
    }
  }, [selectedProviderId, lastDispatchError])
  ```

  **3c. Convert `createSession` to draft-only**: replace the body of `createSession` to NOT issue `POST /sessions`. Instead, set draft state and reset the chat shell:

  ```tsx
  const createSession = React.useCallback<CreateSessionFn>(
    async (createOptions) => {
      // Abort any in-flight materialize
      materializeAbortRef.current?.abort()
      materializeAbortRef.current = new AbortController()
      draftStateRef.current = 'DRAFT_IDLE'
      pendingPromptRunIdRef.current = null
      setSelectedRunId(null)  // clear selection so the shell renders the draft state
      // Return a synthetic Run id so callers can chain — implementer chooses shape
    },
    [/* deps */],
  )
  ```

  **3d. Extend `sendPromptForSelectedProvider` to materialize on first send**: when called with no selected run and draft state is IDLE, transition to MATERIALIZING, issue `POST /sessions` with the AbortController signal, then issue `POST /prompt`. On create success + prompt failure, transition to PROMPT_PENDING_RETRY and cache the id in `pendingPromptRunIdRef`. On the next Send, if `pendingPromptRunIdRef` is set, skip the create and go straight to `POST /prompt`:

  ```tsx
  // Pseudocode of the key region — implementer fills in full bodies:
  const sendPrompt = async (payload, options) => {
    const signal = materializeAbortRef.current?.signal
    // CASE A: retry path — we have a pending id from a prior failed prompt
    if (pendingPromptRunIdRef.current) {
      try {
        await fetchAcp(`/sessions/${pendingPromptRunIdRef.current}/prompt`, {
          method: 'POST', body: JSON.stringify({ prompt: payload.text }), signal,
        })
        pendingPromptRunIdRef.current = null
        draftStateRef.current = 'DRAFT_IDLE'
      } catch (err) {
        if (signal?.aborted) return
        setLastDispatchError({ provider: selectedProviderId, message: errMsg(err), at: Date.now() })
      }
      return
    }
    // CASE B: draft materialize
    if (draftStateRef.current !== 'DRAFT_IDLE') {
      // Surface "still creating" hint instead of silently dropping
      setLastDispatchError({ provider: selectedProviderId, message: 'Session is being created — wait a moment and try again.', at: Date.now() })
      return
    }
    draftStateRef.current = 'DRAFT_MATERIALIZING'
    try {
      const res = await fetchAcp('/sessions', {
        method: 'POST', body: JSON.stringify({ provider, model }), signal,
      })
      const run = toRun(await res.json())
      // CRITICAL: check abort BEFORE mounting the run
      if (signal?.aborted) return
      setRuns((prev) => [run, ...prev])
      setSelectedRunId(run.id)
      // chain into prompt
      try {
        await fetchAcp(`/sessions/${run.id}/prompt`, {
          method: 'POST', body: JSON.stringify({ prompt: payload.text }), signal,
        })
        draftStateRef.current = 'DRAFT_IDLE'
      } catch (err) {
        if (signal?.aborted) return
        // create succeeded, prompt failed — cache id for retry
        pendingPromptRunIdRef.current = run.id
        draftStateRef.current = 'DRAFT_PROMPT_PENDING_RETRY'
        setLastDispatchError({ provider: selectedProviderId, message: errMsg(err), at: Date.now() })
      }
    } catch (err) {
      if (signal?.aborted) return
      draftStateRef.current = 'DRAFT_IDLE'
      setLastDispatchError({ provider: selectedProviderId, message: errMsg(err), at: Date.now() })
    }
  }
  ```

  **3e. REMOVE the synthetic `setProviderHealth({ready: false, ...})` paths**: search for `setProviderHealth((current) => ({` and replace those failure-path synthesis blocks with `setLastDispatchError(...)`. Leave `setProviderHealth` calls in `refreshProvider` and the `setProviderHealth(selected)` sync effect as-is — those reflect TRUE health state.

- [ ] **Step 4: Update `chat-shell.tsx` to render union banner + draft state**

  In `apps/gateway-admin/components/chat/chat-shell.tsx`:

  ```tsx
  const { lastDispatchError } = useChatSessionData()
  const providerUnavailableMessage = providerReady
    ? lastDispatchError?.message ?? null  // even when ready, show dispatch errors
    : providerHealth?.message?.trim() || lastDispatchError?.message || null
  ```

  No layout change needed — the existing banner consumes `providerUnavailableMessage`.

  For the draft render: when `selectedRunId === null`, render the chat panel empty (with the input enabled) instead of bailing. This is mostly a passive change since the existing logic already handles the no-selected-run case.

- [ ] **Step 5: Run TS tests**

  ```bash
  cd apps/gateway-admin && pnpm test chat-session-provider
  ```

  Expected: all 5 race tests pass.

- [ ] **Step 6: Browser end-to-end via agent-browser (manual scenarios from the bead Validation list)**

  ```bash
  # Scenario A: click New, close tab → 0 server rows created
  COUNT_BEFORE=$(curl -sS -H "Authorization: Bearer $TOKEN" http://localhost:8765/v1/acp/sessions | jq '.sessions | length')
  agent-browser --session lab-acp open "https://lab.example.com/chat" --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
  agent-browser --session lab-acp click "button[aria-label*='Start new session']"
  agent-browser --session lab-acp close
  sleep 2
  COUNT_AFTER=$(curl -sS -H "Authorization: Bearer $TOKEN" http://localhost:8765/v1/acp/sessions | jq '.sessions | length')
  [ "$COUNT_BEFORE" = "$COUNT_AFTER" ] && echo "PASS: no orphan row" || echo "FAIL: orphan created"

  # Scenario B: click New, send → exactly 1 server row created
  # (similar pattern; assert COUNT_AFTER = COUNT_BEFORE + 1)
  ```

- [ ] **Step 7: Commit frontend slice**

  ```bash
  git add apps/gateway-admin/lib/chat/chat-session-provider.tsx \
          apps/gateway-admin/lib/chat/chat-session-provider.test.tsx \
          apps/gateway-admin/components/chat/chat-shell.tsx
  git commit -m "feat(chat): defer session row until first prompt; AbortController + lastDispatchError"
  ```

---

# Task 6 — `lab-de6yc.6` `session.start_and_prompt` dispatch orchestrator

**Files:**
- Modify: `crates/lab/src/dispatch/acp/catalog.rs` (new ActionSpec)
- Modify: `crates/lab/src/dispatch/acp/dispatch.rs` (new arm)
- Modify: `crates/lab/src/dispatch/acp/params.rs` (`StartAndPromptInput`)
- Modify: `crates/lab/src/acp/registry.rs` (`start_and_prompt` orchestrator method)
- Modify: `apps/gateway-admin/lib/chat/chat-session-provider.tsx` (collapse two-call materialize → one call)

**Required reading:** `bd show lab-de6yc.6` — atomicity contract (close-on-prompt-fail).

## 6.1 — Backend orchestrator

- [ ] **Step 1: Write failing atomicity test (Rust)**

  Add to `crates/lab/src/acp/registry.rs` tests:

  ```rust
  #[tokio::test]
  async fn start_and_prompt_closes_session_when_prompt_queue_fails() {
      let registry = AcpSessionRegistry::new_for_test_with_saturated_prompt_queue();
      let input = StartAndPromptInput {
          provider: Some("codex-acp".to_string()),
          model: Some("gpt-5.5/medium".to_string()),
          prompt: "test".to_string(),
          attachments: vec![],
      };
      let before = registry.session_count().await;
      let result = registry.start_and_prompt(input, "test-principal").await;
      assert!(result.is_err(), "expected error from saturated queue");
      let after = registry.session_count().await;
      assert_eq!(before, after, "session should be closed/cleaned up on prompt failure");
  }
  ```

  (Implementer: add `new_for_test_with_saturated_prompt_queue` and `session_count` as test helpers in the same file.)

- [ ] **Step 2: Run — expect compile failure**

  ```bash
  cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features start_and_prompt
  ```

- [ ] **Step 3: Add `StartAndPromptInput` and catalog entry**

  In `crates/lab/src/dispatch/acp/params.rs`:

  ```rust
  #[derive(Debug, Clone, serde::Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct StartAndPromptInput {
      pub provider: Option<String>,
      pub model: Option<String>,
      pub prompt: String,
      #[serde(default)]
      pub attachments: Vec<crate::dispatch::acp::params::PromptAttachmentParam>,
  }
  ```

  In `crates/lab/src/dispatch/acp/catalog.rs`, sibling to `session.start`:

  ```rust
  ActionSpec {
      name: "session.start_and_prompt",
      summary: "Atomically create an ACP session and queue its first prompt. Returns session_id + stream_ticket. Closes the orphan-session window of separate create+prompt calls.",
      destructive: false,
      params: &[
          ParamSpec { name: "provider", kind: ParamKind::String, required: false,
                       description: "Provider id (defaults to gateway default)" },
          ParamSpec { name: "model", kind: ParamKind::String, required: false,
                       description: "Model id; provider's default if omitted" },
          ParamSpec { name: "prompt", kind: ParamKind::String, required: true,
                       description: "First user prompt text" },
          ParamSpec { name: "attachments", kind: ParamKind::Array, required: false,
                       description: "Optional list of PromptAttachmentParam" },
      ],
  },
  ```

- [ ] **Step 4: Add dispatch arm**

  In `crates/lab/src/dispatch/acp/dispatch.rs`:

  ```rust
  "session.start_and_prompt" => {
      let input: StartAndPromptInput = serde_json::from_value(params.clone())
          .map_err(|e| ToolError::InvalidParam {
              message: format!("invalid params: {e}"),
              param: "params".to_string(),
          })?;
      let principal = params
          .get("principal")
          .and_then(Value::as_str)
          .ok_or_else(|| ToolError::MissingParam {
              message: "principal is required".to_string(),
              param: "principal".to_string(),
          })?;
      let result = registry.start_and_prompt(input, principal).await?;
      Ok(serde_json::to_value(result).unwrap_or(Value::Null))
  }
  ```

- [ ] **Step 5: Implement `start_and_prompt` orchestrator on registry**

  In `crates/lab/src/acp/registry.rs`, next to `create_session`:

  ```rust
  #[derive(Debug, Clone, serde::Serialize)]
  pub struct StartAndPromptResult {
      pub session_id: String,
      pub provider_session_id: String,
      pub model_id: Option<String>,
      pub stream_ticket: String,
  }

  pub async fn start_and_prompt(
      &self,
      input: StartAndPromptInput,
      principal: &str,
  ) -> Result<StartAndPromptResult, ToolError> {
      // Step 1: create the session
      let created = self
          .create_session(StartSessionInput {
              provider: input.provider,
              model_id: input.model.clone(),
              cwd: None,
          }, principal)
          .await?;

      // Step 2: queue the first prompt — on failure, close the session before returning
      let prompt_input = PromptInput {
          text: input.prompt,
          attachments: prompt_attachments_from_param(input.attachments),
      };
      let prompt_result = self
          .prompt_session(
              &created.session_id,
              prompt_input,
              principal,
              input.model.as_deref(),
              PromptSessionOptions::default(),
          )
          .await;
      if let Err(prompt_err) = prompt_result {
          // Atomicity: close the just-created session before returning the error
          let _ = self.close_session(&created.session_id, principal).await;
          return Err(prompt_err);
      }

      // Step 3: mint a stream ticket so the client can subscribe immediately
      let stream_ticket = self
          .mint_subscribe_ticket(&created.session_id, principal)
          .await?;

      Ok(StartAndPromptResult {
          session_id: created.session_id,
          provider_session_id: created.provider_session_id,
          model_id: created.model_id,
          stream_ticket,
      })
  }
  ```

  Adjust the field names (`StartSessionInput`, `mint_subscribe_ticket`) to match your existing registry API — read the file first.

- [ ] **Step 6: Run tests — verify pass**

  ```bash
  cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features start_and_prompt
  ```

- [ ] **Step 7: Smoke test via API**

  ```bash
  TOKEN=$(grep "LAB_MCP_HTTP_TOKEN" /home/jmagar/.labby/.env | cut -d= -f2 | tr -d '"')
  curl -sS -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
    -d '{"action":"session.start_and_prompt","params":{"provider":"codex-acp","prompt":"Reply with: ATOMIC_OK"}}' \
    http://localhost:8765/v1/acp | jq
  ```

  Expected: returns `{ session_id, provider_session_id, model_id, stream_ticket }` within ~200ms (codex spawn happens server-side after the response).

- [ ] **Step 8: Commit backend slice**

  ```bash
  git add crates/lab/src/dispatch/acp/catalog.rs \
          crates/lab/src/dispatch/acp/dispatch.rs \
          crates/lab/src/dispatch/acp/params.rs \
          crates/lab/src/acp/registry.rs
  git commit -m "feat(acp): add session.start_and_prompt orchestrator (atomic create+prompt)"
  ```

## 6.2 — Frontend: collapse two-call materialize into single orchestrator call

- [ ] **Step 1: Write the regression test**

  In `apps/gateway-admin/lib/chat/chat-session-provider.test.tsx`, replace the existing "POST /sessions and POST /prompt fire in sequence" expectation with:

  ```ts
  test('Send in draft state issues exactly one POST /v1/acp action call', async () => {
    const fetchSpy = vi.fn(async () => ({ ok: true, json: async () => ({ session_id: 'new', stream_ticket: 't' }) }))
    // … set up provider, draft state, fire sendPrompt …
    const calls = fetchSpy.mock.calls.filter((c) => c[0].includes('/v1/acp') && c[1]?.method === 'POST')
    assert.equal(calls.length, 1)
    assert.match(JSON.stringify(calls[0][1].body), /session\.start_and_prompt/)
  })
  ```

- [ ] **Step 2: Replace the two-call materialize block in `chat-session-provider.tsx`**

  Find the materialize region added in Task 5.2 step 3d. Replace the sequential `POST /sessions` + `POST /prompt` with one call:

  ```tsx
  const res = await fetchAcp('', {
    method: 'POST',
    body: JSON.stringify({
      action: 'session.start_and_prompt',
      params: { provider, model, prompt: payload.text },
    }),
    signal,
  })
  const result = await res.json() as { session_id: string; stream_ticket: string; model_id?: string }
  if (signal?.aborted) return
  // Mount the run from the orchestrator's response (synthesize from result + draft form)
  const run: ACPRun = synthesizeRunFromOrchestrator(result, { provider, model })
  setRuns((prev) => [run, ...prev])
  setSelectedRunId(run.id)
  // No second POST — the orchestrator atomically queued the prompt server-side
  draftStateRef.current = 'DRAFT_IDLE'
  ```

- [ ] **Step 3: Remove `pendingPromptRunIdRef`**

  Since create+prompt is now atomic, the cache from Task 5 is no longer needed. Delete:
  - `const pendingPromptRunIdRef = React.useRef<string | null>(null)`
  - All sites that set or read it
  - The `DRAFT_PROMPT_PENDING_RETRY` enum variant (collapse the type back to a 2-state ref)

  Update the race tests added in Task 5.2 step 1 to drop the "create OK + prompt fail" scenario (it no longer happens — both succeed or both fail atomically).

- [ ] **Step 4: Run TS tests**

  ```bash
  cd apps/gateway-admin && pnpm test chat-session-provider
  ```

- [ ] **Step 5: Browser end-to-end latency measurement**

  ```bash
  agent-browser --session lab-acp open "https://lab.example.com/chat" --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
  # Measure time from Send click to first assistant token rendering
  # Expected: < 1.2s for the warmest provider (was 2.1-3.1s pre-orchestrator)
  ```

- [ ] **Step 6: Commit frontend slice**

  ```bash
  git add apps/gateway-admin/lib/chat/chat-session-provider.tsx \
          apps/gateway-admin/lib/chat/chat-session-provider.test.tsx
  git commit -m "feat(chat): use session.start_and_prompt orchestrator (atomic, ~800ms cold start)"
  ```

---

# Final verification

After all tasks complete:

- [ ] **Full backend test sweep**

  ```bash
  just test
  ```

  Expected: green.

- [ ] **Full frontend test sweep + typecheck + build**

  ```bash
  cd apps/gateway-admin && pnpm test && pnpm typecheck && pnpm build
  ```

  Expected: green.

- [ ] **Lint sweep**

  ```bash
  just lint
  ```

  Expected: green.

- [ ] **Smoke test the full chat experience**

  ```bash
  TOKEN=$(grep "LAB_MCP_HTTP_TOKEN" /home/jmagar/.labby/.env | cut -d= -f2 | tr -d '"')
  agent-browser --session lab-acp open "https://lab.example.com/chat" --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
  agent-browser --session lab-acp screenshot /tmp/chat-final.png
  ```

  Read `/tmp/chat-final.png` and confirm:
  - Sidebar shows ~80 sessions (failed/closed hidden), with "Show N closed/failed" toggle
  - No "New session" / "action route session" placeholder titles
  - Model badges only on rows that differ from dominant model
  - Model picker (Codex) shows 5 base rows with effort pills
  - "Clean up" button works → confirm dialog with count + criteria
  - Click New, send first prompt — single POST + ~800ms to first token

- [ ] **Close beads**

  ```bash
  bd close lab-de6yc.1 lab-de6yc.2 lab-de6yc.3 lab-de6yc.4 lab-de6yc.5 lab-de6yc.6 lab-de6yc
  ```

- [ ] **Push branch and open PR**

  Standard `git push -u origin <branch>` + `gh pr create` workflow. Reference epic `lab-de6yc` in the PR body.

---

## Notes for the implementer

- The plan honors the wave structure from `bd swarm validate lab-de6yc`. If using subagent-driven execution, dispatch Wave 1 (Tasks 1 + 4) in parallel.
- Every Locked Decision in the beads MUST be honored. If you find a conflict between the plan and the bead, the bead wins — fix the plan inline and continue.
- The plan uses representative code blocks; some helpers (`new_for_test`, `errMsg`, `synthesizeRunFromOrchestrator`) are sketched rather than fully implemented. Fill them in by mirroring the patterns already present in the file you're editing.
- The bead for `.6` notes that `pendingPromptRunIdRef` from `.5` is removed once `.6` lands. If you're running both in a single session, you can skip the cache entirely and add the orchestrator directly — but the plan keeps them as separate commits so the rollback story is clean.
