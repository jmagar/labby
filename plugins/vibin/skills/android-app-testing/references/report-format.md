# App testing — shared report format

> Canonical report spec shared verbatim across `web-app-testing`, `android-app-testing`, and
> `desktop-app-testing` so reports are directly comparable across platforms. This file is
> duplicated into each skill's `references/` (workshop convention: duplicate the fact, don't
> cross-read another skill).

## Verdict vocabulary (use these exact words)

Per-feature result is exactly one of:

| Verdict | Meaning |
|---|---|
| **PASS** | Feature works as expected; exercised end-to-end with evidence. |
| **PARTIAL** | Feature works but with a defect (wrong output, missing feedback, slow, cosmetic break). |
| **FAIL** | Feature is broken — crash, hang, error dialog, or does nothing when it should act. |
| **BLOCKED** | Could not test (precondition unmet, dependency down, needs creds the run didn't have). |

Overall ship verdict is exactly one of: **SHIP** (no FAIL, no blocking PARTIAL) ·
**FIX-THEN-SHIP** (PARTIALs or non-blocking FAILs) · **BLOCKED** (couldn't complete core coverage).

## Severity (for each PARTIAL/FAIL finding)

`S1` crash/data-loss/blocker · `S2` major feature broken · `S3` minor/cosmetic · `S4` polish/nit.

## Report skeleton (markdown)

```markdown
# <App> <version> — <platform> test report — <YYYY-MM-DD> (run <id>)

## Summary
<X>/<Y> features PASS · <n> PARTIAL · <m> FAIL · <k> BLOCKED — **Verdict: <SHIP|FIX-THEN-SHIP|BLOCKED>**
One-paragraph narrative: what was tested, the headline problems, the single most important fix.

## Environment
- Target: <emulator-5554 / agent-os VM / CDP Chrome 134>
- Build: <path or URL>, transfer method, launch method
- Platform: <Android 15 / Windows 11 24H2 / web>
- Driver: <direct adb / agent-os_windows-mcp gateway / Playwright-over-CDP>
- Date/run-id, tester (agent), coverage note (what was NOT reached and why)

## Feature results
| # | Feature / flow | Steps | Verdict | Sev | Evidence | Notes |
|---|---|---|---|---|---|---|
| 1 | <name> | <1-line> | PASS | — | cp01.png | |
| 2 | <name> | <1-line> | FAIL | S2 | cp02.png, log#L40 | <what broke> |

## Failures & partials (detail)
For each non-PASS, in severity order:
### [S2] <feature> — FAIL
- Repro: numbered steps
- Expected vs actual
- Evidence: screenshot(s), log excerpt, element-tree snippet
- Hypothesis (optional)

## UX / a11y review
Score each (Good / Adequate / Poor) with a one-line justification + evidence:
- **Discoverability** — are features findable?
- **Feedback** — does every action produce visible confirmation?
- **Latency** — perceptible lag between action and response?
- **Layout** — clipping, overlap, off-screen, responsive issues?
- **Error messaging** — are errors clear and recoverable?
- **Accessibility** — labelled controls? (a11y-tree elements without names = a finding)

## Crashes / hangs / errors timeline
Chronological list with timestamps + the detection signal (process exit, ANR, event-log entry,
console error, failed request).

## Coverage
- Features enumerated: <N>; exercised: <M>; blocked: <K> (list each blocked + why).
- Explicitly out of scope this run: <…>

## Evidence index
Relative links into the run dir: every screenshot, UI-tree dump, log file, result.json.

## Recommendation
**<SHIP | FIX-THEN-SHIP | BLOCKED>** — the top 1–3 must-fix items, each linking its finding.
```

## Run directory layout (every platform)

```
<run-dir>/                         # e.g. ~/.agents/docs/sessions/<app>-test/run_<id>/
  report.md                        # the deliverable above
  result.json                      # machine-readable per-feature verdicts + counts
  evidence/
    cp01_<feature>.png             # screenshots, zero-padded, feature-named
    tree01_<feature>.{xml,json,txt}# UI/accessibility tree per step
    logs/<name>.log                # logcat / event-log / console captures
  meta.json                        # target, build, driver, timestamps, coverage
```

## result.json shape

```json
{
  "app": "<name>", "version": "<v>", "platform": "<web|android|desktop>",
  "run_id": "<id>", "date": "YYYY-MM-DD",
  "verdict": "FIX-THEN-SHIP",
  "counts": { "pass": 0, "partial": 0, "fail": 0, "blocked": 0 },
  "features": [
    { "id": 1, "name": "<f>", "verdict": "PASS", "severity": null,
      "evidence": ["evidence/cp01.png"], "notes": "" }
  ],
  "ux": { "discoverability": "Good", "feedback": "Poor", "latency": "Good",
          "layout": "Adequate", "error_messaging": "Poor", "accessibility": "Adequate" },
  "crashes": []
}
```

## Principles (all platforms)

- **Evidence before assertion.** Every PASS/FAIL cites a screenshot, tree dump, or log line. No
  verdict without an artifact.
- **Enumerate before exercising.** Derive the feature checklist from the live UI (accessibility
  tree / DOM / control tree) plus any user-supplied spec — don't test from memory.
- **Detect, don't assume.** After each action, actively check for the failure signal (crash, hang,
  error UI) — silence is not success.
- **No silent truncation.** If coverage was bounded (top-N, sampled, skipped), say so in Coverage.
- **Reset between independent features** to stop state leaking across tests.
