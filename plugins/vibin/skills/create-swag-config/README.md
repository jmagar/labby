# create-swag-config

Add a new reverse proxy entry to **SWAG** (LinuxServer.io) on host `squirts` — the proxy that fronts every `*.tootie.tv` subdomain (~128 active configs).

## What it does

- **Preferred path:** call the `swag-mcp` MCP server (registered as `swag` at `https://swag.tootie.tv/mcp`) through its single action-routed `swag` tool. Actions: `list`, `create`, `view`, `edit`, `update`, `remove`, `logs`, `backups`, `health_check`.
- **Fallback path:** hand-write the file at `/mnt/appdata/swag/nginx/proxy-confs/<service>.subdomain.conf` using the canonical template (`references/fallback-template.md`).

## When to invoke

Triggers include "create a swag config", "add X to swag", "add a swag proxy for X", "make a subdomain config", "expose X on tootie.tv", "add reverse proxy for X", "new tootie.tv subdomain", "proxy X through swag", "scaffold a swag entry", "wire up a SWAG mcp config".

Does **not** fire on generic nginx work outside this homelab.

## What it knows

- The three deployed shapes (Authelia + MCP, upstream-OAuth + MCP, plain web) with annotated examples from `syslog`, `lab`, and `axon`
- Which nginx includes to use and what each provides (`mcp-server.conf`, `mcp-location.conf`, `authelia-*`, `proxy.conf`, `resolver.conf`, `ssl.conf`)
- That `*.tootie.tv` is a wildcard A/CNAME with a wildcard cert — no DNS or cert work needed per service
- That SWAG's filewatch picks up new configs in ~30 seconds (wait, don't panic-restart)
- Verification: use `swag` `action: "health_check"` against the new domain, then `action: "logs"` if it fails

## Files

```
SKILL.md                          — entry point, decision tree, tool surface, verification
references/
  examples.md                     — annotated side-by-side: syslog, lab, axon
  fallback-template.md            — full nginx template + save/reload procedure when swag-mcp is down
  includes.md                     — what each include file does and when to use it
README.md                         — this file
CHANGELOG.md                      — version history
```

## Related skills

- `homelab-map` — for the broader squirts/dookie/tootie host inventory
- `mcp-gateway-tools` — generic mechanics if you reach swag-mcp through the Lab gateway's `tool_search` / `tool_execute` pair instead of the direct registration
- `create-swag-config` does **not** overlap with `lab:lab-service-onboarding` (which adds Python services to the `lab` codebase) — different layer of the stack
