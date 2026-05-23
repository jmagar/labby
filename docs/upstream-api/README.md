# Upstream API References

Reference documentation for every upstream or capability contract that
`lab-apis` wraps. Use these when implementing, auditing, or refreshing service
modules.

## Index

| Service | File | Format | Source |
| --- | --- | --- | --- |
| ACP Registry | [acp-registry.md](./acp-registry.md) | contract note | Lab ACP registry metadata |
| AdGuard Home | [adguard.md](./adguard.md) | upstream REST/OpenAPI notes | AdGuard Home API |
| Apprise | [apprise.md](./apprise.md) | hand-scraped README | caronc/apprise-api |
| Arcane | [arcane-api.yaml](./arcane-api.yaml) | OpenAPI 3.x | getarcane.app |
| Beads | [beads.md](./beads.md) | local CLI JSON contract | Beads `bd` CLI |
| ByteStash | [bytestash.md](./bytestash.md) | instance-served API note | `$BYTESTASH_URL/api-docs` |
| Dozzle | [dozzle.md](./dozzle.md) | HTTP/JSON + SSE notes | amir20/dozzle |
| FreshRSS | [freshrss.md](./freshrss.md) | Google Reader-compatible API notes | FreshRSS API |
| Glances | [glances.md](./glances.md) | REST API notes | Glances REST API v4 |
| Gotify | [gotify.openapi.json](./gotify.openapi.json) | Swagger 2.0 | gotify/server |
| Immich | [immich.md](./immich.md) | OpenAPI-backed notes | api.immich.app |
| Jellyfin | [jellyfin.openapi.json](./jellyfin.openapi.json) | OpenAPI | Jellyfin API |
| Linkding | [linkding.md](./linkding.md) | hand-scraped docs | linkding.link/api |
| LoggiFly | [loggifly.md](./loggifly.md) | deferred contract note | LoggiFly project docs |
| MCP Registry | [mcp-registry.yaml](./mcp-registry.yaml) | OpenAPI | modelcontextprotocol registry |
| Memos | [memos.openapi.yaml](./memos.openapi.yaml) | OpenAPI from protobuf | usememos/memos |
| Navidrome | [navidrome.md](./navidrome.md) | Subsonic/OpenSubsonic notes | Navidrome API |
| Neo4j | [neo4j.md](./neo4j.md) | Bolt client contract | neo4rs/Neo4j docs |
| NotebookLM | [notebooklm.md](./notebooklm.md) | private RPC contract note | teng-lin/notebooklm-py |
| OpenACP | [openacp.md](./openacp.md) | upstream daemon contract | OpenACP |
| OpenAI | [openai.openapi.yaml](./openai.openapi.yaml) | OpenAPI 3.x | openai/openai-openapi manual_spec |
| Overseerr | [overseerr.openapi.yaml](./overseerr.openapi.yaml) | OpenAPI 3.0.2 | sct/overseerr |
| Pi-hole | [pihole.md](./pihole.md) | v6 REST notes | Pi-hole API |
| Plex | [plex.openapi.yaml](./plex.openapi.yaml) | OpenAPI 3.x | LukeHagar/plex-api-spec |
| Prowlarr | [prowlarr.openapi.json](./prowlarr.openapi.json) | OpenAPI 3.0 | Prowlarr develop branch |
| qBittorrent | [qbittorrent.md](./qbittorrent.md) | WebUI API notes | qBittorrent WebUI API |
| Qdrant | [qdrant.openapi.json](./qdrant.openapi.json) | OpenAPI 3.x | qdrant/qdrant |
| Radarr | [radarr.openapi.json](./radarr.openapi.json) | OpenAPI 3.0 | Radarr develop branch |
| SABnzbd | [sabnzbd.md](./sabnzbd.md) | hand-scraped wiki | sabnzbd.org wiki |
| Scrutiny | [scrutiny.md](./scrutiny.md) | read-only endpoint notes | Scrutiny API |
| Sonarr | [sonarr.openapi.json](./sonarr.openapi.json) | OpenAPI 3.0 | Sonarr develop branch |
| Tailscale | [tailscale.openapi.yaml](./tailscale.openapi.yaml) | OpenAPI 3.x | api.tailscale.com |
| Tautulli | [tautulli.md](./tautulli.md) | hand-scraped wiki | Tautulli wiki |
| TEI | [tei.openapi.json](./tei.openapi.json) | OpenAPI 3.x | huggingface/text-embeddings-inference |
| UniFi | [unifi.md](./unifi.md) | controller API notes | UniFi Network Application |
| Unraid | [unraid-api-complete-reference.md](./unraid-api-complete-reference.md) | GraphQL reference | docs.unraid.net/API |
| Uptime Kuma | [uptime-kuma.md](./uptime-kuma.md) | Socket.IO API notes | Uptime Kuma |

## Refreshing Specs

OpenAPI specs go stale. Refresh from the upstream project that owns each spec,
then update the paired coverage doc and generated catalogs if the public action
surface changes.

Common refresh commands:

```bash
curl -fsSL https://raw.githubusercontent.com/Radarr/Radarr/develop/src/Radarr.Api.V3/openapi.json > docs/upstream-api/radarr.openapi.json
curl -fsSL https://raw.githubusercontent.com/Sonarr/Sonarr/develop/src/Sonarr.Api.V3/openapi.json > docs/upstream-api/sonarr.openapi.json
curl -fsSL https://raw.githubusercontent.com/Prowlarr/Prowlarr/develop/src/Prowlarr.Api.V1/openapi.json > docs/upstream-api/prowlarr.openapi.json
curl -fsSL https://raw.githubusercontent.com/LukeHagar/plex-api-spec/main/plex-api-spec.yaml > docs/upstream-api/plex.openapi.yaml
curl -fsSL "https://api.tailscale.com/api/v2?outputOpenapiSchema=true" > docs/upstream-api/tailscale.openapi.yaml
curl -fsSL https://raw.githubusercontent.com/usememos/memos/main/proto/gen/openapi.yaml > docs/upstream-api/memos.openapi.yaml
curl -fsSL https://raw.githubusercontent.com/sct/overseerr/develop/openapi.yaml > docs/upstream-api/overseerr.openapi.yaml
curl -fsSL https://raw.githubusercontent.com/gotify/server/master/docs/spec.json > docs/upstream-api/gotify.openapi.json
curl -fsSL https://raw.githubusercontent.com/openai/openai-openapi/manual_spec/openapi.yaml > docs/upstream-api/openai.openapi.yaml
curl -fsSL https://raw.githubusercontent.com/qdrant/qdrant/master/docs/redoc/master/openapi.json > docs/upstream-api/qdrant.openapi.json
curl -fsSL https://raw.githubusercontent.com/huggingface/text-embeddings-inference/main/docs/openapi.json > docs/upstream-api/tei.openapi.json
```

Instance-served or private-contract docs such as ByteStash,
NotebookLM, OpenACP, and Uptime Kuma require manual review against the running
  instance or upstream repository.
