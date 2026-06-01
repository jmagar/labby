# arrs

The *arr / media-automation stack in a single plugin. Each skill talks directly to its service's REST API.

## Bundled skills

| Skill | Service |
|---|---|
| `radarr` | Movies (Radarr) |
| `sonarr` | TV (Sonarr) |
| `prowlarr` | Indexers (Prowlarr) |
| `overseerr` | Requests (Overseerr) |
| `sabnzbd` | Usenet downloads (SABnzbd) |
| `qbittorrent` | Torrents (qBittorrent) |
| `plex` | Plex Media Server |
| `jellyfin` | Jellyfin media server |
| `tautulli` | Plex analytics (Tautulli) |
| `tracearr` | Media-server monitoring (Plex/Jellyfin/Emby) — merged in from the former standalone `tracearr` plugin |

## Configuration

Credentials are set in the **plugin's `userConfig`** (prompted when the plugin is enabled, editable in plugin settings). Each service has a URL plus an API key / token / password; secrets (`*_api_key`, `*_token`, `*_password`) are marked `sensitive`, so they're stored in secure OS storage, not `settings.json`. Configure only the services you actually run — every field is optional.

A `SessionStart` / `ConfigChange` hook (`scripts/setup.sh`) reads the configured values and writes them to a 600-mode file:

```
~/.config/lab-arrs/config.env
```

The bundled wrapper scripts (`skills/*/scripts/*`) source that file automatically — no manual env editing, no `~/.claude-homelab` / setup-symlinks dependency (the old `load-env.sh` plumbing was removed).

> Why a hook+file instead of reading userConfig directly: Claude Code injects `CLAUDE_PLUGIN_OPTION_*` only into plugin subprocesses (hooks), not the Bash tool the scripts run under — so the hook materializes the file the scripts read.
