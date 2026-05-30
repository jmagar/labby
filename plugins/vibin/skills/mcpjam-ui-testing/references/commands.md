# MCPJam Command Reference

Use `mcpjam <group> --help` on the target machine first; flags can change between MCPJam releases.

## Target Forms

HTTP:

```bash
mcpjam server doctor --url http://127.0.0.1:8001/mcp
```

stdio:

```bash
mcpjam server doctor \
  --command ./target/debug/axon \
  --args mcp \
  --cwd /home/jmagar/workspace/axon_rust
```

## MCP Apps Conformance

```bash
mcpjam apps conformance --url http://127.0.0.1:8001/mcp
```

Expected checks:

- UI tool metadata exists.
- `_meta.ui.resourceUri` is a valid `ui://...` URI.
- Referenced resource is listed.
- Referenced resource is readable.
- Resource MIME type is `text/html;profile=mcp-app`.
- Resource metadata shape is valid.

## Manual Protocol Checks

```bash
mcpjam tools list --url http://127.0.0.1:8001/mcp
mcpjam resources list --url http://127.0.0.1:8001/mcp
mcpjam resources read \
  --url http://127.0.0.1:8001/mcp \
  --resource-uri ui://axon/status-dashboard
mcpjam tools call \
  --url http://127.0.0.1:8001/mcp \
  --name axon_status_dashboard \
  --arguments '{}'
```

Check for:

- `axon_status_dashboard._meta.ui.resourceUri == "ui://axon/status-dashboard"`.
- `axon` has no dashboard UI metadata unless intentionally rendering for every routed call.
- `resources/read` returns HTML content, not markdown/text-only fallback.
- Tool call returns `structuredContent`.

## Inspector

```bash
mcpjam inspector start
mcpjam inspector open
mcpjam tools call \
  --url http://127.0.0.1:8001/mcp \
  --name axon_status_dashboard \
  --arguments '{}' \
  --ui
```

If `--ui` is not recognized, run:

```bash
mcpjam tools call --help
mcpjam inspector --help
```

Then use the current release's equivalent render/open flow.

## agent-os Notes

On the `agent-os` Windows VM, verified packages were:

- `@mcpjam/cli@3.3.4`
- `@mcpjam/inspector@2.4.15`

Available commands:

- `mcpjam`
- `inspector`
- `inspector-vite`
