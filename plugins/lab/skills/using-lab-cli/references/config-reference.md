# Lab Config Reference

Use this file for operator reminders. For complete generated env metadata, use
`docs/generated/env-reference.md` and `docs/generated/env-reference.json`.

## Config Files

Common locations:

- `~/.lab/.env` for env-style runtime secrets and bootstrap values
- `~/.config/lab/config.toml` for structured runtime config, gateway upstreams,
  public URLs, node role, and tool-search settings
- repo `.env` only when running from a checkout that intentionally provides one

Prefer generated docs and live help before editing config:

```bash
labby docs check
labby doctor auth
labby doctor system
labby gateway public-urls
labby gateway tool-search status
```

## Safe Secret Handling

- Store raw bearer tokens and API keys in env files or a secret manager, not in
  `config.toml`.
- Gateway TOML should refer to token env var names with fields such as
  `bearer_token_env`.
- Do not echo raw token values into chat, logs, TOML, or command arguments that
  may be persisted in shell history.
- Prefer `labby setup plugin-sync` and `labby setup plugin-export` for
  plugin-owned `CLAUDE_PLUGIN_OPTION_*` to `LAB_*` mapping.

## Gateway Config

`labby gateway` mutates `[[upstream]]` entries in `~/.config/lab/config.toml`.
Typical HTTP gateway shape:

```toml
[[upstream]]
name = "github"
url = "https://github.example.com/mcp"
bearer_token_env = "GITHUB_MCP_TOKEN"
proxy_resources = false
```

Tool-search mode is gateway-wide, not per upstream. When enabled, MCP clients
discover tools through `scout` and execute them through `invoke`:

```toml
[tool_search]
enabled = true
top_k_default = 10
max_tools = 5000
```

Use the CLI instead of hand-editing when possible:

```bash
labby gateway add --name github --url https://github.example.com/mcp --bearer-token-env GITHUB_MCP_TOKEN
labby gateway tool-search enable --top-k-default 10 --max-tools 5000
labby gateway reload
```

## Extract Writes

`labby extract` writes `~/.lab/.env` only when `--apply` is passed:

```bash
labby extract ~/appdata --diff
labby extract ~/appdata --apply --yes
labby extract host:/mnt/user/appdata --apply --dry-run
```

Write rules:

- back up first
- write atomically
- preserve comments and ordering where possible
- keep existing conflicting values unless `--force` is used
- remain idempotent across repeated applies

Bare `labby extract` performs fleet discovery. Targeted `--diff` and `--apply`
require an explicit URI.

## Auth and Runtime Env

Common runtime vars:

```bash
LAB_MCP_HTTP_TOKEN=...
LAB_AUTH_MODE=bearer        # or oauth
LAB_PUBLIC_URL=https://lab.example.com
LAB_MCP_GATEWAY_URL=https://mcp.example.com
LAB_LOG=labby=info,lab_apis=warn
LAB_LOG_FORMAT=json
```

Use `labby doctor auth` after auth changes and `labby doctor proxy` when public
URLs or protected MCP routes change.
