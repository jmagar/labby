# web-app-testing

Live end-to-end testing of a **web app** in a real browser, producing a works/doesn't-work + UI/UX
report. One of three sibling testing skills (`web-app-testing`, `android-app-testing`,
`desktop-app-testing`) that share a common report format so results compare across platforms.

## When to invoke
- "test my web app", "QA this site", "run E2E and tell me what breaks", "review the UX of this web
  app", "does the deployed site work".
- NOT for writing a single reusable automation script — that's `webwright`. This skill is a *test
  pass + report*, not a one-shot task.

## How it works
Connects Playwright to an existing CDP Chrome (default `http://127.0.0.1:9222`), enumerates features
from the ARIA/DOM tree, exercises each, captures screenshots + console/page/network errors, and
writes a structured report. Built on the Webwright code-as-action contract (plan → instrumented
script → run dir → self-verify) with testing-specific instrumentation added.

## Files
- `SKILL.md` — workflow, prerequisites, failure taxonomy.
- `scripts/webtest.py` — the `WebTest` driver (connect-over-CDP, evidence capture, resilient click).
  Self-tested live; CLI smoke mode: `python webtest.py <run_dir> [url]`.
- `references/report-format.md` — shared cross-platform report spec, run-dir layout, verdict words.

## Prerequisites
- A Chrome with `--remote-debugging-port` open (default 9222; override `WEBTEST_CDP`).
- Playwright venv: `uv venv /tmp/pw_venv --python 3.12 && uv pip install --python
  /tmp/pw_venv/bin/python "playwright>=1.59,<1.60"`.

## Companion skills
- `webwright` / `webwright:run` / `webwright:craft` — single-task web automation + reusable scripts.
- `android-app-testing`, `desktop-app-testing` — same testing job, native targets, same report.
- `chrome`, `agent-browser` — alternate ways to obtain/drive a browser session if no CDP is up.
