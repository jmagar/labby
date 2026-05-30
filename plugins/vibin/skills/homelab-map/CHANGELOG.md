# Changelog

All notable changes to this skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## 2026-05-23

### Added
- Added `scripts/generate-homelab-report.py` to generate `~/.homelab/homelab.md` from live SSH, Docker, ZFS, Unraid, and SWAG config checks.
- Added a static report template at `references/homelab.md`.
- Added generated `~/.homelab/homelab.json` structured inventory output.
- Added generated `~/.homelab/index.html` browser viewer for the JSON inventory.
- Added best-effort viewer serving: local HTTP plus Tailscale Serve only after `tailscale status` succeeds.

### Changed
- Converted `references/homelab.md` from a manually maintained snapshot into a static report template.
- Changed the generator default output to `~/.homelab/homelab.md` so volatile runtime snapshots do not dirty the repository.
- Updated `SKILL.md` and `README.md` to distinguish the repo template from the generated runtime report artifacts.
- Made Tailscale exposure optional and non-fatal; report generation no longer depends on tailnet serving.
- Changed the default local viewer port from `8787` to `40500`.
- Changed the default viewer bind address to `0.0.0.0` so SWAG can proxy dookie's Tailscale IP on port `40500`.

## 2026-05-17

### Added
- Initial CHANGELOG.
