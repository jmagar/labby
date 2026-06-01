# Changelog

## 0.2.0

- Migrated credentials to plugin `userConfig` (per-service URL + sensitive API key/token/password).
- Added a `SessionStart`/`ConfigChange` hook (`scripts/setup.sh`) that materializes userConfig into a 600-mode `~/.config/lab-arrs/config.env`.
- Removed the legacy `load-env.sh` cred loader and the `~/.claude-homelab` / setup-symlinks dependency; wrapper scripts now source `config.env` directly.
- Reworded each skill's Setup section to point at plugin settings instead of manual `.env` editing.

## 0.1.0

- Initial release. Consolidates the media-automation skills into one plugin: radarr, sonarr, prowlarr, overseerr, sabnzbd, qbittorrent, plex, jellyfin, tautulli (moved from the `vibin` plugin).
- Merged in the former standalone `tracearr` plugin as the `tracearr` skill (its `.mcp.json`/monitors/settings were empty scaffolding, so only the skill carried over).
- Credentials continue to come from `~/.claude-homelab/.env` (unchanged per-skill convention).
