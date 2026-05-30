# homelab-map

Authoritative map of Jacob's WillyNet homelab. Triggers strictly on named devices.

## What it does
Tells the agent which host runs which service before it touches anything. Covers six devices — tootie, dookie, shart, squirts, steamy/steamy-wsl, vivobook/vivobook-wsl — plus network topology, MCP servers, storage layout, backup chains, known issues.

## When to invoke
Any prompt naming one of the devices, or `WillyNet`. Strict device-name fidelity — won't fire on generic "my homelab" / "my server" / "my NAS" prompts.

## Files
- `SKILL.md` — lean overview: nodes-at-a-glance table, service→host lookup, conventions
- `references/homelab.md` — static report template containing the `{{generated_report}}` insertion marker
- `scripts/generate-homelab-report.py` — pulls host/container/storage/proxy state and writes report artifacts under `~/.homelab`
- `~/.homelab/homelab.md` — generated markdown runtime inventory, read on demand
- `~/.homelab/homelab.json` — generated structured runtime inventory for tools and viewers
- `~/.homelab/index.html` — generated browser viewer for the JSON inventory

## Updating
Regenerate the external report instead of hand-maintaining runtime values:

```bash
python3 src/skills/homelab-map/scripts/generate-homelab-report.py
```

The generator uses non-interactive SSH plus Docker/ZFS/Unraid/SWAG shell probes. Container counts, RAM%, uptime etc. are point-in-time; rerun before acting on anything current-state dependent.

Use `--output <path>` when you need a one-off report somewhere other than `~/.homelab/homelab.md`. Keep volatile generated output out of the repository.

After writing artifacts, the generator starts or reuses a viewer on `0.0.0.0:40500` so SWAG can reach it through dookie's Tailscale IP. It then checks `tailscale status`; if Tailscale is installed and usable, it attempts to expose the viewer through Tailscale Serve on HTTPS port `8447`. Missing or unhealthy Tailscale does not fail report generation.

Disable serving for CI or one-off generation:

```bash
python3 src/skills/homelab-map/scripts/generate-homelab-report.py --no-serve
```

Override ports:

```bash
python3 src/skills/homelab-map/scripts/generate-homelab-report.py --serve-bind 127.0.0.1 --serve-port 40501 --tailscale-https-port 8448
```
