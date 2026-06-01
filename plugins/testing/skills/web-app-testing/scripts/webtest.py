#!/usr/bin/env python3
"""web-app-testing driver — Playwright over an existing CDP Chrome.

Reusable harness for the web-app-testing skill. Connects to a Chrome already
listening on the DevTools protocol (default http://127.0.0.1:9222), drives a
target URL, and captures evidence (screenshots + console/page/network errors)
into a run directory following the shared report-format contract.

This file provides building blocks; the agent writes a per-app test plan that
imports `WebTest` and calls its methods, OR runs this directly for a smoke
check:

    /tmp/pw_venv/bin/python webtest.py <run_dir> [url]

One-time venv setup (no API keys needed):
    uv venv /tmp/pw_venv --python 3.12
    uv pip install --python /tmp/pw_venv/bin/python "playwright>=1.59,<1.60"

Notes baked in from live validation (2026-05-29):
  - `playwright.__version__` does NOT exist; never version-check that way.
  - Screenshots over connect_over_cdp can come out near-blank on a fresh
    headless target — settle with wait_for_load_state + a short timeout, and
    sanity-check PNG byte size (warn under ~2KB).
  - ARIA-role locators (get_by_role) can time out even when the element is
    visible; `click_resilient` falls back to text, then CSS/XPath.
"""
from __future__ import annotations

import datetime
import json
import os
import sys

from playwright.sync_api import sync_playwright, TimeoutError as PWTimeout


class WebTest:
    def __init__(self, run_dir: str, cdp: str = "http://127.0.0.1:9222",
                 viewport=(1280, 1800)):
        self.run_dir = run_dir
        self.evidence = os.path.join(run_dir, "evidence")
        os.makedirs(self.evidence, exist_ok=True)
        self.cdp = cdp
        self.viewport = viewport
        self.log: list[str] = []
        self.console_errors: list[str] = []
        self.page_errors: list[str] = []
        self.failed_requests: list[str] = []
        self._pw = None
        self.browser = None
        self.ctx = None
        self.page = None

    # ── lifecycle ──────────────────────────────────────────────────────────
    def step(self, msg: str) -> None:
        line = f"[{datetime.datetime.now().strftime('%H:%M:%S')}] {msg}"
        self.log.append(line)
        print(line, flush=True)

    def start(self):
        self._pw = sync_playwright().start()
        self.step(f"connect_over_cdp {self.cdp}")
        self.browser = self._pw.chromium.connect_over_cdp(self.cdp)
        self.ctx = self.browser.contexts[0] if self.browser.contexts else self.browser.new_context()
        self.page = self.ctx.new_page()
        self.page.set_viewport_size({"width": self.viewport[0], "height": self.viewport[1]})
        self.page.on("console", lambda m: self.console_errors.append(m.text) if m.type == "error" else None)
        self.page.on("pageerror", lambda e: self.page_errors.append(str(e)))
        self.page.on("requestfailed",
                     lambda r: self.failed_requests.append(f"{r.method} {r.url} :: {r.failure}"))
        return self

    def finish(self, result: dict) -> dict:
        result.setdefault("console_errors", self.console_errors)
        result.setdefault("page_errors", self.page_errors)
        result.setdefault("failed_requests", self.failed_requests)
        with open(os.path.join(self.run_dir, "final_script_log.txt"), "w") as f:
            f.write("\n".join(self.log) + "\n")
        with open(os.path.join(self.run_dir, "result.json"), "w") as f:
            json.dump(result, f, indent=2)
        try:
            if self.page:
                self.page.close()
        finally:
            if self._pw:
                self._pw.stop()
        return result

    # ── primitives ─────────────────────────────────────────────────────────
    def goto(self, url: str, wait_until: str = "load", timeout: int = 30000):
        self.step(f"goto {url}")
        resp = self.page.goto(url, wait_until=wait_until, timeout=timeout)
        status = resp.status if resp else None
        self.step(f"  status={status}")
        return status

    def shot(self, name: str, settle_ms: int = 500) -> str:
        """Screenshot with a settle + byte-size sanity check (warns if near-blank)."""
        try:
            self.page.wait_for_load_state("load", timeout=10000)
        except PWTimeout:
            pass
        self.page.wait_for_timeout(settle_ms)
        path = os.path.join(self.evidence, name if name.endswith(".png") else f"{name}.png")
        self.page.screenshot(path=path)
        size = os.path.getsize(path)
        if size < 2048:
            self.step(f"  WARN screenshot {name} is {size}B (possibly blank) — re-capturing")
            self.page.wait_for_timeout(800)
            self.page.screenshot(path=path)
            size = os.path.getsize(path)
        self.step(f"  shot {name} ({size}B)")
        return path

    def text_of(self, selector: str) -> str:
        return self.page.locator(selector).first.inner_text()

    def click_resilient(self, *, role=None, name=None, text=None, selector=None,
                        timeout: int = 8000) -> str:
        """Click with fallbacks: role → text → selector. Returns which path worked.

        ARIA-role clicks can time out even on visible elements (live-validated),
        so we degrade rather than fail the whole run on the first strategy.
        """
        attempts = []
        if role and name:
            attempts.append(("role", lambda: self.page.get_by_role(role, name=name).click(timeout=timeout)))
        if text:
            attempts.append(("text", lambda: self.page.get_by_text(text, exact=False).first.click(timeout=timeout)))
        if selector:
            attempts.append(("selector", lambda: self.page.locator(selector).first.click(timeout=timeout)))
        last = None
        for label, fn in attempts:
            try:
                fn()
                self.step(f"  click via {label} ok")
                return label
            except Exception as e:  # noqa: BLE001 — try next strategy
                last = e
                self.step(f"  click via {label} failed: {str(e)[:80]}")
        raise RuntimeError(f"all click strategies failed; last={last}")


def _smoke(run_dir: str, url: str) -> dict:
    t = WebTest(run_dir).start()
    status = t.goto(url)
    title = t.page.title()
    t.step(f"title={title!r}")
    t.shot("cp01_load")
    result = {
        "loaded": status == 200, "status": status, "title": title,
    }
    t.step(f"FINAL_RESPONSE={json.dumps(result)}")
    return t.finish(result)


if __name__ == "__main__":
    rd = sys.argv[1] if len(sys.argv) > 1 else "."
    target = sys.argv[2] if len(sys.argv) > 2 else "https://example.com"
    os.makedirs(rd, exist_ok=True)
    out = _smoke(rd, target)
    print("RESULT", json.dumps(out))
