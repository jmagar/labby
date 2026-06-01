# navidrome

Operate a self-hosted [Navidrome](https://www.navidrome.org/) music server through its Subsonic API using direct HTTP calls — no MCP server, no `lab` CLI dependency.

## What it does

Ping/health, browse artists and albums, search the library, list playlists, and check now-playing — all read-only against the Subsonic `/rest/*` API.

## Configuration

Set when the plugin is enabled (stored by Claude Code; the password goes to secure OS storage, not `settings.json`):

| Setting | Sensitive | Description |
|---|---|---|
| Navidrome URL | no | Base URL, e.g. `https://music.example.com` (no trailing `/rest`) |
| Username | no | Navidrome account username |
| Password | yes | Used only to derive the per-request Subsonic token; never sent raw |

A `SessionStart` hook validates connectivity and credentials and prints a one-line status (or a warning); it changes nothing on the server.

## Auth model

Subsonic salted-token auth: `token = md5(password + salt)`, regenerated per session. The raw password never leaves the machine.

See the skill at `skills/navidrome/SKILL.md` and the API reference at `skills/navidrome/references/api.md`.
