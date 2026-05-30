# Sysinternals CLI tool catalog

Comprehensive reference for the CLI-friendly subset of the Sysinternals Suite. GUI-only tools (Process Explorer, Process Monitor, TCPView, Autoruns GUI) are omitted — use their CLI counterparts (`tcpvcon`, `autorunsc`, `procmon` with `/AcceptEula /Quiet /BackingFile`) for scripting.

## Globally applicable flags

| Flag | Effect |
|---|---|
| `/accepteula` | Skip the EULA prompt (sets HKCU registry key) |
| `-nobanner` | Suppress the "Sysinternals — Mark Russinovich" credit line |
| `-?` | Per-tool help (most tools); some use `/?` |

## Catalog

### Process / threads

```
pslist.exe [-d] [-m] [-x] [-t] [-s [n]] [-r n] [name|pid]
  -d  thread details
  -m  memory details
  -x  combined (processes, memory, threads)
  -t  process tree
  -s  task-manager-style continuous mode, n-sec refresh
  -r  refresh count for -s

pskill.exe [-t] [pid|name]
  -t  kill the entire process tree

psloggedon.exe [-l] [-p] [-x] [user|machine]
  -l  local logons only (no resource share users)
  -x  no welcome banner

listdlls.exe [process|pid] [-r] [-u] [-v] [-d <dll>]
  -r  show only relocated DLLs (security-relevant)
  -u  show only unsigned DLLs
  -v  version info
  -d  list processes that have loaded the named DLL

handle.exe [-a] [-c <handle> [-y] [-p <pid>]] [-l] [-s] [-u] [-p <pid>] [name]
  -a  all handle types (default is file-only)
  -c  close handle by id (DANGEROUS — pass -y to skip confirm)
  -l  pagefile-backed sections only
  -s  count per-type
  -u  show owning user

procdump.exe [-ma|-mp|-mh|-mt|-mm] <pid|name> [out.dmp]
  -ma full memory dump
  -mp full + private working set only
  -mm minimal (mainly stack)
  Also: -p <perf-counter>:<threshold>  -e (unhandled exception trigger)
       -c <cpu%> -m <mb> -t (terminate after dump)
```

### Network

```
tcpvcon.exe [-a] [-c|-n] [process-name|pid]
  -a  show all connections + listening
  -c  CSV (with header)
  -n  no DNS resolution (faster)

psping.exe <target>[:<port>] [-t [-q|-i 0]] [-l <size>] [-n <count>]
  TCP latency, bandwidth (-b), or ICMP (default).
  Server mode: psping.exe -s <ip>:<port>
```

### Autoruns / persistence

```
autorunsc.exe [-a <bdeghiklmnoprstw*>] [-c] [-h] [-s] [-v[rs]]
              [-z <SystemRoot> <UserProfile>] [user]
  -a  category filter (* = all). See SKILL.md for letter codes.
  -c  CSV output
  -h  cryptographic hashes (MD5, SHA1, SHA256)
  -s  verify signatures (slower)
  -v  query VirusTotal (rs = rescan known-bad)
  -m  hide Microsoft-signed entries
  -t  show timestamps
```

### Files / signatures / streams / disk

```
sigcheck.exe [-a] [-c|-ct] [-e] [-h] [-i] [-l] [-m] [-n] [-nobanner]
             [-q] [-r] [-s] [-u] [-v[r|s]] [-vt] [path|file]
  -a  show full version info
  -c  CSV (-ct = tab-separated)
  -e  scan executables only (PE files)
  -h  hashes
  -i  show catalog signers
  -m  show MFT manifest
  -n  show only file version
  -r  disable cert revocation checks
  -s  recurse subdirectories
  -u  show unsigned files only
  -v  query VirusTotal (r = rescan if known)
  -vt accept the VT terms

streams.exe [-s] [-d] [-nobanner] <file-or-dir>
  -s  recurse
  -d  delete streams (DESTRUCTIVE)

du.exe [-c|-ct] [-l n] [-n] [-nobanner] [-q] <path>
  -c  CSV
  -ct tab-separated
  -l  recurse N levels
  -n  no header
  -q  quiet (suppress errors)

accesschk.exe [-a] [-c] [-d] [-e] [-k] [-h] [-l] [-n] [-p [-f] [-t]]
              [-q] [-r] [-s] [-u] [-v] [-w] [user|group] <path|object>
  -d  match directories only
  -e  show only explicit perms (not inherited)
  -f  with -p: full process info
  -k  registry key
  -p  process
  -q  no banner
  -r  show read access only
  -s  recurse
  -u  suppress errors
  -v  verbose (specific access rights)
  -w  show write access only
```

### Remote / elevation

```
psexec.exe [\\computer[,...]] [-u user [-p pass]] [-s|-e|-x] [-i [session]]
           [-d] [-l] [-h] [-w <dir>] [-c [-f|-v]] [-r <name>] [-n <sec>]
           <command> [args]
  -s  run as SYSTEM
  -e  no profile load (faster)
  -i  interactive (allows UI)
  -h  elevated (UAC)
  -d  don't wait for completion
  -c  copy <command> to remote first; -f overwrite, -v if newer
  -n  network timeout

Use cases:
  psexec.exe -s -i cmd               # SYSTEM-level interactive shell (local)
  psexec.exe \\host -u DOMAIN\admin -p PASS ipconfig   # Remote run
  psexec.exe -i 1 -d notepad         # Run in session 1, detach
```

## CSV-out cheat sheet

These produce machine-readable output directly — no temp file dance needed:

```bash
tcpvcon.exe -anc                                     # CSV with header
autorunsc.exe -a * -c -h -nobanner /accepteula      # CSV with hashes
sigcheck.exe -e -h -c -nobanner -accepteula <path>  # CSV per file
du.exe -c -nobanner -accepteula <path>              # CSV size summary
accesschk.exe -q -nobanner /accepteula <perms>      # No banner; not CSV but parseable
```

Use the temp-file pattern from SKILL.md only for tools that don't support `-c`/`/c`.

## What's not here

- **Process Monitor** (`procmon.exe /AcceptEula /Quiet /BackingFile <out.pml>`) — captures every file/registry/network event. Massive output, niche use. Run interactively unless you know exactly what filter you need.
- **PsExec to other machines** — covered under safety tier 3. Always requires explicit user intent + target confirmation.
- **GUI-only tools** (Process Explorer, ZoomIt, BgInfo) — outside scope.
