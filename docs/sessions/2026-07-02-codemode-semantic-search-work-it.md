---
date: 2026-07-02 05:05:00 EST
repo: git@github.com:jmagar/labby.git
branch: feat/codemode-semantic-search
head: 9de5e121
plan: docs/superpowers/plans/2026-07-02-codemode-semantic-search.md
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab (primary checkout, parked on this branch for this plan)
pr: "#172 feat(codemode): semantic search blend for codemode.search() — https://github.com/jmagar/labby/pull/172"
beads: lab-aq646, lab-7k11y
---

# Work-it session: codemode semantic search (PR #172)

## User Request

"There should be a plan file that was committed with PR 172 — find the worktree and the plan file and then /work-it," reusing the checkout dedicated to that plan (no new worktree). Second track of a dual work-it session (first track: PR #171 base-service gating, logged separately in `docs/sessions/2026-07-02-gate-base-services-work-it.md` on `feat/gate-base-services`).

## Session Overview

Executed the committed, engineering-reviewed plan to blend TEI-embedding semantic similarity into `codemode.search()` — fail-open everywhere, zero new sandbox protocol surface (reserved `__lab_internal::semantic_rank` id over the existing `callTool` wire), Rust-owned vector math, fingerprint-keyed embedding cache with 30s cooldown. All 7 plan tasks landed (8 commits, bead `lab-aq646`), verified by live TEI smoke tests, then hardened through an adversarial review wave whose security findings (unmetered internal-call amplification and unbounded request/response sizes) were fixed in one batch (bead `lab-7k11y`). Two one-line CI lint fixes were applied by the coordinator (rustc `-D warnings` lints that local clippy misses).

## Sequence of Events

1. Located the branch (primary checkout, deliberately parked) and the plan; verified the pre-existing uncommitted `host.rs` diff matched the plan's Task 1 spec exactly; dispatched the implementation agent with `superpowers:executing-plans`.
2. Implementation: Tasks 1-7 as commits `fc862f99`, `3590914b`, `98247dc6`, `04650d7b`, `58aea024`, `78dce493`, `aedc0ead`+`408c4b8c`; live smoke matrix (real TEI @ :52000 + server-everything upstream): semantic synonym query works, unconfigured behavior byte-identical, dead-TEI fails open with exactly one warn.
3. Coordinator CI fix `27ad64d2` (`unused_qualifications`).
4. Review wave (3 agents): security-sentinel confirmed the scope invariant holds but found P1 unmetered `__lab_internal` TEI amplification + P2 unbounded query/response sizes; code-reviewer verified all seven plan contracts (fail-open layers, lexical-identical unconfigured path, lock ordering, blend math, budget gate) with nothing actionable; goal-verifier: PASS 7/7 with command evidence.
5. Security fix batch `28e55f70` (bead `lab-7k11y`): MAX_INTERNAL_CALLS_PER_RUN=32 with fail-open `{"ranked": []}` settlement + warn-once; 8 KiB query clamp on char boundary; Content-Length precheck + streamed 16 MiB response cap; shared `discovery_render_params`/`discovery_entry_visible` helpers exported from labby-codemode (formula divergence now impossible); `LazyLock` TEI client.
6. Delta review of the fix commit: nothing actionable — recommend merge. Coordinator CI fix `9de5e121` (`let_underscore_drop` in the new raw-TCP test).

## Key Findings

- The scope-security invariant survived adversarial review: embeddings are cached per catalog fingerprint, but the `kind == Snippet || scope.allows(...)` filter re-applies per call before ranking (`code_mode_host.rs:246-261`), so a shared cache can never leak out-of-scope ids.
- `codemode.search()` was already Promise-returning on main, so making it `async` changed nothing observable for JS callers.
- The plan's "size-capped before JSON decoding" constraint was not actually met by `response.bytes()` (full buffering precedes the check) — fixed with streaming accumulation.
- CI compiles tests with `RUSTFLAGS=-D warnings` (rustc lints), catching `unused_qualifications`/`let_underscore_drop` that `just lint` (clippy) misses — bit twice; captured as a persistent memory and worth adding to fix-agent verification lists.
- Shared `target/` between this checkout and the PR #171 worktree caused phantom compile errors and a foreign-binary smoke-test trap; `cargo clean -p <crates>` and pinning copied binaries are the workarounds.

## Technical Decisions

- Over-ceiling internal calls settle fail-open (never error the run) to preserve the plan's degradation contract; warn-once on first breach.
- Dedup direction: helpers exported FROM labby-codemode (gateway already depends on it), keeping the client-neutral crate free of gateway vocabulary.
- Implementer deviation on Task 5 Step 4 (mirroring `build_code_mode_proxy`'s formulas instead of the plan's literal `false, false` fallback) accepted — strengthens the invariant and makes cache warming effective; validated by goal-verifier against the plan's own flagged uncertainty.

## Files Changed

| status | path | purpose |
|---|---|---|
| modified | crates/labby-codemode/src/{host,execute,runner_drive,preamble,config,lib}.rs, pool/runner_handle.rs | trait method, internal-call bridge + ceiling, query clamp, blend JS, shared helpers |
| created | crates/labby-gateway/src/gateway/code_mode/embeddings.rs | TEI client (chunked, streamed-capped, LazyLock), cosine ranking |
| modified | crates/labby-gateway/src/gateway/{code_mode.rs, code_mode/{code_mode_host,search}.rs, config.rs, manager.rs, manager/{core,code_mode_runtime}.rs, manager/tests/code_mode.rs} | semantic_rank impl, embedding cache, cooldown, config validation, tests |
| modified | crates/labby-runtime/src/gateway_config.rs | SemanticSearchConfig (tei_url, blend_weight) + validation |
| modified | docs/runtime/CONFIG.md | [code_mode.semantic_search] section |

## Beads Activity

| id | title | action | status |
|---|---|---|---|
| lab-aq646 | codemode semantic search implementation | created, claimed, closed by impl agent | closed |
| lab-7k11y | security fix batch (internal-call cap, TEI bounds, formula dedup) | created, claimed, closed by fix agent | closed |

## Repository Maintenance

- Plans: completed plan left in place (`docs/superpowers/plans/`); moving completed plans is recorded as shared follow-up with the #171 track.
- Worktrees/branches: none touched; a foreign untracked plan file (`2026-07-02-codemode-wasmtime-dual-sandbox.md`, another session's output) left alone — its worktree `codemode-wasmtime-dual-sandbox` now exists and owns that work.
- Stale docs: CONFIG.md updated in-band; no other docs contradicted by this change.

## Tools and Skills Used

- Skills: work-it (coordinator), executing-plans (impl agent), save-to-md contract for this log.
- Agents: 1 implementation, 1 security-fix, 4 reviewers (security-sentinel, code-reviewer ×2, goal-verifier), all background-dispatched.
- Shell/git/gh/bd. Issues: shared-target contamination (workarounds above); two CI-only rustc lints; one Bash 2-minute timeout during an unrelated docs regen (split and re-run); CodeRabbit's automatic review errored on this PR (external tooling, non-blocking — internal waves substituted).

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `just lint` | clean | clean (after scoped cargo clean) | pass |
| `just test` (all-features) | all pass | 2357 pass / 13 skip | pass |
| `cargo test -p labby-codemode` / `-p labby-gateway` | pass | 173 / 458 pass | pass |
| `git diff main...HEAD --stat -- protocol.rs runner.rs` | empty | empty | pass |
| `rg reqwest crates/labby-codemode/src/` | empty | empty | pass |
| live smoke (semantic / control / fail-open) | per plan | all three PASS (PR body) | pass |
| PR #172 CI at log time | green | 33 pass / 1 pending (Incus image) / 0 fail | pending |

## Risks and Rollback

- New runtime surface is fail-open by contract; worst case of a TEI outage is lexical-only search plus one warn per transition. Rollback: revert the branch; with `tei_url` unset the feature is inert, so even a partial rollback leaves unconfigured deployments byte-identical to main.
- The 32-call internal ceiling is a hardcoded constant; if legitimate agents ever exceed it, searches silently degrade to lexical (visible via the warn log).

## Next Steps

1. Wait for the pending Incus-image CI job; run merge-status; merge PR #172 when green.
2. Post-merge operational step: set `[code_mode.semantic_search] tei_url` in the gateway's config to enable the feature (TEI already runs on dookie); unconfigured deployments are unaffected.
3. Shared follow-ups tracked on the #171 log: completed-plan moves, web-UI capability discovery, feature-table cleanup.
