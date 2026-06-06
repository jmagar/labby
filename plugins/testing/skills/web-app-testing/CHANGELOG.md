# Changelog — web-app-testing

## 2026-06-06 — auth'd-backend prerequisite
- Added a prerequisite note for the case where you spin up the app's **own** dev server against a
  token-protected backend: front it with a bearer-token-injecting proxy (browser never holds the
  token, CORS moot) and make the client use relative API paths. Points at the in-process-vite +
  `playwright-core`→Edge recipe in `desktop-app-testing/references/ssh-fallback-capture.md`.

## 2026-05-29 — initial release
- Added — initial release. Live end-to-end web-app testing over CDP Chrome via Playwright.
- `scripts/webtest.py` — `WebTest` driver: connect-over-CDP, evidence capture (screenshots +
  console/page/network error listeners), `click_resilient` (role→text→selector fallback),
  screenshot settle + <2KB re-capture. Self-tested live against `127.0.0.1:9222` (status 200,
  title + 24.7KB screenshot).
- `references/report-format.md` — shared cross-platform report spec (verdict vocabulary, severity,
  run-dir layout, result.json shape), duplicated across the three sibling testing skills.
- Baked-in gotchas from live validation: `playwright.__version__` doesn't exist; ARIA-role clicks
  can time out on visible elements (hence the resilient fallback); fresh CDP targets can produce
  near-blank screenshots (hence settle + size check).
