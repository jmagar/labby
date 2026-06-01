# Changelog

## 0.1.0

- Initial release. Direct-HTTP, read-only plugin for Uptime Kuma (replaces the removed lab-MCP-routed skill).
- `userConfig` for URL, a sensitive metrics API key, and an optional status-page slug.
- Reads Prometheus `/metrics` (API-key Basic auth) and public status-page JSON.
- Documents that monitor management requires Uptime Kuma's socket.io API and is intentionally out of scope.
- `SessionStart` / `ConfigChange` hook validates read connectivity (no writes).
