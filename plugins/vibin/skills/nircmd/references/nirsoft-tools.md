# NirSoft companion tools

NirCmd's sibling utilities from the same author (Nir Sofer). All free, all portable single .exe files, all share the same SSH-shell invocation pattern as NirCmd. Most expose CLI flags for non-interactive use — the table below covers the ones that are *actually* scriptable.

> **Verification note**: NirCmd itself is round-trip-verified on the default host. The flags below are taken from NirSoft's documentation pages; the path conventions and example output assume a standard install. Sanity-check on first use.

## Path convention

By default the skill expects companions under `C:\tools\nirsoft\<tool>.exe`. Override per-tool or globally:

```bash
NIRSOFT_DIR="${NIRSOFT_DIR:-/mnt/c/tools/nirsoft}"        # POSIX path from WSL
# or, with the full NirLauncher bundle:
NIRSOFT_DIR="/mnt/c/tools/NirLauncher/NirSoft"
```

Universal invocation:
```bash
ssh "$NIRCMD_HOST" "$NIRSOFT_DIR/<tool>.exe <flags...>"
```

## Bulk install — NirLauncher

The fastest way to land all 200+ NirSoft tools at once:

```powershell
# On the Windows host:
Invoke-WebRequest 'https://launcher.nirsoft.net/downloads/nirlauncher.zip' -OutFile $env:TEMP\nl.zip
Expand-Archive $env:TEMP\nl.zip -DestinationPath C:\tools\NirLauncher -Force
# Tools end up at C:\tools\NirLauncher\NirSoft\<ToolName>.exe
```

Otherwise grab individual zips from `https://www.nirsoft.net/utils/<tool>.html` and drop them in `C:\tools\nirsoft\`.

## CLI-friendly catalog

All `/scomma`, `/stab`, `/sxml`, `/shtml` flags produce *quiet* output to the named file (no GUI, no popups) and exit when done. Substitute Windows-style paths with `C:\` prefix.

### Network

| Tool | Useful flags | Example |
|---|---|---|
| **CurrPorts** (`cports.exe`) | `/scomma <out>`, `/sxml <out>`, `/close <lip> <lport> <rip> <rport>`, `/CloseProcessPorts <name>`, `/CloseProcessIDPorts <pid>` | `cports.exe /scomma C:\out\ports.csv` |
| **WirelessNetView** | `/scomma`, `/sxml`, `/shtml` | `WirelessNetView.exe /scomma C:\out\wifi.csv` |
| **WirelessNetConsole** | (no flags — streams to stdout) | `WirelessNetConsole.exe` |
| **WifiInfoView** | `/scomma`, `/sxml`, `/AllowOnlySingleInstance` | `WifiInfoView.exe /scomma C:\out\aps.csv` |
| **DNSQuerySniffer** | `/scomma`, `/sxml`, `/Capture <iface-name>`, `/StopCapture` (with running instance) | `DNSQuerySniffer.exe /scomma C:\out\dns.csv` |
| **NetworkTrafficView** | `/scomma`, `/sxml`, `/StartCapture <iface>`, `/StopCapture` | `NetworkTrafficView.exe /StartCapture "Wi-Fi"` |

### System forensics — "what just happened"

| Tool | Useful flags | Example |
|---|---|---|
| **LastActivityView** | `/scomma`, `/stab`, `/sxml`, `/showtype <bitmask>` | `LastActivityView.exe /scomma C:\out\activity.csv` |
| **TurnedOnTimesView** | `/scomma`, `/sxml`, `/ProcessEventLog` | `TurnedOnTimesView.exe /scomma C:\out\uptime.csv` |
| **ExecutedProgramsList** | `/scomma`, `/sxml`, `/DataSource <0-3>` | `ExecutedProgramsList.exe /scomma C:\out\exec.csv` |
| **BrowsingHistoryView** | `/HistorySource <1-4>`, `/HistorySourceFolder`, `/VisitTimeFilterType <n>`, `/scomma` | `BrowsingHistoryView.exe /HistorySource 1 /scomma C:\out\hist.csv` |

`HistorySource`: 1=current user / 2=all users / 3=external profiles folder / 4=external single profile.

### Process / handles

| Tool | Useful flags | Example |
|---|---|---|
| **OpenedFilesView** (`OpenedFilesView.exe`) | `/scomma`, `/processfilter <pid-or-name>`, `/filefilter <substring>`, `/showownfiles 0\|1` | `OpenedFilesView.exe /filefilter "C:\stuck\file" /scomma C:\out\holders.csv` |
| **ProcessActivityView** | `/scomma`, `/StartCapture <pid-or-name>`, `/StopCapture` | `ProcessActivityView.exe /StartCapture explorer.exe` |
| **InstalledDriversList** | `/scomma`, `/sxml`, `/showtype <n>` | `InstalledDriversList.exe /scomma C:\out\drivers.csv` |

### Files / disk

| Tool | Useful flags | Example |
|---|---|---|
| **SearchMyFiles** | `/cfg <saved.cfg>`, `/scomma`, `/Start`, `/SearchSubFolders 1\|0` | `SearchMyFiles.exe /cfg C:\saved\find-large.cfg /scomma C:\out\hits.csv` |
| **AlternateStreamView** | `/scomma`, `/scanfolder <path>` | `AlternateStreamView.exe /scanfolder C:\Downloads /scomma C:\out\ads.csv` |
| **FolderTimeUpdate** | `/folderpath <path>`, `/IncludeSubfolders 1`, `/TimeMode <n>`, `/Start` | `FolderTimeUpdate.exe /folderpath C:\restored /Start` |

## Round-trip patterns

The natural pipeline is **invoke on Windows → fetch CSV back via SSH → process locally**:

```bash
# What's holding TCP port 5000?
WIN_OUT='C:\Users\jmaga\AppData\Local\Temp\ports.csv'
ssh "$NIRCMD_HOST" "$NIRSOFT_DIR/cports.exe /scomma '$WIN_OUT'"
ssh "$NIRCMD_HOST" "cat '/mnt/c/Users/jmaga/AppData/Local/Temp/ports.csv'" \
  | awk -F, '$3==5000 {print}'

# Did chrome.exe run today and when?
WIN_OUT='C:\Users\jmaga\AppData\Local\Temp\activity.csv'
ssh "$NIRCMD_HOST" "$NIRSOFT_DIR/LastActivityView.exe /scomma '$WIN_OUT'"
ssh "$NIRCMD_HOST" "cat '/mnt/c/Users/jmaga/AppData/Local/Temp/activity.csv'" \
  | grep -i chrome.exe
```

The temp-file dance is the same one used for clipboard UTF-8 — see `references/clipboard.md`.

## Privacy / safety notes

NirSoft also publishes **credential-dumping tools** (WebBrowserPassView, Mail PassView, WirelessKeyView, etc.). These are intentionally *not* covered here — they require Administrator and AV often blocks them. If the user explicitly asks for one, treat it the same as `runas`/registry writes: **ask first**, document the target file, and never auto-fetch the result without explicit consent. They're legitimate forensics/recovery tools but the blast radius (passwords on disk readable from SSH) is unique.

## Where to find one not listed here

`https://www.nirsoft.net/utils/index.html` — full alphabetized index. Each tool's page has a "Command-Line Options" section near the bottom; if it's there, it's scriptable.
