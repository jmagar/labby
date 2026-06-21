# Monitors

This document covers the `labby deploy monitor` CLI command (a lab-owned host-probe monitor). Plugin-declared Claude Code monitors — the `monitors.json` manifest mechanism described below — now ship from plugins in the [dendrite marketplace repo](https://github.com/jmagar/dendrite), not from this repo.

## What Monitors Are

The Claude Code Monitor tool runs a long-lived shell command and turns each newline on stdout into an in-session event. Plugins declare their monitors in a manifest so users can enable them without copying commands by hand.

Plugin manifests reference monitor files from each plugin's `.claude-plugin/plugin.json`:

```json
"monitors": "./monitors/monitors.json"
```

## Manifest Schema

`plugins/<name>/monitors/monitors.json` is a JSON array. Each entry has three required fields:

| Field | Meaning |
|-------|---------|
| `name` | Stable identifier for the monitor. |
| `command` | Shell command to run. May reference `${user_config.<key>}` placeholders defined in `plugin.json`. |
| `description` | One-line summary shown in the Monitor tool details panel. |

User-configurable placeholders are declared under `userConfig` in `plugin.json` and substituted by Claude Code at launch.

## Registered Monitor Files

The plugins that ship monitor manifests live in the [dendrite marketplace repo](https://github.com/jmagar/dendrite), not in this repo. From a dendrite checkout, run:

```bash
jq '.[] | {name, command, description}' plugins/*/monitors/monitors.json
```

for the current registered monitor names and command payloads.

## `labby deploy monitor` Command

```
labby deploy monitor <targets...> [--interval SECS] [--timeout SECS]
```

| Flag | Default | Meaning |
|------|---------|---------|
| `<targets...>` | required | SSH aliases, must exist in deploy config |
| `--interval` | `30` | Poll period between probes |
| `--timeout` | `3` | TCP connect timeout per probe |

### Probe model

Each tick attempts a TCP connect to `<host>:<port>` (defaulting to port `22`). A connect within `--timeout` is `online`; a refused, reset, or timed-out connect is `offline`. The probe does not run an SSH handshake — only the TCP layer is checked.

### Output

One newline-delimited JSON object per emit, written to stdout:

```jsonc
{ "ts": 1777176765, "host": "tootie", "status": "online", "addr": "tootie:29229" }
```

| Field | Meaning |
|-------|---------|
| `ts` | Unix epoch seconds |
| `host` | The SSH alias from deploy config |
| `status` | `online` or `offline` |
| `addr` | The `host:port` actually probed |

### When events are emitted

- **Startup snapshot.** One event per target on launch, regardless of state. Confirms the monitor is alive and reflects the current world.
- **Steady state.** One event per host per real `online ↔ offline` transition. Hosts that stay online (or stay offline) are silent.

This means a healthy fleet generates exactly one burst of events at startup and nothing afterward. A flapping host is the only normal source of ongoing traffic.

### Single-instance lock

`labby deploy monitor` writes its PID to a lock file before entering the watch loop:

```
~/.lab/run/deploy-monitor.lock
```

Behavior:

- **Fresh start.** Creates `~/.lab/run/` if needed, writes the current PID, runs.
- **Lock exists, PID alive.** Refuses to start with a structured error pointing at the live PID and lock path. Exits non-zero.
- **Lock exists, PID dead (stale).** Silently overwrites the lock and runs.
- **Clean exit (Ctrl-C / SIGINT).** Removes the lock file via RAII drop.
- **Hard kill (SIGKILL or process crash).** Lock file lingers; the next launch detects the dead PID and recovers automatically.

Liveness is checked with `nix::sys::signal::kill(pid, None)` (Unix only — sends signal 0, which probes existence without delivering anything).

### Manual recovery

If you need to clear a lock by hand:

```bash
rm ~/.lab/run/deploy-monitor.lock
```

Only do this if `ps -p <pid>` confirms the named process is gone. The stale-PID path handles dead processes automatically; manual deletion is only needed if the lock holder changed identity (e.g. PID was reused by an unrelated process).

## Operator Recipes

### Verify the monitor is running

```bash
ps -ef | grep "labby deploy monitor" | grep -v grep
cat ~/.lab/run/deploy-monitor.lock
```

The PID in the lock file should match exactly one running `labby deploy monitor` process.

### Stop the monitor

```bash
kill -INT $(cat ~/.lab/run/deploy-monitor.lock)
```

SIGINT triggers the clean exit path and removes the lock file. SIGTERM works too, but the lock will linger until the next launch overwrites it.

## Adding a New Monitor

Plugin monitor manifests are authored in the [dendrite marketplace repo](https://github.com/jmagar/dendrite); steps 1–2 below refer to paths in that repo.

1. Append an entry to the owning plugin's `plugins/<name>/monitors/monitors.json`.
2. If the command reads runtime config, declare any new `${user_config.*}` keys under `userConfig` in `plugins/<name>/.claude-plugin/plugin.json`.
3. Long-running monitors that should be singletons must implement their own pidfile lock — there is no shared lock helper, but `crates/lab/src/dispatch/deploy/monitor.rs::LockGuard` is a small, copyable reference.
4. Filter aggressively before printing. Each stdout line becomes a Monitor tool event; chatty commands generate noise. Prefer transition-only emit with a single startup snapshot, as `deploy-host-monitor` does.
