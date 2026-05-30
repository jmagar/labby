# nircmd — Window Control

## The `win` command

NirCmd's `win` is a multi-subcommand dispatcher. Syntax:

```
nircmd win <action> <find-method> "<target>" [extra args]
```

## Find methods

| Method | Matches | Use when |
|--------|---------|----------|
| `title` | Exact window title | You know the literal title |
| `ititle` | Case-insensitive substring | You want fuzzy matching — **most common** |
| `class` | Window class name | You know the WNDCLASS (rare) |
| `process` | Process executable name (`chrome.exe`) | One process owns one window |
| `handle` | Numeric HWND | You already have a handle |
| `active` | Currently focused window | No need for a target string |
| `alltop` | All top-level windows | Bulk operations |
| `all` | All windows including children | Bulk; usually too broad |

## Common actions

| Action | What it does |
|--------|--------------|
| `activate` | Bring to front, focus |
| `min` | Minimize |
| `max` | Maximize |
| `normal` | Restore to normal size |
| `show` | Make visible (if hidden) |
| `hide` | Make invisible |
| `close` | Send WM_CLOSE (graceful close) |
| `flash` | Flash the title bar (taskbar notification) |
| `flashex` | Flash with count/interval params |
| `settext` | Set the window title |
| `settopmost <0 or 1>` | Pin (1) or unpin (0) always-on-top |
| `setalpha <0-255>` | Window transparency |
| `move <x> <y>` | Move to coordinates |
| `setsize <w> <h>` | Resize |
| `center` | Center on screen |
| `togglemin` / `togglemax` | Toggle between current state |

## Examples

Bring VS Code to front:
```bash
ssh "$NIRCMD_HOST" "$NIRCMD_PATH win activate ititle \"Visual Studio Code\""
```

Minimize all Chrome windows:
```bash
ssh "$NIRCMD_HOST" "$NIRCMD_PATH win min process \"chrome.exe\""
```

Pin a window always-on-top:
```bash
ssh "$NIRCMD_HOST" "$NIRCMD_PATH win settopmost ititle \"Calculator\" 1"
```

Close the currently focused window:
```bash
ssh "$NIRCMD_HOST" "$NIRCMD_PATH win close active"
```

## Activate + capture pattern (used by `scripts/win-shot.sh`)

```bash
ssh "$NIRCMD_HOST" "$NIRCMD_PATH win activate ititle \"$title\" && $NIRCMD_PATH wait 200 && $NIRCMD_PATH savescreenshotwin '$remote_win'"
```

The `wait 200` (milliseconds) lets Windows redraw before `savescreenshotwin` reads the framebuffer. Without it you sometimes get the pre-activation paint.

## Listing windows

NirCmd doesn't have a clean "list windows" command. For that, drop down to PowerShell:

```bash
ssh "$NIRCMD_HOST" "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe -NoProfile -Command \"
  Get-Process | Where-Object { \\\$_.MainWindowTitle -ne '' } | Select-Object Id, ProcessName, MainWindowTitle | Format-Table -AutoSize
\""
```

## Keystroke simulation

`sendkey` and `sendkeypress` simulate keyboard input *to whatever window is currently focused* — so always `win activate` first.

```bash
nircmd win activate ititle "Notepad"
nircmd sendkeypress alt+f4    # close Notepad
```

**Warning:** keystroke simulation can lose data (Ctrl+W, Ctrl+Q, Alt+F4 in editors with unsaved files). See `safety-boundaries.md` Tier 2 — combos that affect data should be confirmed before use.

## Mouse

| Command | Effect |
|---------|--------|
| `setcursor <x> <y>` | Move mouse to absolute coordinates |
| `setcursorwin <relx> <rely>` | Move mouse to coords relative to active window |
| `movecursor <dx> <dy>` | Relative move from current position |
| `sendmouse left/right/middle click/down/up` | Click |

For mouse-driven UI automation prefer `pyautogui` or `nut.js` over this — NirCmd's mouse support is barebones.
