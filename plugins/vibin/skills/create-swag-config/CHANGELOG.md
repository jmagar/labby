# Changelog

All notable changes to this skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## 2026-05-17

### Added

- Initial release of the `create-swag-config` skill.
- `SKILL.md` covering the preferred path (call `swag-mcp` via the `swag` MCP server at `https://swag.tootie.tv/mcp`, action-routed through a single `swag` tool with actions `list` / `create` / `view` / `edit` / `update` / `remove` / `logs` / `backups` / `health_check`) and the fallback path (hand-write at `/mnt/appdata/swag/nginx/proxy-confs/`).
- `references/examples.md` — annotated side-by-side of three deployed configs (`syslog`, `lab`, `axon`) with the differences highlighted.
- `references/fallback-template.md` — full nginx server-block template plus save/reload procedure for when swag-mcp is unreachable.
- `references/includes.md` — what each nginx include in `/mnt/appdata/swag/nginx/` provides and when to use it.
- `README.md` — human-facing overview, when-to-invoke, file layout, related skills.

### Skill-review polish applied before first ship

The skill was reviewed by `plugin-dev:skill-reviewer` after initial draft. Changes from that review:

- **Corrected the swag-mcp tool surface.** First draft listed `create_config`, `list_configs`, `update_config`, etc. as separate tools. The server actually exposes **one** tool (`swag`) plus `swag_help`, dispatched by an `action` parameter. Replaced the bogus tool table with the real action list (`list`, `create`, `view`, `edit`, `update`, `remove`, `logs`, `backups`, `health_check`).
- **Named the gateway-side server alias.** Added the explicit registration: `swag` server at `https://swag.tootie.tv/mcp` (from `~/.claude.json`).
- **Added a concrete `action: "create"` JSON example** so the agent can pattern-match a real call.
- **Moved the 50-line hand-write template** out of `SKILL.md` and into `references/fallback-template.md` (progressive disclosure — only load when swag-mcp is unreachable).
- **Moved the "what each include does" table** out of `SKILL.md` and into `references/includes.md`.
- **Added a DNS + cert note**: `*.tootie.tv` is a wildcard with a wildcard cert; no per-service work needed.
- **Added a filewatch latency note**: SWAG picks up new configs in ~30 seconds; don't panic-restart.
- **Documented the `view` action** as the read tool (the previously-unnamed "diff before edit" path).
- **Documented the `samples` filter** on `list` (returns LinuxServer-shipped `*.subdomain.conf.sample` reference configs to crib from for non-MCP services).
- **Updated verification checklist** to use `swag` `action: "health_check"` and `action: "logs"` instead of raw `nginx -t` + `docker logs swag`.

### Skill metadata

- `name: create-swag-config`
- description length: ~960 chars (under the 1024 cap)
- passes `skills-ref validate`
- symlinked into `~/.claude/skills/create-swag-config`

## 2026-05-17 (post-ship correction)

### Changed

- **Re-anchored `references/fallback-template.md` on `_template.subdomain.conf.sample`** (LinuxServer's official baseline at `/mnt/appdata/swag/nginx/proxy-confs/_template.subdomain.conf.sample`). The first draft documented the MCP-specific shape (server-level `set $upstream_*`, multiple location blocks, `mcp-*` includes) as if it were the baseline — it isn't. The LSIO sample is the baseline; MCP services are a deviation that layers extra structure on top. The reference now leads with the upstream sample (verbatim, comments stripped), explains the LSIO conventions (`<container_name>.*` wildcard server_name, in-location `set` directives), and then shows the diff for MCP services.
- Hand-write decision tree clarified: plain web → start from `_template.subdomain.conf.sample`; MCP-aware → start from a deployed config (`lab` / `syslog` / `axon`).
