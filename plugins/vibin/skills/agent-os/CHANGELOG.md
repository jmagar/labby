# Changelog

All notable changes to this skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## 2026-05-23

### Changed

- Renamed the skill from `winbox` to `agent-os`; `winbox` remains a legacy trigger phrase only.
- Added the shared web-dev priority ladder: CDP on agent-os, agent-browser, claude-in-chrome on agent-os, agent-os Windows-MCP, then claude-in-chrome on steamy.
- Updated docs and examples to use `agent-os` as the primary name.

## 2026-05-17 (later — naming + side-channel correction)

### Changed

- **Clarified the VM's official name.** The sandbox is officially the **`agent-os`** VM (container `agent-os-win11`, image `dockur/windows`) on host `dookie`. At the time, `winbox` remained the skill name; on 2026-05-23 the skill was renamed to `agent-os` and `winbox` became a legacy trigger phrase only.
- Replaced all `docker ps | grep windows` / `docker inspect windows` references with the correct container name, `agent-os-win11`.
- Promoted **SSH on `dookie:2222`** (host → guest port 22) from "not verified" to "confirmed exposed by the container" in the side-channel list. Whether sshd is actually answering inside the guest depends on first-boot provisioning, but the port forward is real.

## 2026-05-17

### Changed

- **Switched primary interaction surface from noVNC + `agent-browser` to Windows-MCP.** [CursorTouch/Windows-MCP](https://github.com/CursorTouch/Windows-MCP) is now installed inside the agent-os VM and exposed as an HTTP MCP server (registered in `~/.claude.json` as `windows-mcp`). All workflows now go through the `mcp__windows-mcp__*` tools.
- Rewrote SKILL.md around the new tool surface: `App`, `Click`, `Move`, `Scroll`, `Type`, `Shortcut`, `MultiSelect`, `MultiEdit`, `Screenshot`, `Snapshot`, `PowerShell`, `Clipboard`, `FileSystem`, `Process`, `Registry`, `Notification`, `Wait`, `Scrape`.
- Rewrote README.md to summarize the migration and clarify why Windows-MCP is preferred over the legacy path.
- Updated frontmatter trigger phrases to cover Windows-MCP-style requests ("PowerShell on the winbox", "run X on the winbox", "screenshot the winbox") in addition to the prior triggers, while keeping noVNC mentions for backward-compatibility.

### Removed

- The bash helpers `winbox_click` and `winbox_type` that dispatched `MouseEvent`s and per-character `press`es against the noVNC canvas. They're obsolete with `Click` / `Type` / `PowerShell`. Available in this file's git history if ever needed.
- The "Keystroke gotchas" section documenting noVNC-specific bugs (`keyboard type` no-op, `Shift+<digit>` unreliable, dropped Shift modifier on uppercase letters). Windows-MCP's `Type` handles all of these correctly.
- The "Bypassing the GUI" section's emphasis on `Meta+r` → `winbox_type "powershell"`. Direct `PowerShell` invocation through Windows-MCP supersedes it.

### Kept (still relevant)

- Notes about the `/oem` SMB drop folder (install-time only — gone after first boot).
- RDP on `dookie:33890` as a future option if heavy interactive sessions are ever needed (now a weaker case since Windows-MCP exists).
- noVNC at `http://dookie:8006` retained as a visual debugging fallback only.

### Migration note

If you were calling the old `winbox_click` or `winbox_type` shell helpers from another skill or recipe, swap them for `mcp__windows-mcp__Click {x, y}` and `mcp__windows-mcp__Type {text}` respectively. For any task expressible as a shell command, prefer `mcp__windows-mcp__PowerShell` over GUI automation entirely.

## 2026-05-17 (earlier — pre-migration)

### Added

- Initial CHANGELOG.
