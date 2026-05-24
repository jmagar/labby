# Plugin Coverage

All plugins in `plugins/` with their registered components. Each plugin lives at `plugins/<name>/` and declares itself via `.claude-plugin/plugin.json`.

**Categories:** agents · bin · commands · hooks · monitors · output-styles · scripts · skills · themes · .mcp.json · .lsp.json · settings.json

---

## acp

| Type | Detail |
|------|--------|
| skill | `skills/rust/SKILL.md` |
| .mcp.json | *(empty — no servers registered)* |

---

## adguard

| Type | Detail |
|------|--------|
| skill | `skills/adguard/SKILL.md` |
| .mcp.json | `adguard` → `labby mcp --services adguard` |

---

## ai-sdk

| Type | Detail |
|------|--------|
| .mcp.json | *(empty — no servers registered)* |

---

## apprise

| Type | Detail |
|------|--------|
| skill | `skills/apprise/SKILL.md` |
| .mcp.json | `apprise` → `labby mcp --services apprise` |

---

## arcane

| Type | Detail |
|------|--------|
| hook | `hooks/hooks.json` |
| skill | `skills/arcane/SKILL.md` |
| .mcp.json | `arcane` → `labby mcp --services arcane` |

---

## axon

| Type | Detail |
|------|--------|
| skill | `skills/axon/SKILL.md` |
| .mcp.json | `axon` → `axon mcp` |

---

## birdclaw

| Type | Detail |
|------|--------|
| skill | `skills/birdclaw/SKILL.md` |

---

## bitwarden

| Type | Detail |
|------|--------|
| bin | `bin/bitwarden-mcp` |
| command | `commands/bw-generate.md` |
| command | `commands/bw-get.md` |
| command | `commands/bw-list.md` |
| script | `scripts/install-shell-wrappers` |
| script | `scripts/session` |
| skill | `skills/bitwarden/SKILL.md` |
| .mcp.json | `bitwarden` → `${CLAUDE_PLUGIN_ROOT}/bin/bitwarden-mcp` |

---

## bytestash

| Type | Detail |
|------|--------|
| skill | `skills/bytestash/SKILL.md` |
| .mcp.json | `bytestash` → `labby mcp --services bytestash` |

---

## claude

| Type | Detail |
|------|--------|
| .mcp.json | `claude-code` → `claude mcp serve` (stdio) |

---

## claude-in-mobile

| Type | Detail |
|------|--------|
| monitor | `monitors/monitors.json` |
| skill | `skills/claude-in-mobile/SKILL.md` |
| .mcp.json | `mobile` → `npx -y claude-in-mobile@3.7.0` (stdio) |
| settings.json | ✓ |

---

## codex

| Type | Detail |
|------|--------|
| .mcp.json | `codex` → `codex mcp-server` (stdio) |

---

## discrawl

| Type | Detail |
|------|--------|
| skill | `skills/discrawl/SKILL.md` |

---

## download-stack

| Type | Detail |
|------|--------|
| skill | `skills/qbittorrent/SKILL.md` |
| skill | `skills/sabnzbd/SKILL.md` |
| .mcp.json | `download-stack` → `labby mcp --services sabnzbd,qbittorrent` |

---

## dozzle

| Type | Detail |
|------|--------|
| skill | `skills/dozzle/SKILL.md` |
| .mcp.json | `dozzle` → `${userConfig.dozzle_mcp_url}` (HTTP, Dozzle native `/api/mcp`) |

---

## freshrss

| Type | Detail |
|------|--------|
| skill | `skills/freshrss/SKILL.md` |
| .mcp.json | `freshrss` → `labby mcp --services freshrss` |

---

## gemini

| Type | Detail |
|------|--------|
| .mcp.json | *(empty — no servers registered)* |

---

## gh-auto

| Type | Detail |
|------|--------|
| skill | `skills/gh-address-comments/SKILL.md` |
| skill | `skills/gh-fix-ci/SKILL.md` |
| .mcp.json | `github` → `https://api.githubcopilot.com/mcp/` (HTTP) |

---

## glances

| Type | Detail |
|------|--------|
| skill | `skills/glances/SKILL.md` |
| .mcp.json | `glances` → `labby mcp --services glances` |

---

## gogcli

| Type | Detail |
|------|--------|
| skill | `skills/gogcli/SKILL.md` |

---

## gotify

| Type | Detail |
|------|--------|
| hook | `hooks/hooks.json` |
| skill | `skills/gotify/SKILL.md` |
| .mcp.json | `gotify` → `labby mcp --services gotify` |

---

## homelab-health

| Type | Detail |
|------|--------|
| skill | `skills/homelab-health/SKILL.md` |
| skill | `skills/zfs/SKILL.md` |
| .mcp.json | *(empty — no servers registered)* |

---

## immich

| Type | Detail |
|------|--------|
| skill | `skills/immich/SKILL.md` |
| .mcp.json | `immich` → `labby mcp --services immich` |

---

## jellyfin

| Type | Detail |
|------|--------|
| monitor | `monitors/monitors.json` |
| skill | `skills/jellyfin/SKILL.md` |
| .mcp.json | *(empty — no servers registered)* |
| settings.json | ✓ |

---

## lab

| Type | Detail |
|------|--------|
| skill | `skills/lab-service-onboarding/SKILL.md` |
| skill | `skills/using-lab-cli/SKILL.md` |
| .mcp.json | `lab` → `labby mcp` (all services) |

---

## linkding

| Type | Detail |
|------|--------|
| skill | `skills/linkding/SKILL.md` |
| .mcp.json | `linkding` → `labby mcp --services linkding` |

---

## loggifly

| Type | Detail |
|------|--------|
| skill | `skills/loggifly/SKILL.md` |
| .mcp.json | `loggifly` → `labby mcp --services loggifly` |

---

## mcp

| Type | Detail |
|------|--------|
| skill | `skills/mcporter/SKILL.md` |
| skill | `skills/rust/SKILL.md` |
| .mcp.json | *(empty — no servers registered)* |

---

## media-stack

| Type | Detail |
|------|--------|
| skill | `skills/overseerr/SKILL.md` |
| skill | `skills/plex/SKILL.md` |
| skill | `skills/prowlarr/SKILL.md` |
| skill | `skills/radarr/SKILL.md` |
| skill | `skills/sonarr/SKILL.md` |
| skill | `skills/tautulli/SKILL.md` |
| .mcp.json | `media-stack` → `labby mcp --services sonarr,radarr,prowlarr,overseerr,plex,tautulli` |

---

## memos

| Type | Detail |
|------|--------|
| skill | `skills/memos/SKILL.md` |
| .mcp.json | `memos` → `labby mcp --services memos` |

---

## navidrome

| Type | Detail |
|------|--------|
| skill | `skills/navidrome/SKILL.md` |
| .mcp.json | `navidrome` → `labby mcp --services navidrome` |

---

## neo4j

| Type | Detail |
|------|--------|
| skill | `skills/neo4j/SKILL.md` |
| .mcp.json | `neo4j` → `labby mcp --services neo4j` |

---

## notebooklm

| Type | Detail |
|------|--------|
| skill | `skills/notebooklm/SKILL.md` |
| .mcp.json | `notebooklm` → `labby mcp --services notebooklm` |

---

## openacp

| Type | Detail |
|------|--------|
| monitor | `monitors/monitors.json` |
| skill | `skills/openacp/SKILL.md` |
| .mcp.json | *(empty — no servers registered)* |
| settings.json | ✓ |

---

## openai

| Type | Detail |
|------|--------|
| skill | `skills/openai/SKILL.md` |
| .mcp.json | `openai` → `labby mcp --services openai` |

---

## pihole

| Type | Detail |
|------|--------|
| skill | `skills/pihole/SKILL.md` |
| .mcp.json | `pihole` → `labby mcp --services pihole` |

---

## qdrant

| Type | Detail |
|------|--------|
| skill | `skills/qdrant/SKILL.md` |
| .mcp.json | `qdrant` → `labby mcp --services qdrant` |

---

## qmd

| Type | Detail |
|------|--------|
| skill | `skills/qmd/SKILL.md` |
| .mcp.json | `qmd` → `qmd mcp` |

---

## radicale

| Type | Detail |
|------|--------|
| skill | `skills/radicale/SKILL.md` |
| .mcp.json | `radicale` → `labby mcp --services radicale` |

---

## rag

| Type | Detail |
|------|--------|
| skill | `skills/qdrant-quality/SKILL.md` |
| skill | `skills/qdrant-vector-search/SKILL.md` |
| skill | `skills/tei/SKILL.md` |
| .mcp.json | `rag` → `labby mcp --services qdrant,tei` |

---

## rust

| Type | Detail |
|------|--------|
| skill | `skills/cargo-perf/SKILL.md` |
| skill | `skills/rust-best-practices/SKILL.md` |
| .mcp.json | *(empty — no servers registered)* |

---

## rust-bin-tools

| Type | Detail |
|------|--------|
| agent | `agents/agent-parity.md` |
| agent | `agents/aurora-reviewer.md` |
| agent | `agents/drift-detector.md` |
| agent | `agents/mcp-publisher.md` |
| agent | `agents/mcp-tool-reviewer.md` |
| agent | `agents/plugin-builder.md` |
| agent | `agents/rust-spec-reviewer.md` |
| agent | `agents/security-reviewer.md` |
| agent | `agents/sync-stack-llms.md` |
| hook | `hooks/hooks.json` |
| skill | `skills/add-domain/SKILL.md` |
| skill | `skills/agent-config/SKILL.md` |
| skill | `skills/aurora-checklist/SKILL.md` |
| skill | `skills/check-agent-parity/SKILL.md` |
| skill | `skills/check-llms-drift/SKILL.md` |
| skill | `skills/check-project-drift/SKILL.md` |
| skill | `skills/manage-llms-txt/SKILL.md` |
| skill | `skills/mcp-registry-publish/SKILL.md` |
| skill | `skills/mcp-tool-checklist/SKILL.md` |
| skill | `skills/promote-plan/SKILL.md` |
| skill | `skills/release/SKILL.md` |
| skill | `skills/security-baseline/SKILL.md` |
| skill | `skills/spec-check/SKILL.md` |
| skill | `skills/sync-claude-mds/SKILL.md` |
| skill | `skills/sync-skills/SKILL.md` |
| skill | `skills/template-init/SKILL.md` |

---

## scrutiny

| Type | Detail |
|------|--------|
| skill | `skills/scrutiny/SKILL.md` |
| .mcp.json | `scrutiny` → `labby mcp --services scrutiny` |

---

## summarize

| Type | Detail |
|------|--------|
| skill | `skills/summarize/SKILL.md` |

---

## swag

| Type | Detail |
|------|--------|
| hook | `hooks/hooks.json` |
| skill | `skills/swag/SKILL.md` |
| .mcp.json | `swag-mcp` → `uv run python -m swag_mcp` (uses `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}/.venv`, and `userConfig` vars) |
| .mcp.json | `swag-mcp-remote` → `mcp-remote ${SWAG_MCP_URL}` |

---

## sweetlink

| Type | Detail |
|------|--------|
| skill | `skills/sweetlink/SKILL.md` |

---

## syslog

| Type | Detail |
|------|--------|
| hook | `hooks/hooks.json` |
| skill | `skills/syslog/SKILL.md` |
| .mcp.json | `syslog-mcp` → `${userConfig.syslog_mcp_url}` (HTTP, Bearer auth) |

---

## tailscale

| Type | Detail |
|------|--------|
| skill | `skills/tailscale/SKILL.md` |
| .mcp.json | `tailscale` → `labby mcp --services tailscale` |

---

## tei

| Type | Detail |
|------|--------|
| skill | `skills/tei/SKILL.md` |
| .mcp.json | `tei` → `labby mcp --services tei` |

---

## tracearr

| Type | Detail |
|------|--------|
| monitor | `monitors/monitors.json` |
| skill | `skills/tracearr/SKILL.md` |
| .mcp.json | *(empty — no servers registered)* |
| settings.json | ✓ |

---

## unifi

| Type | Detail |
|------|--------|
| hook | `hooks/hooks.json` |
| skill | `skills/unifi/SKILL.md` |
| .mcp.json | `unifi` → `labby mcp --services unifi` |

---

## unraid

| Type | Detail |
|------|--------|
| hook | `hooks/hooks.json` |
| skill | `skills/unraid/SKILL.md` |
| .mcp.json | `unraid` → `labby mcp --services unraid` |

---

## uptime_kuma

| Type | Detail |
|------|--------|
| skill | `skills/uptime_kuma/SKILL.md` |
| .mcp.json | `uptime_kuma` → `labby mcp --services uptime_kuma` |

---

## vibin

| Type | Detail |
|------|--------|
| monitor | `monitors/monitors.json` |
| skill | `skills/quick-push/SKILL.md` |
| skill | `skills/save-to-md/SKILL.md` |
| .mcp.json | *(empty — no servers registered)* |
