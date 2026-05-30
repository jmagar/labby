# sysinternals

Drive Microsoft Sysinternals CLI tools on a remote Windows machine over SSH. The same SSH-shell pattern as the [`nircmd`](../nircmd/) skill, but pointed at Mark Russinovich's signed-binary toolkit instead. Use when you need kernel handles, autorun forensics, signed-process kills (works under EDR where `taskkill` doesn't), file signature/permission audits, or anything else from the Sysinternals Suite.

Built on the [Sysinternals Suite](https://learn.microsoft.com/en-us/sysinternals/) (~100 binaries, ~50MB, Microsoft-signed).

## What it does

| Capability | One-line example |
|---|---|
| Process tree | `pslist.exe -t` |
| Kill process by name (signed, beats EDR-blocked taskkill) | `pskill.exe -t chrome.exe` *(ask-first)* |
| Find which process holds a file | `handle.exe -a stuck-file.txt` |
| List all autoruns + hashes + VT lookup | `autorunsc.exe -a * -c -h -v -nobanner /accepteula` |
| Active TCP/UDP + owning process, CSV | `tcpvcon.exe -anc` |
| Signature + hash + VT scan of a folder | `sigcheck.exe -e -u -h -c C:\Downloads` |
| Who can write to this folder | `accesschk.exe -uw C:\Program Files` |
| Recursive disk usage, CSV | `du.exe -c -l 2 C:\Users` |
| Alternate data streams in a tree | `streams.exe -s C:\Downloads` |
| Live process dump (full memory) | `procdump.exe -ma <pid> dump.dmp` *(ask-first)* |
| Run command as SYSTEM / on remote box | `psexec.exe -s cmd` *(ask-first)* |

## How it works

```
[remote Linux + Claude]                       [Win11 desktop]
       │
       │  ssh steamy-wsl  ─────────────────►  WSL Ubuntu
       │                                           │
       │                                           │  shells out to
       │                                           ▼
       │                       /mnt/c/tools/sysinternals/<tool>.exe
       │                                           │
       │                                           ▼
       │                      processes / handles / autoruns / etc.
       ▼
   Output piped back via SSH for local awk/grep/jq processing.
```

## Prerequisites

- Passwordless SSH from the Claude host to your Windows-side WSL (`ssh steamy-wsl`). Same alias as the nircmd skill — they share `NIRCMD_HOST` if `SYSINTERNALS_HOST` is unset.
- Sysinternals Suite installed at `C:\tools\sysinternals\`. One-shot install:
  ```powershell
  Invoke-WebRequest 'https://download.sysinternals.com/files/SysinternalsSuite.zip' -OutFile $env:TEMP\sys.zip
  Expand-Archive $env:TEMP\sys.zip -DestinationPath C:\tools\sysinternals -Force
  ```
- **First-run EULA** — every Sysinternals tool blocks on an EULA prompt on first invocation per user. Pass `/accepteula` once per tool (sets `HKCU\Software\Sysinternals\<Tool>\EulaAccepted = 1`); subsequent runs are silent. The skill always passes it as a safety belt.

## Pointing at a different machine

```json
{
  "env": {
    "SYSINTERNALS_HOST": "workbox",
    "SYSINTERNALS_DIR":  "/mnt/c/tools/sysinternals"
  }
}
```

in `~/.claude/settings.json`. If unset, falls back to `NIRCMD_HOST` so the two skills share the same target by default.

## Safety

Three tiers, see `SKILL.md`:

- **Auto-allowed**: pure read-only inspection (`pslist`, `tcpvcon`, `handle`, `autorunsc`, `sigcheck` without VT, `du`, `accesschk`, `streams` read).
- **Ask first**: state-changing or data-leaking — `pskill`, `procdump`, `handle -c`, `streams -d`, `sigcheck -v` (uploads hashes to VirusTotal), bulk `autorunsc` deletes.
- **Refuse without explicit instruction**: `psexec` against other machines, anything that runs as another user without clear scope.

## Sibling skills

| Skill | When to prefer |
|---|---|
| [nircmd](../nircmd/) | Side-effecting actions: clipboard, screenshot, lock, audio, window control |
| nircmd / NirSoft companion | Activity log, browser history, Wi-Fi scan |
| **sysinternals (this)** | Kernel handles, autoruns, signed-process control, signature/permission audits |

## Files

```
sysinternals/
├── SKILL.md
├── README.md
└── references/
    └── tool-catalog.md       Full Sysinternals CLI catalog with all flags
```
