# Beads Coverage

Status: Dolt SQL contract implemented. Read-only.

Actions: `contract.status`, `health.status`, `version.get`, `project.list`, `context.get`, `status.summary`, `issue.list`, `issue.ready`, `issue.show`, `graph.show`, plus built-in `help` and `schema`.

Implemented now: full Lab service wiring (SDK, dispatch, CLI, MCP catalog, HTTP API, TUI metadata, onboarding audit) backed by `mysql_async` against the configured Dolt SQL server. Each database on the server is treated as one Beads project; the dispatcher accepts an optional `project` param on every issue/graph action and falls back to `BEADS_DEFAULT_PROJECT`.

Frontend: `/beads` admin page renders the project picker (driven by `SHOW DATABASES`), the status-summary strip, the ready/all issue toggle, and an issue detail drawer with dependencies, labels, and comments.

Deferred: issue/comment/dependency writes, close/reopen/update/create, raw SQL, Dolt push/pull/commit, branch operations, import/export.

Security: v1 issues only `SELECT` against the configured projection. Project identifiers are validated against `[A-Za-z0-9_-]+` before backtick interpolation; status filters accept any shape-checked string (no whitespace or control characters, length-bounded) so user-defined values from `custom_statuses` work, and the value travels as a bound MySQL parameter rather than being string-interpolated. Credentials live in `~/.lab/.env` (writable from the **Settings → Services → Beads** web form) and never leave `lab-apis` — Beads consumes them via the shared `lab_apis::core::Auth` enum, whose `Debug` impl already redacts secrets.
