# Gateway Operations

Use this reference for upstream MCP gateway registration, runtime state, OAuth,
import/discovery, protected routes, and Code Mode enablement.

## Gateway Model

Labby is the operator gateway. External MCP servers are registered as upstream
gateways, then exposed through policy-filtered MCP resources/tools and the public
Code Mode `search`/`execute` tools.

Common CLI state checks:

```bash
labby gateway list --json
labby gateway get <name> --json
labby gateway mcp list --json
labby gateway public-urls --json
```

Common action-dispatch equivalents:

```json
{ "action": "gateway.list", "params": {} }
{ "action": "gateway.get", "params": { "name": "<name>" } }
{ "action": "gateway.mcp.list", "params": {} }
{ "action": "gateway.public_urls.get", "params": {} }
```

## Adding And Testing Upstreams

Test before saving when possible:

```bash
labby gateway test --name candidate --url https://example.invalid/mcp --json
labby gateway test --name candidate --command node --arg server.js --json
```

Add HTTP or stdio upstreams:

```bash
labby gateway add --name docs --url https://example.invalid/mcp --json
labby gateway add --name local-tool --command node --arg server.js --json
```

If bearer auth is needed, prefer an env-var reference:

```bash
labby gateway add \
  --name private-tool \
  --url https://example.invalid/mcp \
  --bearer-token-env LAB_GW_PRIVATE_TOOL_AUTH_HEADER \
  --json
```

`bearer_token_env` must be an environment variable name, not the raw token
value. Use `LAB_GW_<NAME>_AUTH_HEADER` style for gateway auth headers.

When a public HTTP/SSE upstream requires no auth, omit `bearer_token_env` and
OAuth config. Labby supports no-auth HTTP upstreams.

## Updating And Removing Upstreams

```bash
labby gateway update <name> --url https://new.example.invalid/mcp --json
labby gateway update <name> --bearer-token-env LAB_GW_NEW_AUTH_HEADER --json
labby gateway remove <name> -y --json
labby gateway reload --json
```

Use `gateway.reload` after direct config/env changes to reconcile runtime state.
Only reload promises to pick up changed bearer-token env values.

## Discovery And Import

Discovery scans local MCP client configs from known editors/tools:

```bash
labby gateway discover --json
labby gateway discover --clients claude,codex --json
```

Import is destructive because it mutates gateway config:

```bash
labby gateway import --name <server> -y --json
labby gateway import --all -y --json
```

Pending discovered servers can be reviewed and approved/rejected:

```bash
labby gateway pending list --json
labby gateway pending approve <name> -y --json
labby gateway pending reject <name> -y --json
```

Use `--dry-run` on pending approve/reject when available.

## Runtime MCP Lifecycle

Use `gateway mcp` for runtime lifecycle and process cleanup:

```bash
labby gateway mcp list --json
labby gateway mcp enable <name> --json
labby gateway mcp disable <name> --cleanup --json
labby gateway mcp cleanup <name> --dry-run --json
labby gateway mcp cleanup <name> --aggressive --json
```

The runtime list includes discovery counts and likely stale process counts. Use
cleanup when the config changed but old server processes remain.

## Upstream OAuth

OAuth is per upstream and subject. Shared gateway credential flows are available
from CLI:

```bash
labby gateway mcp auth status <name> --json
labby gateway mcp auth start <name> --json
labby gateway mcp auth open <name> --wait --json
labby gateway mcp auth clear <name> -y --json
```

Use the server-side OAuth status path when browser OAuth looks connected but
runtime calls still fail. Code Mode admin/trusted paths use the shared gateway
subject; non-admin scoped users may use their own subject.

## Tool Search Surface

The gateway-wide tool-search setting exposes the synthetic public MCP tools
`search` and `execute` instead of raw upstream tools:

```bash
labby gateway tool-search status --json
labby gateway tool-search enable --top-k-default 10 --max-tools 5000 --json
labby gateway tool-search disable --json
```

In action dispatch:

```json
{ "action": "gateway.tool_search.get", "params": {} }
{ "action": "gateway.tool_search.set", "params": { "enabled": true } }
```

## Gateway Schema Resources

For complete connected-upstream inspection:

```json
{ "action": "gateway.servers", "params": {} }
{ "action": "gateway.schema", "params": { "name": "<upstream>" } }
{ "action": "gateway.discovered_tools", "params": { "name": "<upstream>" } }
{ "action": "gateway.discovered_resources", "params": { "name": "<upstream>" } }
{ "action": "gateway.discovered_prompts", "params": { "name": "<upstream>" } }
```

MCP resources:

- `lab://gateway/servers`
- `lab://gateway/<name>/schema`

Resources are cache-backed and exposure-policy filtered. If a tool is absent,
check exposure policy and reload runtime state before concluding the upstream
does not provide it.

## Protected Routes

Protected routes publish Lab-managed public MCP routes with OAuth protection:

```bash
labby gateway protected-route list --json
labby gateway protected-route test \
  --name route \
  --public-host lab.example.invalid \
  --public-path /mcp \
  --upstream upstream-name \
  --json
labby gateway protected-route add \
  --name route \
  --public-host lab.example.invalid \
  --public-path /mcp \
  --upstream upstream-name \
  --scope lab:read \
  --json
```

Protected routes may use either an `upstream` or a `backend_url`, not both.
Backend targets are validated to avoid unsafe local/link-local targets.

## Config Mutation Actions

Use service-config actions for Labby-owned service config fields:

```json
{ "action": "gateway.service_config.get", "params": { "service": "deploy" } }
{ "action": "gateway.service_config.set", "params": { "service": "deploy", "values": {} } }
```

Values are redacted on reads when fields are marked secret.

## Common Failure Routing

| Symptom | First check |
| --- | --- |
| Upstream missing from Code Mode search | `labby gateway mcp list --json`, then `gateway.schema` |
| OAuth works in browser but runtime fails | `labby gateway mcp auth status <name> --json` |
| Tool absent from one upstream | `gateway.discovered_tools`, exposure policy, reload |
| Stale process or old schema | `labby gateway mcp cleanup <name> --dry-run --json` |
| Config changed but runtime did not | `labby gateway reload --json` |
| Import keeps reappearing | pending/tombstone actions in generated action catalog |
