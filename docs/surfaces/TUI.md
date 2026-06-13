# TUI

The Ratatui plugin-manager surface is not part of the current `labby` CLI or
runtime. Older docs referred to `lab plugins`, compiled-in service metadata
tabs, and `.mcp.json` patching from a TUI; that surface is currently deferred.

Current operator surfaces are:

- CLI: `labby marketplace`, `labby gateway`, `labby setup`, `labby stash`,
  `labby logs`, `labby nodes`, `labby doctor`, `labby health`, and feature-gated
  `labby deploy`
- MCP: `labby mcp` and hosted `/mcp`
- HTTP/Web: `labby serve`
- Labby web UI for marketplace, gateway, logs, setup, activity, settings, docs,
  filesystem preview, and ACP chat workflows

If a TUI is restored later, it should consume the generated service catalog and
feature matrix instead of hardcoding service categories or assuming removed
first-party upstream integrations.
