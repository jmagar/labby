# nircmd — Safety Boundaries

NirCmd exposes ~115 commands. Some are completely safe ("set the volume"), some can damage the user's environment ("reboot now"), and most live in the middle ("kill this process — but what if it was their unsaved editor?").

The skill enforces three tiers. **Always apply these regardless of how the user phrased the request.**

## Tier 1 — Auto-allowed

Run freely. No confirmation needed. Reversible, low blast radius.

| Command(s) | Why safe |
|------------|----------|
| `setsysvolume`, `changesysvolume`, `mutesysvolume`, `setappvolume`, `muteappvolume`, `setdefaultsounddevice`, `showsounddevices` | Audio is reversible and contextual |
| `setbrightness`, `changebrightness` | Display brightness — annoying if wrong but harmless |
| `monitor on/off` | Reversible by mouse wiggle |
| `lockws` | Locks screen; user just unlocks |
| `screensaver`, `screensavertimeout` | Reversible |
| `speak text "..."` | TTS — audible but harmless |
| `stdbeep`, `beep` | Audible, harmless |
| `mediaplay` (play/pause/stop) | Media control, user-recoverable |
| `win activate/min/max/normal/show/hide/move/size/settext/settopmost/setalpha` | Window state — visible, easily undone |
| `setcursor`, `setcursorwin`, `movecursor` | Mouse position — user moves it back |
| `sendkey`, `sendkeypress` | Single keystrokes; AVOID combos that destroy data (e.g. don't send Ctrl+W to an editor) |
| `infobox`, `qboxtop` (display-only), `trayballoon` | Show notifications/dialogs to the user |
| `cdrom open/close` | Trivial |
| `inisetval`, `inidelval`, `inidelsec` on app-config `.ini` files | Scoped to the file you target |
| `convertimage`, `convertimages` | Image format conversion — output to a new file |
| `clonefiletime`, `setfiletime`, `setfilefoldertime` on files you created | Timestamp manipulation; fine if you own the file |
| `urlshortcut`, `shortcut`, `cmdshortcut`, `cmdshortcutkey` | Creates `.lnk` files; benign |
| `shellrefresh`, `sysrefresh`, `restartexplorer` | Refreshes Explorer; benign annoyance at worst |
| `consolewrite`, `setconsolecolor`, `setconsolemode`, `debugwrite` | Console I/O |
| `wait`, `loop`, `returnval`, `sysvar`, `sysreq`, `script`, `paramsfile`, `using` | Scripting primitives, no system effect |
| `verhistory`, `gac`, `memdump` | Read-only info |

## Tier 2 — Ask the user first

Always confirm with the user before invoking. Tell them exactly what will be affected.

| Command(s) | Why caution |
|------------|----------|
| `killprocess <name>`, `closeprocess <name>` | Can lose unsaved work; name match may hit multiple processes |
| `suspendprocess` | Halts a running program |
| `setprocessaffinity`, `setprocesspriority` | Performance-affecting; may starve other apps |
| `runas`, `runassystem`, `runinteractive`, `runinteractivecmd`, `elevate`, `elevatecmd`, `exec`, `exec2`, `execmd`, `shexec` | Runs arbitrary code; admin escalation possible |
| `regsetval`, `regdelkey`, `regdelval`, `regsvr` | Registry mutation — can break apps or Windows |
| `regedit` | Opens UAC-prompted GUI; intent unclear |
| `service start/stop/restart/pause/continue` | Affects system services; can disrupt running apps |
| `emptybin` | Empties Recycle Bin — irreversible |
| `filldelete <file>` | Secure-delete a file — irreversible, multi-pass overwrite |
| `moverecyclebin` | Moves files to Recycle Bin |
| `setdisplay` | Resolution change — disorients the user |
| `setprimarydisplay` | Reshuffles multi-monitor layout |
| `setdialuplogon`, `rasdial`, `rashangup`, `inetdial` | Network connection manipulation |
| `qbox`, `qboxcom`, `qboxcomtop` (when they invoke a command-on-Yes) | The `command-if-yes` arg runs arbitrary code |
| `sendkey` combos that affect data (Ctrl+S, Ctrl+W, Ctrl+Q, etc.) | Could save garbage or close unsaved work |
| `multiremote`, `remote` | Acts on other machines |
| `dlg`, `dlgany` | Generic dialog-driven actions |

## Tier 3 — Refuse without an extremely explicit instruction

Even if asked, confirm the *exact target and timing* and require unambiguous user intent. Default to refusal.

| Command(s) | Why refuse |
|------------|----------|
| `exitwin logoff/poweroff/reboot/shutdown` | Loses unsaved work; user may not realize Claude can do this |
| `standby`, `hibernate` | Same blast radius — user loses interactive state |
| `initshutdown` | Scheduled shutdown of local or remote machine |
| `abortshutdown` | Only safe if user explicitly asked to cancel a pending shutdown |

For Tier 3, the user instruction must contain BOTH (a) the explicit action ("reboot my computer", not "restart things") AND (b) the target ("my Win11 box", "steamy-wsl"). If either is fuzzy, ask before acting.

## When in doubt

Read this file. If a command isn't listed: classify it conservatively — if you can't undo it in one obvious step, treat it as Tier 2 (ask first).
