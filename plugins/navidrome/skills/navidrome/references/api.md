# Navidrome / Subsonic API reference

Navidrome implements the [Subsonic API](https://www.subsonic.org/pages/api.jsp). Base path is `/rest/`, and every endpoint takes the shared auth + format params.

## Auth params (on every request)

| Param | Meaning |
|---|---|
| `u` | username |
| `t` | token = `md5(password + salt)` |
| `s` | salt (random per session is fine) |
| `v` | protocol version, e.g. `1.16.1` |
| `c` | client name (any string, e.g. `lab`) |
| `f` | response format — use `json` |

Legacy `?u=&p=` (plaintext or `enc:<hex>`) also works but is discouraged; prefer salted token auth.

## Response envelope

```json
{ "subsonic-response": { "status": "ok", "version": "1.16.1", "type": "navidrome", "...": {} } }
```

On failure:

```json
{ "subsonic-response": { "status": "failed", "error": { "code": 40, "message": "Wrong username or password" } } }
```

Common error codes: `10` missing param, `40` wrong credentials, `50` not authorized, `70` not found.

## Endpoints used by this skill (all read-only)

| Endpoint | Purpose | Notable params |
|---|---|---|
| `ping.view` | health + auth check | — |
| `getArtists.view` | all artists (indexed) | — |
| `getArtist.view` | one artist + albums | `id` |
| `getAlbumList2.view` | album lists | `type`, `size`, `offset` |
| `getAlbum.view` | album + songs | `id` |
| `search3.view` | unified search | `query`, `artistCount`, `albumCount`, `songCount` |
| `getPlaylists.view` | all playlists | — |
| `getPlaylist.view` | playlist + entries | `id` |
| `getNowPlaying.view` | active streams | — |
| `getStarred2.view` | starred artists/albums/songs | — |
| `getScanStatus.view` | library scan progress | — |

## Mutating endpoints (NOT used here — confirm with the user first)

`star.view` / `unstar.view`, `setRating.view`, `scrobble.view`, `createPlaylist.view`, `updatePlaylist.view`, `deletePlaylist.view`. These change user/library state; only use on explicit request.

## Notes

- IDs are opaque strings; always source them from a prior list/search call, never guess.
- `getAlbumList2`/`getArtists` use the ID3-tag organization (preferred); the older `getAlbumList`/`getIndexes` use folder structure.
