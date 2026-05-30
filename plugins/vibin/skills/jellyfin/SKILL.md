---
name: jellyfin
description: Use when working with Jellyfin media server management, libraries, users, playback, metadata, transcoding, server health, or homelab media workflows.
---

# Jellyfin

Use this skill for Jellyfin media server workflows. Prefer an available Lab or
MCP Jellyfin integration when one is present; otherwise work through the
Jellyfin HTTP API, Docker/container inspection, or server logs.

## Workflow

1. Identify the target server, auth source, and scope of change. Do not assume a
   default server URL or admin token; ask for missing credentials instead of
   searching broadly.
2. For read-only checks, use the Jellyfin API or available MCP tools to inspect
   server info, libraries, users, active sessions, scheduled tasks, devices, and
   logs.
3. For library or metadata problems, collect the library id/name, affected item
   ids, provider ids, scan/task status, and recent Jellyfin log lines before
   recommending refreshes or metadata edits.
4. For playback or transcoding problems, gather client, stream type, codec,
   container, bitrate, subtitle mode, hardware acceleration settings, and the
   relevant transcode/log output.
5. For user/account work, confirm the exact user and requested permission
   change before applying writes.

## API Notes

- Common REST roots are `/System/Info`, `/Users`, `/Sessions`,
  `/Library/VirtualFolders`, `/Items`, `/ScheduledTasks`, and `/Devices`.
- Jellyfin typically accepts API keys through `X-Emby-Token` or
  `Authorization: MediaBrowser Token="<token>"` depending on the client path.
- Treat delete, metadata rewrite, library rescan, and user-permission changes as
  writes. Summarize the intended object ids before executing them.

## Operational Checks

- If Jellyfin is containerized, inspect the container status, mounted config and
  media paths, network reachability, and recent logs before changing settings.
- For database, plugin, or upgrade issues, back up or identify the Jellyfin
  config/data path first.
- When no MCP or authenticated API path is available, provide the exact API or
  admin-console action the user can run rather than inventing a local command.
