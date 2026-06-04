---
name: mcporter
description: Use when the user mentions mcporter, says "test an MCP server", "smoke-test these tools", "automate MCP testing", "call a tool from the shell", "list MCP tools", "exercise the gateway tools", or asks for a script that hits MCP endpoints. Covers using mcporter to discover, inspect, and call MCP servers from the shell, and to write repeatable regression or smoke-test scripts. Not for designing new MCP servers, writing server-side handlers, or generic API testing unrelated to MCP.
---

## Context

- Argument: $ARGUMENTS
- CWD: !`pwd`
- mcporter: !`command -v mcporter >/dev/null && mcporter --version 2>/dev/null || echo "not installed (npm i -g mcporter)"`
- Configured servers: !`mcporter list --json 2>/dev/null | jq -r '.servers[] | "\(.status)\t\(.name)"' 2>/dev/null | head -20 || echo "none / mcporter unavailable"`

# mcporter — MCP CLI & test scripting

`mcporter` is a shell client for the Model Context Protocol. Use it to list servers, inspect tool schemas, call tools, and drive scripted regression tests over the wire — without writing a TypeScript client.

Auto-loads servers from `./config/mcporter.json` plus editor imports (Cursor, Claude Code, Codex, etc.), so most named servers in the user's environment already work without setup.

## Core verbs

```bash
mcporter list                          # all configured servers (status only)
mcporter list <server> --schema        # tool docs for one server
mcporter list <server> --schema --all-parameters --json
mcporter call <server>.<tool> k=v ...  # invoke a tool
mcporter call <server>.<tool> --args '{"k":"v"}'      # JSON payload
mcporter call <server>.<tool> --output json           # machine-readable
mcporter auth <server>                 # OAuth handshake only (no listing)
mcporter config doctor                 # validate all configs
```

Selectors are `server.tool`. The argument is `server` or a full `https://host/mcp` URL.

## Calling resources & prompts

`mcporter call` overloads on the second segment:

```bash
mcporter call <server>.<tool> k=v               # tool call
mcporter call <server>.<resource-uri>           # resource read (URI contains ://)
mcporter call <server> --tool <prompt-name> ... # prompt fetch
```

If the schema isn't obvious, run `mcporter list <server> --schema --all-parameters` first — guessing arg names against the wrong shape is the #1 wasted call.

## Argument forms (pick one, don't mix)

| Form | When |
|---|---|
| `key=value` / `key:value` | Flat scalar args, fast for shell use |
| `--args '{...}'` | Nested objects, arrays, anything with quoting pain |
| `'server.tool(key: "value", n: 1)'` | Function-call syntax when you want it self-documenting in a script |

`--output text\|markdown\|json\|raw` controls formatting; always use `json` for scripts.

## Ad-hoc servers (one-shot, no config edit)

```bash
mcporter list --http-url https://host/mcp --schema
mcporter call --stdio 'bun run ./server.ts' my_tool arg=1
mcporter list https://host/mcp                    # bare URL = HTTP
mcporter call --stdio 'node srv.js' --env API_KEY=$KEY tool arg=1
```

Persist a working ad-hoc definition with `--persist ./config/mcporter.json --yes`.

## Generating a standalone CLI

When the user wants a sharable, schema-validated wrapper rather than a shell script, prefer `mcporter generate-cli`:

```bash
mcporter generate-cli --server <name> --compile ./bin/<name>-cli
mcporter generate-cli --command 'npx -y @org/server' --name my-cli --compile ./bin/my-cli
```

The generated CLI bundles the tool schemas at build time, so every call is type-checked locally before it leaves the process. Inspect a generated binary with `mcporter inspect-cli ./bin/my-cli`.

## Writing a test harness

The goal: prove each tool returns the *right* thing for a known input — and catch breakage *before* sending the call when possible. Keep the script readable.

The template at `scripts/smoke.sh` (in this skill folder) does three things `mcporter` doesn't:

1. **Schema preflight** — pulls `inputSchema` once and rejects any case whose `args` are missing a required key. Catches typos *locally*, no network call.
2. **Robust error detection** — treats transport failure, wrapper warnings, MCP protocol errors (`MCP error -32xxx`), and tool-level `isError: true` envelopes as failures (each tagged differently in the report).
3. **String + regex assertions** — see below. Avoids depending on `mcporter call --output json`, which currently emits Node `util.inspect` format (not parseable JSON) on most servers.

### Case format

Each row in `CASES=()`:

```
"label|args|assertion"
```

| Field | Meaning |
|---|---|
| `label` | Tool name (`search`) or resource URI (`ui://server/status`) |
| `args` | Appended to `mcporter call`. `key=value` flat, `--args '{...}'` nested, empty for resources |
| `assertion` | One of the five forms below |

| Assertion form | What it checks |
|---|---|
| *(empty)* | Liveness — call must succeed, no error envelope |
| `contains: TEXT` | Response text must include `TEXT` |
| `regex: PATTERN` | Bash ERE matched against response text |
| `jq: FILTER` | Response text parsed as JSON, `jq -e FILTER` must be truthy |
| `error: KIND` | Expects an error envelope; passes if `.kind == KIND` or message contains `KIND` |

### Helper modes

```bash
./smoke.sh --list-tools <server>   # one tool name per line — pipe to grep
./smoke.sh --init <server>         # print skeleton CASES=() from the schema
                                   # required args are pre-filled with TODO
```

Typical flow: `./smoke.sh --init lab > cases.sh.fragment`, paste the relevant tools into `smoke.sh`, replace each `TODO` with a real value plus an assertion, run.

### Env flags

| Var | Effect |
|---|---|
| `TIMEOUT_MS=8000` | Per-call timeout (default 15000) |
| `VERBOSE=1` | Dump raw response on any failure |
| `NO_PREFLIGHT=1` | Skip schema preflight (e.g. to test the server's own validation) |

### Why not `--output json`?

`mcporter call --output json` currently emits Node `util.inspect` output (unquoted keys, single quotes, string concatenation with `+`) — verified against multiple servers. `jq` cannot parse it. The script uses `--output text` for assertions and `--output raw` only to inspect the envelope for `isError` and the error kind. If a tool's response *is* a JSON string, the `jq:` assertion form parses that string with jq.

### `set -e` traps to know

If you copy snippets out of `smoke.sh`, two patterns will bite you under `set -e`:

- `((counter++))` returns the *old* value of counter — when it was 0, exit code is 1 and the script dies. Use `((++counter))` or `counter=$((counter+1))`.
- `[[ test ]] && command` returns the failed test's exit when the test is false — fine inside `if`/`while`/`||` lists, fatal as a standalone statement. Use `if [[ test ]]; then command; fi`.

### Resources

Same loop — URI is the `label`, `args` is empty:

```bash
"ui://server/status||contains: ok"
"file://docs/readme.md||regex: ^# "
```

### Patterns worth keeping

## Common failure modes

| Symptom | Likely cause | Fix |
|---|---|---|
| `Unknown MCP server 'X'` | Name typo or import not picked up | `mcporter list` to see actual names; `mcporter config list --verbose` for sources |
| Tool call hangs ~30s then `tools unavailable` | Stdio binary missing (`ENOENT`) | Build / install the server binary; check the `transport` field in `--json` |
| `SSE error: Non-200 status code` | HTTP server down or auth expired | `mcporter auth <server> --reset` or check the URL |
| Args rejected with cryptic schema error | Flat `k=v` against a nested schema | Switch to `--args '{...}'` |
| `OAuth timeout` | Browser flow didn't complete in time | `--oauth-timeout 180000` and rerun `mcporter auth` |

## Quick reference

```bash
mcporter list --json | jq '.servers[] | {name, status}'      # health snapshot
mcporter list <s> --schema --json | jq '.tools[].name'       # tool names only
mcporter call <s>.<t> --args "$(cat payload.json)" --output json | jq .
mcporter generate-cli --server <s> --compile ./bin/<s>       # ship a binary
```

## What NOT to do

- Don't paste secrets as `key=value` on the command line — use `--env KEY=$VAR` for stdio servers, env-injection for HTTP.
- Don't write a Node/TS client when `mcporter call` + a shell loop will do.
- Don't rely on `--output json` for machine-parseable output in scripts — it emits Node util.inspect format, not valid JSON. Use `--output text` for assertions and `--output raw` for envelope inspection.
- Don't run smoke scripts against production data without checking which side-effects each tool has; mcporter is just a transport, it has no idea what's destructive.
- Don't commit `./config/mcporter.json` with personal tokens; use editor-imports or `--env`.
