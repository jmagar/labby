# Configuration Reference

Config lives in `~/.lab/.env`. Loaded at startup by `crates/lab/src/config.rs`.

Runtime gateway settings live in `config.toml`; verify exact fields against
`crates/lab/src/config.rs` before editing.

## Env Var Naming Convention

```
LAB_MCP_HTTP_TOKEN            # static bearer token for Labby HTTP/MCP
LAB_GW_<NAME>_AUTH_HEADER     # auth header for one gateway upstream
LAB_ACP_DB                    # ACP database path
LAB_ACP_HMAC_SECRET           # ACP permission-signing key
```

Use `docs/generated/env-reference.md` for current Labby-owned env vars. For
gateway auth, prefer `gateway.add`/`gateway.update` with `bearer_token_env` and
let Labby derive `LAB_GW_<NAME>_AUTH_HEADER` when possible.

## Logging

```
LAB_LOG=labby=info,lab_apis=warn    # tracing filter directive (default)
LAB_LOG_FORMAT=json               # emit newline-delimited JSON (for prod/CI)
```

## Code Mode Config

Root `[code_mode]` controls Code Mode limits:

```toml
[code_mode]
enabled = true
trace_params = true
result_shape_policy = "off"      # off | truncate
timeout_ms = 30000
max_response_bytes = 24576
max_response_tokens = 6000
token_estimate_divisor = 4
max_log_entries = 1000
max_log_bytes = 65536
```

`gateway.code_mode.set` accepts these public fields. `result_shape_policy =
"truncate"` shapes only successful completed final `result` values for
model-facing output. It does not affect sandbox-visible `callTool()` results,
does not retain raw results, and is not redaction.

## Config Mutation

Use setup and gateway actions instead of direct `.env` edits when possible:

```json
{ "action": "gateway.service_config.get", "params": { "service": "deploy" } }
{ "action": "gateway.service_config.set", "params": { "service": "deploy", "values": {} } }
```

For upstream MCP servers, use `labby gateway add`, `labby gateway update`,
`labby gateway discover`, `labby gateway import`, and `labby gateway reload`.
For operational gateway examples, read `gateway-operations.md`.
