---
name: navidrome
description: "This skill should be used when the user wants to interact with their Navidrome self-hosted music server — checking server status, browsing artists or albums, searching the music library, listing playlists, viewing now-playing or recently played tracks, or accessing starred items. Triggers include: \"what's playing on Navidrome\", \"show my playlists\", \"search for an artist\", \"recent albums\", \"Navidrome status\"."
---

# Navidrome

Self-hosted music streaming server. Navidrome implements the **Subsonic API**, served under `/rest/*` and returning JSON when `f=json` is passed. This skill calls it directly with `curl`.

## Configuration

The user sets URL / username / password in the plugin's user configuration when the plugin is enabled. A `SessionStart` hook writes those into a 600-mode file:

```
${XDG_CONFIG_HOME:-$HOME/.config}/lab-navidrome/config.env
```

> Why a file and not env vars: Claude Code injects `CLAUDE_PLUGIN_OPTION_*` only into plugin subprocesses (hooks/MCP/LSP), **not** into the Bash tool that runs these commands. The hook (a subprocess) reads them and materializes this file; the skill sources it.

Load it first. If it is missing, fall back to `~/.lab/.env`; this docs/homelab
setup keeps `NAVIDROME_URL`, `NAVIDROME_USERNAME`, and `NAVIDROME_PASSWORD`
there. If neither source has all three values, tell the user to set the
Navidrome URL/username/password in plugin settings or `~/.lab/.env`.

```bash
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/lab-navidrome/config.env"
if [ -f "$CONFIG" ]; then
  . "$CONFIG"
elif [ -f "$HOME/.lab/.env" ]; then
  set -a; . "$HOME/.lab/.env"; set +a
fi
[ -n "${NAVIDROME_URL:-}" ] && [ -n "${NAVIDROME_USERNAME:-}" ] && [ -n "${NAVIDROME_PASSWORD:-}" ] || {
  echo "navidrome not configured — set URL/username/password in plugin settings or ~/.lab/.env"
}
```

In this homelab, the public URL may return `403` for unauthenticated script
probes. If Subsonic requests through `NAVIDROME_URL` fail with proxy/auth errors,
use the direct backend `http://10.1.0.2:4533` for API calls.

## Authentication (do this first, every session)

Subsonic auth is token-based: generate a random `salt`, then `token = md5(password + salt)`. The raw password never goes over the wire. Build the shared auth query once and reuse it:

```bash
URL="${NAVIDROME_URL%/}"
SALT="$(openssl rand -hex 8)"
TOKEN="$(printf '%s%s' "$NAVIDROME_PASSWORD" "$SALT" | md5sum | cut -d' ' -f1)"
AUTH="u=${NAVIDROME_USERNAME}&t=${TOKEN}&s=${SALT}&v=1.16.1&c=lab&f=json"
```

`v` is the Subsonic protocol version, `c` is the client name (any string), `f=json` requests JSON. Every call below appends `?$AUTH`.

> Never echo `$NAVIDROME_PASSWORD` or the derived `$TOKEN`. A fresh salt+token per session is fine; you do not need a new one per request.

## Common operations

| Intent | Request |
|---|---|
| Health / auth check | `curl -sS "$URL/rest/ping.view?$AUTH"` |
| List all artists | `curl -sS "$URL/rest/getArtists.view?$AUTH"` |
| List albums (newest 20) | `curl -sS "$URL/rest/getAlbumList2.view?type=newest&size=20&$AUTH"` |
| Album details + tracks | `curl -sS "$URL/rest/getAlbum.view?id=<albumId>&$AUTH"` |
| Search everything | `curl -sS "$URL/rest/search3.view?query=<term>&$AUTH"` |
| List playlists | `curl -sS "$URL/rest/getPlaylists.view?$AUTH"` |
| Playlist contents | `curl -sS "$URL/rest/getPlaylist.view?id=<playlistId>&$AUTH"` |
| Now playing | `curl -sS "$URL/rest/getNowPlaying.view?$AUTH"` |
| Starred items | `curl -sS "$URL/rest/getStarred2.view?$AUTH"` |

`getAlbumList2` `type` accepts `newest`, `recent`, `frequent`, `random`, `alphabeticalByName`, `starred`, etc. URL-encode user-supplied query terms.

Full endpoint and response reference: [`references/api.md`](references/api.md).

## Checking the response

Subsonic wraps every reply: `{"subsonic-response":{"status":"ok"|"failed", ...}}`. On `failed`, read `.error.code` / `.error.message` (e.g. code 40 = wrong username/password, code 70 = data not found). Pipe through `jq '."subsonic-response"'` to read it.

## When NOT to use this skill

- The user wants to upload, edit tags, or change library/server settings — Navidrome manages those through its web UI and scanner, not the Subsonic API.
- The user is asking about a different homelab service — load that service's skill.
- Streaming actual audio bytes (`stream.view`) into the terminal is rarely useful; prefer metadata endpoints unless the user explicitly wants a download.
