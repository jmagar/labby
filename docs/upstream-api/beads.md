# Beads Source Contract

Retrieved: 2026-05-07

Sources:
- https://gastownhall.github.io/beads/
- https://github.com/gastownhall/beads — `internal/storage/schema/migrations/`
- Dolt SQL server `SHOW DATABASES` / `DESCRIBE issues`

Beads is a git/Dolt-backed issue tracker. Lab v1 connects to Beads through the Dolt SQL server over the MySQL protocol (`mysql_async` in `lab-apis`) and treats each Dolt database on the server as one Beads project. All queries are read-only — Lab does not mutate issues, comments, dependencies, or Dolt commit/branch state.

## V1 Actions

| Action | Contract | Hosted posture |
|---|---|---|
| `contract.status` | returns the Lab/Beads integration contract | safe |
| `health.status` | reports Dolt reachability and the server version | network-only |
| `version.get` | runs `SELECT @@version` (and best-effort `dolt_version()`) | network-only |
| `project.list` | runs `SHOW DATABASES`, filtered to user-visible DBs | network-only |
| `context.get` | counts total / open issues for the requested project | network-only |
| `status.summary` | groups `issues.status` and returns counts | network-only |
| `issue.list` | reads `issues` with optional `status` filter | network-only |
| `issue.ready` | reads the `ready_issues` view (falls back to `status = 'open'`) | network-only |
| `issue.show` | reads one row plus joined `dependencies`, `labels`, and `comments` | network-only |
| `graph.show` | walks `dependencies` from a root issue (capped at 100 nodes) | network-only |

Every action accepts an optional `project: string` param. When omitted, the dispatcher falls back to `BEADS_DEFAULT_PROJECT`. Project identifiers are validated to `[A-Za-z0-9_-]+` before being interpolated as a backtick-quoted database name.

## Schema mapping

Lab queries map onto these tables and views from the Beads schema migrations:
- `issues` — primary entity; the projection limits Rust to a stable subset (id, title, description, status, priority, issue_type, assignee, owner, external_ref, lifecycle timestamps).
- `dependencies` — `issue_id`, `depends_on_id`, `type` (`'blocks'`, `'parent-child'`, …).
- `labels` — joined into `Issue.labels` for both list and detail views.
- `comments` — surfaced verbatim in the detail panel.
- `ready_issues` — used directly by `issue.ready`.

## Configuration

Set in `~/.lab/.env` (or via the **Settings → Services → Beads** form in the web UI, which writes the same keys):

- `BEADS_DOLT_URL` — required, MySQL connection URL (`mysql://host:3306/`).
- `BEADS_DOLT_USER`, `BEADS_DOLT_PASSWORD` — optional credentials, layered onto the URL at runtime.
- `BEADS_DEFAULT_PROJECT` — optional Dolt database name used when a request omits `project`.

## Deferred

Write operations remain deferred: create, update, close, reopen, comment, dependency mutation, raw SQL, Dolt push/pull/commit, branch operations, import/export, and direct schema-changing access.

## Security

- Every action is read-only; no `INSERT`/`UPDATE`/`DELETE`/`CALL` is issued.
- Project identifiers are validated before interpolation; status filters are matched against an allowlist.
- Comment text and issue descriptions are returned verbatim as plain JSON — surface code is responsible for HTML escaping when it renders.
- Credentials never leave `lab-apis`; `Auth` is constructed inside the dispatch client from env vars and passed straight to `mysql_async`.
