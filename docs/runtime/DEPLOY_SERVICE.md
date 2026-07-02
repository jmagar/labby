# Deploy Service

> **Implementation status:** The `deploy` service is wired for CLI and MCP
> with the live SSH runner path. `config.list`, `plan`, `run`, `rollback`, and
> the built-in `help`/`schema` actions are available. The HTTP API surface is
> still deferred.

The `deploy` service is a synthetic service that pushes the local `lab`
release binary to one or more SSH targets with end-to-end integrity
verification, atomic install, and a timestamped backup of the previous
binary.

Deploy is feature-gated behind `deploy` and surfaced only on the **CLI** and
**MCP** transports in V1. The HTTP API surface is deferred.

## Authorization

Deploy actions require a dedicated token **above** the general MCP bearer.
Set `LAB_DEPLOY_TOKEN` in `~/.labby/.env` (or the process environment):

```
LAB_DEPLOY_TOKEN=<opaque-secret>
```

Every deploy action — `config.list`, `plan`, `run`, `rollback` — checks
this token first. Absent or empty values return `auth_failed`.

Destructive actions (`run`, `rollback`) additionally require live MCP
elicitation when called over MCP. A client that does not advertise
elicitation cannot bypass the confirm by setting `params.confirm = true`;
the dispatch layer rejects the request with `auth_failed`.

## Target inventory

Targets are resolved from `~/.ssh/config`. Each entry becomes an alias
usable in `targets: [...]`. `Include` and `Match` directives are ignored in
V1 with a WARN log; host aliases that depend on them will not deploy.

## Intent configuration

```toml
[deploy.defaults]
remote_path  = "/usr/local/bin/labby"
service      = "lab"
service_scope = "system"   # or "user"
max_parallel = 1
canary_hosts = ["mini1"]

[deploy.hosts.mini2]
remote_path  = "/opt/lab/bin/labby"
service      = "lab-worker"
service_scope = "user"
```

`remote_path` must fall under the allowlist `/usr/local/bin/` or
`/opt/lab/bin/`. Paths outside the allowlist are rejected with
`validation_failed`.

## Pipeline stages

1. **Build** — `cargo build --release --all-features -p labby`. The artifact
   is sha256-hashed locally.
2. **Preflight** — per host: `uname -m` must match the local build's target
   triple arch (`arch_mismatch` otherwise); the install directory's parent
   must be writable (canary `touch`+`rm` with an allowlist-validated path);
   if the remote binary already hashes to the local sha256, transfer is
   skipped.
3. **Transfer** — stream bytes to `<remote_path>.new.partial` via
   `ssh <host> "cat > …"`, then rename to `<remote_path>.new`.
4. **Install** — verify remote sha256 matches local; back up the current
   binary to `<remote_path>.bak.<timestamp>`; atomically rename
   `<remote_path>.new` over `<remote_path>`.
5. **Restart** — `systemctl restart <unit>` (or `systemctl --user restart
   <unit>` for `service_scope = "user"`), then `systemctl is-active --wait`.
6. **Verify** — run `<remote_path> --version`; non-zero exit is
   `verify_failed`.

## Concurrency & canary

V1 defaults to `max_parallel = 1`. Configured canary hosts deploy
sequentially first; if any canary fails with `--fail-fast`, subsequent
hosts are aborted. `--fail-fast` on non-canary hosts aborts the remaining
queue on first failure.

## Rollback

`deploy rollback <targets> -y` finds the most recent `<remote_path>.bak.*`
on each target and swaps it back into place. Rollback runs through the same
lock + preflight + atomic-swap path as `run`.

## Error kinds

See [docs/ERRORS.md](ERRORS.md#deploy-service-kinds) for the full table and
HTTP status mapping (deploy does not yet expose HTTP routes but the status
mapping is defined for when it does).

## Non-goals (V1)

- HTTP API surface (`/v1/deploy`).
- rsync transport.
- `[deploy.groups]` expansion.
- `deploy.verify` as a standalone action (verification is always part of
  `run`).
- Per-call policy overrides (`restart`, `backup`, `verify_service` as
  booleans on `run`).
- `Category::Operator` variant.

## Implementation status

Types, dispatch layer (catalog + params + authz + per-host lock + build
stage), CLI shim, MCP adapter, and the V1 orchestrator are wired. The
orchestrator runs the per-stage preflight/transfer/install/restart/verify
pipeline against the `HostIo` trait, with the production path using SSH I/O.

The CLI also exposes `labby deploy monitor` for monitor-oriented deployment
workflows.
