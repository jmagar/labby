# Changelog

## 0.1.0

- Initial release. Direct Subsonic-API plugin for Navidrome (replaces the removed lab-MCP-routed skill).
- `userConfig` for URL, username, and a sensitive password; salted-token auth derived per session.
- Read-only skill: ping, artists, albums, album detail, search, playlists, now-playing, starred.
- `SessionStart` / `ConfigChange` hook validates connectivity and credentials (no writes).
