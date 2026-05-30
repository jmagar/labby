# Changelog

All notable changes to the `nircmd` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.2.0] - 2026-05-17
- Extended scope to cover NirSoft companion CLIs alongside NirCmd. The skill now triggers on questions about Windows-side state (open ports, Wi-Fi APs, recent activity, open file handles, etc.), not just NirCmd actions.
- Added `NIRSOFT_DIR` env var (default `/mnt/c/tools/nirsoft`) and documented `NirLauncher` as a one-shot bundle for all 200+ tools.
- New `## NirSoft companion tools` section in SKILL.md with the most-useful 8 tools, each with the single flag combo that matters and a round-trip CSV-fetch example.
- New `references/nirsoft-tools.md`: CLI-friendly catalog (CurrPorts, WirelessNetView/WirelessNetConsole, WifiInfoView, DNSQuerySniffer, NetworkTrafficView, LastActivityView, TurnedOnTimesView, ExecutedProgramsList, BrowsingHistoryView, OpenedFilesView, ProcessActivityView, InstalledDriversList, SearchMyFiles, AlternateStreamView, FolderTimeUpdate) with documented flags, example invocations, and the temp-file → SSH cat-back pattern.
- Explicit out-of-scope note for credential-dumping tools (WebBrowserPassView, Mail PassView, WirelessKeyView): treat any user request the same as `runas` / registry writes — ask first, no auto-fetch.
- README + capability table + install steps + `env` block + files tree all updated to match. No script changes (the round-trip pattern is documented inline rather than wrapped; existing scripts are NirCmd-specific and stay as-is).


## [0.1.1] - 2026-05-17
- Discovered: `nircmd clipboard set` / `readfile` / `writefile` are hardcoded to CP-1252 ANSI (CF_TEXT) and silently lose anything outside that codepage (emoji become `?`, CJK becomes `???`). Em-dash survives because it's in CP-1252.
- Rewrote `scripts/clip.sh` to route non-ASCII / multi-line content through PowerShell's `Set-Clipboard` (which uses CF_UNICODETEXT and is lossless). ASCII single-line still uses fast `nircmd clipboard set`.
- Updated `references/clipboard.md` to document the limitation and the PowerShell escape hatch; warned that `clipboard writefile` lies when verifying Unicode pushes (also outputs CP-1252).
- Verified: full round-trip of `em-dash — emoji 🎉 CJK 日本語 curly 'quotes' + multiline` survives intact when read back via `Get-Clipboard` with UTF-8 console encoding.

## [0.1.0] - 2026-05-17
- Initial release.
- Installed NirCmd x64 to `C:\tools\nircmd\nircmd.exe` on the default host (`steamy-wsl`).
- SKILL.md with three-tier safety model (auto / ask / refuse), charset gotcha documented, defaults block at top.
- Scripts: `clip.sh` (UTF-8-safe text/file push), `clip-grab.sh` (text+image), `win-shot.sh` (activate+capture by title), `lock.sh`.
- References: full categorized command reference (115 commands), clipboard patterns, window control patterns, safety boundaries.
- Verified end-to-end: pushed text from SSH session to Windows clipboard and read it back via PowerShell.
