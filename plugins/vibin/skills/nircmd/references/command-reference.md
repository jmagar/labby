# nircmd — Command Reference (all 115 commands)

Every NirCmd command, categorized. For canonical syntax/args, run `nircmd <command>` with no args (opens a GUI help dialog locally — over SSH this hangs, so prefer this doc + the bundled `C:\tools\nircmd\NirCmd.chm` for deep-dive).

For safety classification (auto / ask first / refuse) see `safety-boundaries.md`.



## Audio

| Command | Effect |
|---------|--------|
| `setsysvolume <0-65535>` | Set master volume |
| `setsysvolume2 <0-65535> <device>` | Set on specific device |
| `setvolume <device-id> <left> <right>` | Per-channel |
| `changesysvolume <delta>` | Bump volume up/down |
| `changesysvolume2 <delta> <device>` | Bump on specific device |
| `mutesysvolume <1=mute, 0=unmute, 2=toggle>` | Master mute |
| `setappvolume <app.exe> <0.0-1.0>` | Per-app volume |
| `changeappvolume <app.exe> <delta>` | Per-app delta |
| `muteappvolume <app.exe> <1/0/2>` | Per-app mute |
| `setsubunitvolumedb <dB>` | Subunit (advanced) |
| `mutesubunitvolume <1/0/2>` | Subunit mute |
| `setdefaultsounddevice "<name>"` | Set default playback device |
| `showsounddevices` | List sound devices |
| `mediaplay <0=toggle, 1=play, 2=pause, 3=stop>` | Media transport |
| `speak text "<text>"` | TTS |
| `speak file "C:\file.txt"` | TTS from file |
| `beep <freq> <duration_ms>` | Generate tone |
| `stdbeep` | Short system beep |

## Display / monitor

| Command | Effect |
|---------|--------|
| `setbrightness <0-100>` | Set monitor brightness |
| `changebrightness <delta>` | Bump brightness |
| `setdisplay <w> <h> <bpp> <hz>` | Change resolution / refresh |
| `setprimarydisplay <index>` | Make a monitor primary |
| `monitor on/off/standby/low` | Power state |
| `screensaver` | Start screensaver |
| `screensavertimeout <seconds>` | Set timeout |

## Window & input

| Command | Effect |
|---------|--------|
| `win <action> <find> "<target>" [args]` | See `window-control.md` for full action list |
| `sendkey <key> <down/up/press>` | Single key event |
| `sendkeypress <combo>` | Send full combo (e.g. `ctrl+s`) |
| `sendmouse <button> <action>` | Mouse button event |
| `setcursor <x> <y>` | Move mouse |
| `setcursorwin <relx> <rely>` | Move mouse relative to active window |
| `movecursor <dx> <dy>` | Relative mouse move |

## Session / power

| Command | Effect | Safety |
|---------|--------|--------|
| `lockws` | Lock workstation | auto |
| `exitwin logoff/poweroff/reboot/shutdown` | End session | **refuse** |
| `exitwin <action> force` | Force variant | **refuse** |
| `standby` | Sleep | **refuse** |
| `hibernate` | Hibernate | **refuse** |
| `initshutdown <message> <timeout> <force> <reboot>` | Scheduled shutdown | **refuse** |
| `abortshutdown` | Cancel pending shutdown | ask first |

## Process

| Command | Effect | Safety |
|---------|--------|--------|
| `killprocess <name>` | Force-kill matching process | ask first |
| `closeprocess <name>` | Graceful close | ask first |
| `suspendprocess <name>` | Suspend (pause) process | ask first |
| `waitprocess <name>` | Block until process exits | auto |
| `setprocessaffinity <name> <mask>` | CPU affinity | ask first |
| `setprocesspriority <name> <level>` | Process priority | ask first |
| `runas <user> <pass> <prog> <args>` | Run as user | ask first |
| `runassystem <prog> <args>` | Run as SYSTEM | ask first |
| `runinteractive <prog> <args>` | Run in user session | ask first |
| `runinteractivecmd <cmd>` | Same for cmd | ask first |
| `elevate <prog> <args>` | UAC-elevate | ask first |
| `elevatecmd <cmd>` | UAC-elevate cmd | ask first |
| `exec show/hide/min/max "<prog>" <args>` | Launch program | ask first |
| `exec2 <wait> <show> <prog> <args>` | Launch + wait | ask first |
| `execmd <cmd>` | Run cmd line | ask first |
| `shexec <verb> <file>` | Shell-execute (open/edit/print) | ask first |
| `cmdwait <ms> <cmd>` | Run cmd after delay | ask first |

## Registry

| Command | Effect | Safety |
|---------|--------|--------|
| `regsetval <type> <key> <name> <data>` | Write registry value | ask first |
| `regdelval <key> <name>` | Delete value | ask first |
| `regdelkey <key>` | Delete key | ask first |
| `regsvr <register/unregister> "<dll>"` | (Un)register COM DLL | ask first |
| `regedit "<key>"` | Open regedit at key | ask first (opens GUI) |

## INI files

| Command | Effect |
|---------|--------|
| `inisetval "<file>" "<section>" "<key>" "<value>"` | Set INI value |
| `inidelval "<file>" "<section>" "<key>"` | Delete INI value |
| `inidelsec "<file>" "<section>"` | Delete INI section |

## Service control

| Command | Effect | Safety |
|---------|--------|--------|
| `service start/stop/restart/pause/continue/auto/demand/disabled "<name>"` | Service control | ask first |

## Files & shell

| Command | Effect | Safety |
|---------|--------|--------|
| `shellcopy <src> <dst> <opts>` | Shell copy (with UI progress) | auto if explicit |
| `filldelete "<path>"` | Secure-delete (multi-pass overwrite) | ask first |
| `emptybin <drive>` | Empty Recycle Bin | ask first |
| `moverecyclebin "<path>"` | Send to Recycle Bin | ask first |
| `clonefiletime <src> <dst>` | Copy timestamps src→dst | auto |
| `setfiletime <file> <created> <modified> <accessed>` | Set timestamps | auto |
| `setfilefoldertime <path> ...` | Set on file or folder | auto |
| `shellrefresh` | Refresh Explorer | auto |
| `restartexplorer` | Restart Explorer (taskbar reloads) | ask first |
| `sysrefresh environment` | Refresh env vars / icons | auto |

## Dialogs & notifications

| Command | Effect |
|---------|--------|
| `infobox "<body>" "<title>"` | Info popup |
| `qbox "<question>" "<title>" "<cmd-on-yes>"` | Yes/No prompt (Yes runs cmd) |
| `qboxtop "<question>" "<title>" "<cmd>"` | Same but always-on-top |
| `qboxcom "<question>" "<title>" <cmd-id>` | COM-callback variant |
| `qboxcomtop ...` | Always-on-top variant |
| `dlg "<title>" <type> "<prompt>"` | Generic dialog |
| `dlgany "<title>" <type> ...` | Extended dialog |
| `trayballoon "<title>" "<body>" "<icon>" <ms>` | Tray notification balloon |
| `cdrom open/close <drive>` | CD/DVD tray |

## Shortcuts

| Command | Effect |
|---------|--------|
| `shortcut "<target>" "<folder>" "<name>" [args] [icon] [hotkey] [show]` | Create `.lnk` |
| `urlshortcut "<url>" "<folder>" "<name>"` | Create `.url` |
| `cmdshortcut "<folder>" "<name>" <nircmd-cmd>` | NirCmd command as shortcut |
| `cmdshortcutkey "<folder>" "<name>" <hotkey> <cmd>` | + global hotkey |

## Console

| Command | Effect |
|---------|--------|
| `consolewrite "<text>"` | Write to console |
| `setconsolecolor <fg> <bg>` | Set console colors |
| `setconsolemode <mode>` | Set console mode |
| `debugwrite "<text>"` | Write to debugger (OutputDebugString) |

## Network

| Command | Effect | Safety |
|---------|--------|--------|
| `rasdial "<entry>"` | Dial RAS/VPN entry | ask first |
| `rashangup "<entry>"` | Hang up | ask first |
| `rasdialdlg "<entry>"` | Open dial dialog | ask first |
| `setdialuplogon <entry> <user> <pass>` | Set creds | ask first |
| `inetdial "<entry>"` | Internet dial | ask first |
| `multiremote <file> <cmd>` | Run on multiple machines | ask first |
| `remote <machine> <cmd>` | Run on remote machine | ask first |

## Scripting primitives

| Command | Effect |
|---------|--------|
| `script "<file>"` | Run NirCmd script file |
| `paramsfile "<file>"` | Read params from file |
| `loop <count> <delay> <cmd>` | Repeat a command |
| `wait <ms>` | Sleep |
| `returnval <n>` | Set exit code |
| `sysvar set/get <name> <value>` | Set/get env var |
| `sysreq <hwnd> <message>` | Send window message |
| `using <hwnd> <cmd>` | Run command in context of window |

## Misc

| Command | Effect |
|---------|--------|
| `gac install/uninstall "<assembly>"` | .NET GAC management |
| `memdump <process> "<file>"` | Memory dump |
| `verhistory` | Show NirCmd version history |
| `cmdwait <ms> <cmd>` | Already listed under Process |

---

For commands not listed in detail here, the bundled `NirCmd.chm` (in `C:\tools\nircmd\`) is the canonical reference. You can decompile it to HTML with `hh.exe -decompile <outdir> "C:\tools\nircmd\NirCmd.chm"` if you need it on the Linux side.
