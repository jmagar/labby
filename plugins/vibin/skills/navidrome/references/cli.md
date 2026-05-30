# Navidrome CLI Reference

Use the CLI only when the MCP tool is unavailable or the user explicitly asks for shell commands.

## Discovery

```bash
lab navidrome --help
lab navidrome <action> --help
labby --json navidrome <action> ...
```

MCP action names with dots are exposed as dash-separated CLI commands, for example `server.health` becomes `server-health`.

Prefer JSON output where available so results can be parsed instead of scraped.
