---
name: sysinternals
description: Drive Microsoft Sysinternals CLI tools on a remote Windows machine over SSH — list/kill processes (pslist, pskill), inspect open handles (handle), audit autoruns (autorunsc), check TCP/UDP connections (tcpvcon), verify file signatures + VirusTotal (sigcheck), audit permissions (accesschk), find alt data streams (streams), measure disk usage (du), check who's logged in (psloggedon), and run remotely (psexec). Use whenever the user wants kernel-level handle inspection, signed-binary process control under EDR, autorun forensics, file signature verification, security/permission audits, or anything from Mark Russinovich's suite. Sibling to the nircmd / NirSoft skill — prefer Sysinternals when the answer needs Microsoft-signed tooling (works under stricter AV, deeper system access) or covers kernel handles, autoruns, or signature/permission audits. Defaults to `ssh steamy-wsl` and `C:\tools\sysinternals\`; override via `SYSINTERNALS_HOST` and `SYSINTERNALS_DIR` env vars.
---

# sysinternals

Bridge for driving Mark Russinovich's Sysinternals Suite on a remote Windows box over SSH. Microsoft-signed (often the *only* thing that works under strict EDR), broader system access than NirSoft, all CLI-friendly with CSV output.

## Defaults (override via env vars)

```bash
SYSINTERNALS_HOST="${SYSINTERNALS_HOST:-${NIRCMD_HOST:-steamy-wsl}}"   # ssh alias
SYSINTERNALS_DIR="${SYSINTERNALS_DIR:-/mnt/c/tools/sysinternals}"      # POSIX path from WSL
```

For persistence across sessions, set in `~/.claude/settings.json` under `env`.

## Universal invocation pattern

```bash
ssh "$SYSINTERNALS_HOST" "$SYSINTERNALS_DIR/<tool>.exe <args> /accepteula" [-nobanner]
```

**Always include `/accepteula`** on the first run of any tool — Sysinternals tools block on an EULA prompt otherwise (it sets a registry key after first accept; subsequent runs are silent). Most tools accept `-nobanner` to suppress the credit line.

## Install (one-shot)

```powershell
# On the Windows host
Invoke-WebRequest 'https://download.sysinternals.com/files/SysinternalsSuite.zip' -OutFile $env:TEMP\sys.zip
Expand-Archive $env:TEMP\sys.zip -DestinationPath C:\tools\sysinternals -Force
```

~100 binaries, ~50MB. No installer, fully portable.

## Most-used tools

### Process inspection / control

| Tool | Useful flags | Example |
|---|---|---|
| `pslist.exe` | `-t` (tree), `-x` (memory + threads), `<name>` to filter | `pslist.exe -t chrome` |
| `pskill.exe` | `<pid>` or `<name>`, `-t` (kill tree) | `pskill.exe -t chrome.exe` — **ask first** |
| `psloggedon.exe` | `-l` (local only), `-x` (no welcome) | `psloggedon.exe -x` |
| `handle.exe` | `-p <pid>`, `-a` (all types), `-c <handle>` (close — risky), `<substring>` (search) | `handle.exe -a stuck-file.txt` |
| `listdlls.exe` | `<process-name>`, `-r` (relocated), `-u` (unsigned) | `listdlls.exe -u` |
| `procdump.exe` | `-ma <pid> <out.dmp>` (full mem dump), `-mp <pid>` (mini) | **ask first** — large files |

### Network

| Tool | Useful flags | Example |
|---|---|---|
| `tcpvcon.exe` | `-a` (all + CSV), `-c` (CSV no header), `-n` (no DNS) | `tcpvcon.exe -anc` |
| `psping.exe` | `-t <target>` (TCP), `-l <size>` | `psping.exe 8.8.8.8:443` |

### Autoruns / persistence

| Tool | Useful flags | Example |
|---|---|---|
| `autorunsc.exe` | `-a *` (all categories), `-c` (CSV), `-h` (hashes), `-s` (signed status), `-v` (VirusTotal) | `autorunsc.exe -a * -c -h -nobanner /accepteula > out.csv` |

`-a` categories: `b` boot, `d` AppInit DLLs, `e` explorer addons, `g` sidebar gadgets, `h` image hijacks, `i` IE addons, `k` known DLLs, `l` logon, `m` WMI, `n` Winsock, `o` codecs, `p` printer monitors, `r` LSA providers, `s` services, `t` scheduled tasks, `w` Winlogon. `*` = all.

### Files / signatures / streams

| Tool | Useful flags | Example |
|---|---|---|
| `sigcheck.exe` | `-h` (hashes), `-c` (CSV), `-v r` (VirusTotal — sends hashes), `-u` (unsigned only), `-e` (executables only) | `sigcheck.exe -e -u -h -c C:\Users\jmaga\Downloads` |
| `streams.exe` | `-s` (recurse), `-d` (delete — **ask first**) | `streams.exe -s C:\Downloads` |
| `du.exe` | `-c` (CSV), `-l <levels>`, `-n` (no header) | `du.exe -c -l 2 C:\Users` |
| `accesschk.exe` | `-u` (no errors), `-w` (write access only), `-s` (recurse), `-q` (quiet) | `accesschk.exe -uw "Authenticated Users" C:\Program Files` |

### Remote / elevation — **ask first tier**

| Tool | Notes |
|---|---|
| `psexec.exe -s -i <cmd>` | Run as SYSTEM interactively. Audited by EDR as suspicious. |
| `psexec.exe \\host -u user -p pass <cmd>` | Run on another machine. Same audit profile. |

## Safety boundaries

| Tier | Tools |
|---|---|
| **Auto-allowed** (read-only) | `pslist`, `psloggedon`, `handle` (no `-c`), `listdlls`, `tcpvcon`, `psping`, `autorunsc` (read), `sigcheck` (without `-v`), `streams` (read), `du`, `accesschk` |
| **Ask first** | `pskill`, `handle -c` (close handle), `procdump`, `streams -d`, `sigcheck -v` (sends hashes to VirusTotal — may leak filenames in metadata), `autorunsc -m` |
| **Refuse without extremely explicit instruction** | `psexec` to other hosts, anything `-accepteula` is being used to bulk-deploy under another user |

## Round-trip output pattern

Same as the NirSoft companions — write CSV on Windows, `cat` back over SSH:

```bash
WIN_OUT='C:\Users\jmaga\AppData\Local\Temp\handles.csv'
POSIX_OUT='/mnt/c/Users/jmaga/AppData/Local/Temp/handles.csv'
ssh "$SYSINTERNALS_HOST" "$SYSINTERNALS_DIR/handle.exe -a -nobanner /accepteula > '$WIN_OUT'"
ssh "$SYSINTERNALS_HOST" "cat '$POSIX_OUT'" | grep -i stuck-file
```

Some tools emit CSV directly to stdout (`tcpvcon -anc`, `autorunsc -c`, `sigcheck -c`) — you can skip the temp file and pipe straight to local awk/jq.

## When to pick this vs nircmd/NirSoft

| Need | Tool |
|---|---|
| Push to clipboard / lock / TTS / window control | **nircmd** |
| What ran on this box / browser history / Wi-Fi APs | **NirSoft** (LastActivityView, BrowsingHistoryView, WirelessNetView) |
| Kernel handles, autoruns, signed-binary process control | **Sysinternals** (handle, autorunsc, pskill) |
| Open TCP ports | Either (`cports` is friendlier UI, `tcpvcon -anc` is signed) |
| File signatures / permissions audit | **Sysinternals** (sigcheck, accesschk) |

## References

- `references/tool-catalog.md` — full categorized list with all documented flags
