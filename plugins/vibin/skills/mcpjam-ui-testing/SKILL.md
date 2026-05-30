---
name: mcpjam-ui-testing
description: Validate MCP-UI / MCP Apps implementations with MCPJam CLI and Inspector. Use when testing a server's UI-capable MCP tools, `_meta.ui.resourceUri`, `ui://` resources, `text/html;profile=mcp-app` resource contents, `structuredContent`, Inspector rendering, or Axon-style MCP Apps widgets.
---

# MCPJam UI Testing

Use this skill to prove an MCP-UI surface works at the protocol and rendered-widget layers. Prefer MCPJam CLI for deterministic checks, then MCPJam Inspector for visual/rendering behavior.

## Workflow

1. Identify the target transport.
   - HTTP: use `--url http://host:port/mcp`.
   - stdio: use `--command <binary> --args <args...> --cwd <repo>`.
   - If the server requires auth, include the MCPJam auth/header flags supported by the installed CLI version; run `mcpjam <command> --help` before guessing.

2. Run server health first.
   - `mcpjam server doctor ...`
   - Fix connectivity, auth, protocol version, or transport errors before testing UI.

3. Run app conformance.
   - `mcpjam apps conformance ...`
   - Treat failures here as contract bugs unless the CLI output clearly indicates an unsupported host/transport feature.

4. Manually verify the wire shape.
   - `mcpjam tools list ...`
   - UI-capable tools must advertise `_meta.ui.resourceUri`.
   - The URI should be stable and use `ui://...`.
   - Non-UI catch-all tools should not advertise UI metadata unless every call should render that UI.

5. Verify resource discovery and content.
   - `mcpjam resources list ...`
   - `mcpjam resources read --resource-uri ui://... ...`
   - The UI resource must use `text/html;profile=mcp-app`.
   - `resources/read` should return exactly one HTML text/blob payload for the referenced UI URI.
   - Resource `_meta.ui` should declare CSP/permissions when the app needs them; prefer locked-down empty arrays/objects for self-contained widgets.

6. Verify the tool call payload.
   - `mcpjam tools call --name <tool> --arguments '{}' ...`
   - Prefer `structuredContent` for data the widget consumes.
   - Keep model-readable `content` useful, but do not make the widget scrape human text when structured JSON is available.

7. Test rendering in Inspector.
   - Start or open Inspector with `mcpjam inspector start` or `mcpjam inspector open`.
   - Then call the UI tool with the CLI's UI rendering flag if available (`mcpjam tools call ... --ui`).
   - A pass means the Inspector reports/render-displays the app, not merely that `resources/read` returns HTML.

## Axon Pattern

For Axon, the expected UI contract is:

- UI tool: `axon_status_dashboard`
- UI resource: `ui://axon/status-dashboard`
- MIME type: `text/html;profile=mcp-app`
- Generic routed tool: `axon` should not carry dashboard UI metadata
- Tool call: `axon_status_dashboard` should return `structuredContent` with status payload data

Run focused checks:

```bash
mcpjam server doctor --url http://127.0.0.1:8001/mcp
mcpjam apps conformance --url http://127.0.0.1:8001/mcp
mcpjam tools list --url http://127.0.0.1:8001/mcp
mcpjam resources list --url http://127.0.0.1:8001/mcp
mcpjam resources read --url http://127.0.0.1:8001/mcp --resource-uri ui://axon/status-dashboard
mcpjam tools call --url http://127.0.0.1:8001/mcp --name axon_status_dashboard --arguments '{}'
```

For stdio Axon:

```bash
mcpjam server doctor --command ./target/debug/axon --args mcp --cwd /home/jmagar/workspace/axon_rust
```

## Debugging Failures

- `tools/list` missing `_meta.ui.resourceUri`: fix tool metadata, not resource serving.
- `resources/list` missing the URI: register the UI resource.
- `resources/read` wrong MIME: set `text/html;profile=mcp-app` on the returned resource contents.
- Conformance passes but UI does not render: use Inspector and check HTML runtime errors, sandbox/CSP metadata, and whether the tool result contains structured data.
- Inspector skipped/no active client: open/start Inspector first, then rerun `tools call --ui`.
- HTTP `406` or SSE errors: ensure the client sends `Accept: application/json, text/event-stream`; MCPJam normally handles this.
- Auth errors: verify bearer/OAuth config before debugging UI.

## References

Load [references/commands.md](references/commands.md) when exact MCPJam commands or expected outputs are needed.
