# labby Binary Crate

`crates/lab` builds the `labby` product binary. It owns the CLI, MCP server,
HTTP API, Labby web serving, config loading, output rendering, generated docs,
and shared product dispatch.

Pure SDK/data types live in `crates/lab-apis`. HTTP/OAuth middleware lives in
`crates/lab-auth`. Windows process-tree reaping lives in `crates/lab-winjob`.

## Build

```bash
cargo build -p labby --all-features
cargo build -p labby --no-default-features --features gateway
cargo build -p labby --no-default-features --features marketplace
```

## Run

```bash
labby --help
labby mcp
labby serve
labby doctor
labby marketplace mcp.list --params '{"search":"github","limit":10}'
```

`labby serve` hosts the product HTTP API, streamable HTTP MCP at `/mcp`, auth
routes, and the Labby web UI when exported assets are available.

## Feature Slices

Supported standalone product slices:

- `gateway`
- `marketplace`
- `fs`
- `deploy`
- `acp_registry`

`mcpregistry` is a compatibility alias for `marketplace`. Base services such as
`doctor`, `setup`, `nodes`, `logs`, `stash`, and `acp` are intentionally compiled
without individual feature flags.

## Dispatch

Services expose a standard action shape across MCP and HTTP:

```json
{
  "action": "mcp.list",
  "params": { "search": "github", "limit": 10 }
}
```

The registry and action catalog are generated from code-owned metadata. See the
workspace README and `docs/` for the current public contract.
