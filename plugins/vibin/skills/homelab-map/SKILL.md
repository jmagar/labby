---
name: homelab-map
description: 'This skill should be used whenever a prompt mentions any of Jacob''s named homelab hosts — **tootie, dookie, shart, squirts, steamy, steamy-wsl, vivobook, vivobook-wsl** — or the **WillyNet** LAN. It provides the authoritative map of which host runs which service, IPs, ports, SSH details, storage layout, backup chains, MCP server locations, and known issues. Example triggers: "what''s on dookie", "is plex running on tootie", "ssh into squirts", "where''s the qdrant container", "which box has the GPU", "check shart''s backup status", "the steamy box", "ssh steamy-wsl", "vivobook-wsl logs". Does NOT fire on generic "my homelab" / "my server" / "my NAS" prompts that don''t name a specific host. Runtime inventory is generated at `~/.homelab/homelab.md` and `~/.homelab/homelab.json`; `references/homelab.md` is only the report template.'
---

# homelab-map

Quick map of Jacob's homelab. **Full runtime inventory lives at `~/.homelab/homelab.md` and `~/.homelab/homelab.json` — read or regenerate those files the moment you need anything more specific than this overview.**

## Nodes at a glance

| Name | Role | LAN IP | OS | Notes |
|---|---|---|---|---|
| **tootie** | Primary NAS / app server | 10.1.0.2 | Unraid 7.2.4 (i7-13700K, 128GB) | 49 containers. Web: `:6969`. SSH port `29229`. Also runs dookie as a KVM guest. **⚠ no parity disk currently.** |
| **dookie** | Dev / AI / MCP hub | 10.1.0.6 | Linux KVM guest on tootie | Axon RAG stack, syslog-mcp (1514/3100), arcane-mcp (44332), unraid-mcp (40010), Lab (8765), MCP bridge containers (40020-40060). |
| **squirts** | Edge services | 10.1.0.8 | Ubuntu (4 cores, 15GB) | SWAG (149 active configs), Authelia, AdGuard, Gotify, MCP gateway, Vaultwarden, Paperless, etc. RAM sample 10GiB/14GiB used. |
| **shart** | ZFS backup target | 10.1.0.3 | Unraid | ZFS `backup` pool 7.27TB / 1.80TB used. Also has old link-local `169.254.80.235` on `shim-br0`. Receives Syncoid streams from tootie + squirts. |
| **steamy** | GPU workloads (RTX 4070) | 10.1.0.65 | Win11 + WSL2 | `crawl4r-qdrant` (GPU qdrant). Arcane marks this env disabled/offline, but `ssh steamy-wsl` works and remains the default target for the `screenshots`, `clipboard`, `nircmd` skills. |
| **vivobook** | Mobile dev laptop | 10.1.0.5 (when docked) | Win11 + WSL2 | Just an `arcane-agent`. |

All nodes joined to **Tailscale** mesh (`100.x.y.z`). Router is a UniFi UCG-Max ("The Mothership"). WiFi SSID `WillyNet`. Public services live at `*.tootie.tv` via SWAG.

## "Where does X live" — quick lookups

| If the user mentions… | It's on… |
|---|---|
| Plex, Sonarr, Radarr, Bazarr, Prowlarr, qBittorrent, Sabnzbd, Tautulli, Immich, Audiobookshelf, Kavita, Navidrome | tootie |
| Axon runtime, Qdrant (CPU), TEI/Qwen3-Embedding, axon-chrome | dookie |
| GPU qdrant (`crawl4r-qdrant`), anything with `gpu-nvidia` | steamy |
| SWAG, Authelia, AdGuard, Gotify, Vaultwarden, Paperless, Linkding, Karakeep, Bytestash, Memos, Radicale, Searxng, Dockge, Dozzle, multi-scrobbler/maloja, RustDesk | squirts |
| Sanoid / Syncoid backups, ZFS receive | shart |
| Portainer, Glances, Scrutiny, Vnstat, MinIO, Loggifly, Notifiarr, Apprise API, Olivetin, Crontab UI, Zipline | tootie |
| **MCP servers** — syslog, arcane, unraid, swag, unifi, gotify, tailscale, apprise, rmcp-template/example | mostly **dookie + squirts** — see `~/.homelab/homelab.md` §"MCP Server Ecosystem" for exact host |
| Windows sandbox (tootie:8006 noVNC), agent-os skill target | tootie (`agent-os-win11` / dockurr/windows container) |

## Conventions

- **All scatological naming.** Don't be cute about it — they are named tootie, dookie, shart, squirts. Use the names verbatim.
- **`steamy-wsl` ≠ `steamy`** in skill defaults: most skills (`screenshots`, `clipboard`, `nircmd`, `chrome`) target the WSL2 alias because the actual user desktop / win11 box is reached via WSL ssh.
- **`*.tootie.tv` = SWAG vhost on squirts**, fronts a service running anywhere. Don't assume the service is on tootie just because of the domain.
- **arcane-agent usually runs on every node** — it manages local compose projects. Check the generated report because stopped/missing agents are runtime state.
- **Public SSH does not exist.** All inter-node SSH goes through the Tailscale mesh.

## When to read the reference doc

Read `~/.homelab/homelab.md` or `~/.homelab/homelab.json` whenever you need:
- Exact container lists per host
- Storage layout (Unraid disk slots, ZFS pools, share-level breakdowns)
- Backup chains (which datasets replicate where)
- All 149 active SWAG configs
- Current runtime collection notes and known follow-up checks
- Known issues / tech debt log
- Specific port numbers beyond the headline ones above

`grep` the generated report — it's structured with clear generated headers (`## Nodes`, `## Service Location Summary`, `## Host Container Inventory`, `## Storage Architecture`, `## AI / RAG / Agent Stack`, `## MCP Server Ecosystem`, etc.)

## Updating this skill

`~/.homelab/homelab.md`, `~/.homelab/homelab.json`, and `~/.homelab/index.html` are generated from live collection plus the template at `references/homelab.md`. Refresh them instead of hand-editing point-in-time data:

```bash
python3 src/skills/homelab-map/scripts/generate-homelab-report.py
```

By default, the generator also starts or reuses a viewer on `0.0.0.0:40500` so SWAG can reach it through dookie's Tailscale IP, checks `tailscale status`, and attempts Tailscale Serve on HTTPS port `8447` only if Tailscale is installed and usable. Use `--no-serve` when you only want files.

The generator uses non-interactive SSH, Docker CLI, ZFS CLI, Unraid shell commands, and SWAG config files. Treat container counts, RAM%, uptime numbers etc. as **point-in-time** — re-run the generator before acting on anything that depends on current state. Names of nodes, roles, and architectural choices are stable; individual IPs and ports should still be verified before automation.

If a stable architectural fact changes (node renamed, service permanently moved, critical port changed), update `scripts/generate-homelab-report.py` and/or the template in `references/homelab.md`, then regenerate `~/.homelab` artifacts. Keep this overview aligned with the generated report.
