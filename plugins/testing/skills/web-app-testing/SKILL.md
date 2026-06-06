---
name: web-app-testing
description: 'Use when the user wants to live-test a WEB app/site end-to-end in a real browser and get a works/doesn''t-work + UI/UX report — not just write Playwright code. Triggers: "test my web app", "QA this site", "run E2E on the web app", "click through every feature and tell me what breaks", "review the UX of this web app", "does my web app work", "test the deployed site". Drives a real Chrome via Playwright over CDP (default http://127.0.0.1:9222), enumerates features from the DOM/ARIA tree, exercises each, captures screenshots + console/page/network errors, and emits a structured report. Sibling of android-app-testing and desktop-app-testing (shared report format). Does NOT fire for: writing a one-off Playwright script (use webwright), pure web design/build with no testing, or backend/API-only testing.'
---

# web-app-testing

Live, end-to-end testing of a **web application** in a real browser: drive every feature, watch for
breakage (console errors, failed requests, broken flows), review UI/UX, and emit a structured
works/doesn't-work report. Companion to `android-app-testing` and `desktop-app-testing` — all three
share one report format (`references/report-format.md`) so results compare across platforms.

This builds on the **Webwright code-as-action contract** (plan → instrumented script in a run dir →
screenshots + self-verify) but adds what raw Webwright lacks for *testing*: feature enumeration,
console/network error instrumentation, a failure taxonomy, and the shared report.

## When to use vs. siblings / neighbors
- **This skill** — you want a *test pass + report* over a web app's features and UX.
- `webwright` / `webwright:run` — you want to automate ONE web task and get a reusable script. Use
  that for "log in and download the invoice"; use THIS for "test the whole app and report defects."
- `android-app-testing` / `desktop-app-testing` — same job, native targets.

## Prerequisites (verified 2026-05-29)
1. **A Chrome with CDP open.** Default endpoint `http://127.0.0.1:9222`. Confirm:
   `curl -s http://127.0.0.1:9222/json/version` → returns a `Browser` string. In this homelab a
   persistent headless Chrome runs there (axon-chrome). Override with `WEBTEST_CDP`.
   - No CDP available? Launch one: `chromium --headless --remote-debugging-port=9222` (or use the
     `chrome` / `agent-browser` skills to get a session), then point the driver at it.
2. **Playwright venv (one-time, no API keys):**
   ```bash
   uv venv /tmp/pw_venv --python 3.12
   uv pip install --python /tmp/pw_venv/bin/python "playwright>=1.59,<1.60"
   ```
   Gotcha: `playwright.__version__` does NOT exist — verify with
   `from playwright.sync_api import sync_playwright`.
3. **Spinning up the app's own dev server against an auth'd backend?** If you're starting the
   frontend yourself (not hitting a deployed URL) and it calls a token-protected backend, front the
   dev server with a proxy that **injects the bearer token** — the browser never holds it and CORS
   is moot — and make the app's client use **relative** API paths (not an absolute baseUrl) so they
   hit the proxy. For a vite app you can run it in-process and drive system Edge via `playwright-core`
   (no browser download). Recipe:
   `../desktop-app-testing/references/ssh-fallback-capture.md` (§ "Faster loop").

## The driver
`scripts/webtest.py` provides a `WebTest` class (connect-over-CDP, evidence capture, resilient
click) and a CLI smoke mode:
```bash
/tmp/pw_venv/bin/python scripts/webtest.py <run_dir> [url]   # smoke: load + title + screenshot
```
For a real test pass, write a per-app plan script that imports `WebTest` and drives the app's
features (see Workflow). The class bakes in the live-validated gotchas:
- **Screenshots settle + sanity-check size** (a fresh CDP target can yield near-blank PNGs; the
  driver re-captures if a shot is < 2KB).
- **`click_resilient(role=, name=, text=, selector=)`** falls back role → text → selector, because
  ARIA-role clicks can time out even on visible elements (live-confirmed: `get_by_role("link",
  name="More information")` timed out where text/CSS worked).

## Workflow

1. **Preflight.** Confirm CDP (`curl .../json/version`) and the venv import. Pick the target URL.
   Create the run dir: `~/.agents/docs/sessions/<app>-web-test/run_<id>/`.
2. **Map features.** `goto` the app, snapshot the **ARIA/DOM tree** (Playwright
   `page.accessibility.snapshot()` or `page.get_by_role(...)` enumeration), and list every nav item,
   button, form, link, tab. Merge with any user-supplied spec. This list is the test checklist —
   one row per feature in the report.
3. **Write `plan.md`** in the run dir: the feature checklist as Critical Points to verify.
4. **Exercise each feature** in an instrumented script (`final_script.py` importing `WebTest`):
   navigate/click/fill per feature, `shot(...)` after each, assert the expected post-state. Use
   `click_resilient` for anything flaky. Keep the console/page/network listeners on the whole run.
5. **Detect failures** — after each action check for: non-2xx `goto` status, thrown `pageerror`,
   `console` errors, `requestfailed`, an element that should appear/disappear and didn't, a flow
   that dead-ends. Classify PASS / PARTIAL / FAIL / BLOCKED.
6. **Self-verify** each Critical Point against its screenshot — `Read` the PNG and confirm the
   expected UI is actually there (don't trust that the script "ran"). Re-run in `run_<id+1>/` if
   you fixed the plan.
7. **UX/a11y pass** — score the rubric in the report format from the captured trees + screenshots
   (unlabelled ARIA nodes = an accessibility finding).
8. **Write the report** → `report.md` + `result.json` in the run dir, per
   `references/report-format.md`. Surface the screenshots as the evidence index.

## Failure taxonomy (web)
- **Crash/error** — uncaught `pageerror`, app shows an error boundary/500. → FAIL.
- **Broken nav/flow** — `goto`/click leads nowhere, 404/non-2xx, infinite spinner. → FAIL.
- **Silent breakage** — console errors or failed requests while the UI looks fine. → PARTIAL (note it).
- **Cosmetic/UX** — layout clip, missing feedback, slow response. → PARTIAL (S3/S4).
- **Can't reach** — needs login/data the run lacks. → BLOCKED (record what's needed).

## Evidence
Everything lands under the run dir (see `references/report-format.md` layout): `evidence/*.png`
(zero-padded, feature-named), `final_script_log.txt` (step log), `result.json` (machine-readable
verdicts), plus the console/page/network error arrays the driver records automatically.

## References
- `references/report-format.md` — the shared cross-platform report spec + run-dir layout + verdict
  vocabulary. Read it before writing the report.
