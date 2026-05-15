# System Service Audit - 2026-05-15

Audit run from `/home/jmagar/workspace/lab` on host `dookie`.

## Executive Summary

- Removed the OpenClaw user service:
  - Unit: `openclaw-gateway.service`
  - Unit file removed: `/home/jmagar/.config/systemd/user/openclaw-gateway.service`
  - Previous command: `node .../openclaw/dist/index.js gateway --port 18789`
  - Previous listener: `127.0.0.1:18789` and `[::1]:18789`
  - Verification after removal: systemd could not find the unit, no OpenClaw process remained, and port `18789` was no longer listening.
- Removed the stale disabled system Lab unit:
  - Unit file removed: `/etc/systemd/system/lab.service`
  - Previous command: `/usr/local/bin/lab serve`
  - Verification after removal: systemd could not find `lab.service`.
- Current service cruft is mostly in three buckets:
  - Docker containers with `restart=unless-stopped` or `restart=always`, especially `labby`, MCP playground containers, Open Design, Mem0, Axon, Arcane, and Docker socket proxy.
  - User systemd units under `~/.config/systemd/user`, including old agent/orchestration experiments and failed syslog AI units.
  - Agent/MCP process fan-out from active Zed/Codex/Claude/Lab sessions. At audit time, the current agent/MCP process pattern count was `110`; `agent-proc-reaper --once --log ''` reported `wrappers=71 stale=10`.

## OpenClaw Removal

Before removal:

```text
openclaw-gateway.service
Loaded: /home/jmagar/.config/systemd/user/openclaw-gateway.service; enabled
Active: active (running)
Main PID: 3347
Memory: 330.6M, peak 1.1G
ExecStart: /home/jmagar/.local/share/fnm/node-versions/v24.15.0/installation/bin/node /home/jmagar/.local/share/fnm/node-versions/v22.18.0/installation/lib/node_modules/openclaw/dist/index.js gateway --port 18789
Restart=always
```

Actions taken:

```bash
systemctl --user disable --now openclaw-gateway.service
rm -f /home/jmagar/.config/systemd/user/openclaw-gateway.service
systemctl --user daemon-reload
```

Post-removal verification:

```text
systemctl --user status openclaw-gateway.service -> Unit could not be found
unit_file_absent=0
ss -ltnp | rg '18789|openclaw' -> no output
ps ... | rg 'openclaw|gateway --port 18789' -> no matching live process
```

## Currently Running User Services

These are active user-level services:

| Unit | Status | Notes |
| --- | --- | --- |
| `agent-proc-watch.service` | running | Custom watcher. Exec: `/home/jmagar/.local/bin/agent-proc-watch --interval 1 ...`. Memory about 24 MB. |
| `at-spi-dbus-bus.service` | running | Desktop accessibility bus. |
| `dbus.service` | running | User D-Bus. |
| `filter-chain.service` | running | PipeWire filter chain. |
| `gnome-keyring-daemon.service` | running | GNOME keyring. |
| `mpris-proxy.service` | running | Bluetooth MPRIS proxy. |
| `pipewire-pulse.service` | running | PulseAudio compatibility. |
| `pipewire.service` | running | PipeWire. |
| `sccache.service` | running | Custom user unit. Exec: `/usr/bin/env SCCACHE_START_SERVER=1 /usr/bin/sccache`. Memory about 1.5 GB. |
| `snap.snapd-desktop-integration...service` | running | Snap desktop integration. |
| `wireplumber.service` | running | PipeWire session manager. |
| `xdg-document-portal.service` | running | Flatpak document portal. |
| `xdg-permission-store.service` | running | Portal permission store. |

Failed user units:

| Unit | Status | Evidence |
| --- | --- | --- |
| `syslog-ai-index.service` | failed | Custom unit: `/home/jmagar/.local/bin/syslog-ai-index`. |
| `syslog-ai-watch.service` | failed | Custom unit: `/home/jmagar/.local/bin/syslog ai watch --no-initial-scan --json`; enabled and failed after repeated restart attempts. |
| `xdg-desktop-portal.service` | failed | Desktop portal failure; may be harmless if GNOME portal is active, but should be checked before removing. |
| `xdg-desktop-portal-gtk.service` | failed | GTK portal failure. |
| `run-p1585795-i22530282.scope` | failed | Old transient cargo test scope. Safe to reset failed state. |

## Custom User Systemd Units

Files present in `/home/jmagar/.config/systemd/user`:

| Unit | State | Risk / cleanup note |
| --- | --- | --- |
| `agent-proc-watch.service` | enabled/running | Keep if you want process-storm telemetry. |
| `agent-proc-reaper.service` | static | One-shot cleanup service. Recent logs show it killed 9-10 stale wrappers repeatedly until the Lab container state changed, then `stale=0`. Keep but matcher safety remains important. |
| `agent-proc-reaper.timer` | enabled/active | Runs every ~10 minutes. Keep only if you want automatic MCP cleanup. |
| `artifact-prune.service` | static | Runs `/home/jmagar/bin/artifact-prune.sh`. |
| `artifact-prune.timer` | enabled/active | Runs every ~15 minutes. Check whether this is still desired; it can delete build artifacts by design. |
| `agor.service` | disabled/inactive | Old agent orchestration daemon. Cleanup candidate if no longer used. |
| `moltbot-gateway.service` | disabled/inactive | Old gateway on the same `18789` port OpenClaw used. Cleanup candidate. |
| `nugs-watch.service` | disabled/inactive | Watch command is inactive, but the timer is enabled. |
| `nugs-watch.timer` | enabled/active | Runs `nugs watch check`. Cleanup candidate if you do not still want automatic downloads/checks. |
| `oom-protect.service` | static/inactive | Uses `sudo /home/jmagar/.local/bin/oom-protect`. Timer disabled. Keep only if intentionally used. |
| `oom-protect.timer` | disabled/inactive | Cleanup candidate if abandoned. |
| `sccache.service` | enabled/running | Heavy but intentional for Rust builds. Memory about 1.5 GB. |
| `syslog-ai-index.service` | static/failed | Cleanup or fix candidate. |
| `syslog-ai-index.timer` | disabled/inactive | Cleanup candidate if syslog AI indexing is abandoned. |
| `syslog-ai-watch.service` | enabled/failed | Highest-priority user-unit cleanup candidate: enabled but failed. |
| `zclean.service` | static/inactive | AI coding zombie cleanup helper. |
| `zclean.timer` | enabled/active | Runs hourly. Keep only if you trust this cleaner. |

## Currently Running System Services

Running system services at audit time:

```text
accounts-daemon.service
avahi-daemon.service
chrony.service
colord.service
containerd.service
cron.service
cups-browsed.service
cups.service
dbus.service
docker.service
fwupd.service
gdm.service
gnome-remote-desktop.service
ModemManager.service
networkd-dispatcher.service
NetworkManager.service
nvidia-persistenced.service
polkit.service
power-profiles-daemon.service
qemu-guest-agent.service
rsyslog.service
rtkit-daemon.service
rustdesk.service
snap.tailscale.tailscaled.service
snapd.service
ssh.service
switcheroo-control.service
systemd-journald.service
systemd-logind.service
systemd-resolved.service
systemd-udevd.service
udisks2.service
unattended-upgrades.service
upower.service
user@1000.service
user@60578.service
wpa_supplicant.service
zfs-zed.service
```

Notable system-level services to review:

| Unit | State | Note |
| --- | --- | --- |
| `docker.service` | enabled/running | Owns most of the homelab/playground cruft because many containers use restart policies. |
| `snap.tailscale.tailscaled.service` | enabled/running | Snap Tailscale is active and exposes Tailscale listeners. |
| `rustdesk.service` | enabled/running | Remote desktop service. Keep only if intentionally used. |
| `gnome-remote-desktop.service` | enabled/running | Another remote desktop surface. |
| `ssh.service` | enabled/running | Expected if remote access is needed. |
| `cups.service` + `cups-browsed.service` | enabled/running | Printer discovery/scheduler. Cleanup candidate if this machine does not print. |
| `avahi-daemon.service` | enabled/running | mDNS discovery. Cleanup candidate if not needed. |
| `ModemManager.service` | enabled/running | Usually unnecessary on fixed desktops/servers without cellular modems. |
| `openvpn.service` | enabled/exited | Enabled but not actively running a tunnel. Review if unused. |
| `sssd.service` | enabled/inactive | Enabled but inactive; likely package cruft unless identity integration is used. |

Custom or manually added system unit files:

| Unit | State | Note |
| --- | --- | --- |
| `/etc/systemd/system/coredns.service` | disabled/inactive | Description says `CoreDNS (moltbot/clawdbot internal)`. Cleanup candidate. |
| `/etc/systemd/system/lab.service` | removed | Old system `lab serve` unit removed after the audit; current Lab runs through Docker `labby`. |
| `/etc/systemd/system/ollama.service` | disabled/inactive | Ollama service file remains installed but disabled. Cleanup candidate if abandoned. |
| `/etc/systemd/system/snap.mesa-2404.component-monitor.service` | disabled/inactive | Snap-generated. |
| `/etc/systemd/system/snap.tailscale.tailscaled.service` | enabled/running | Snap-generated Tailscale. |

## Docker Containers

Running containers:

| Container | Restart policy | Ports | Image | Cleanup note |
| --- | --- | --- | --- | --- |
| `labby` | `unless-stopped` | `0.0.0.0:8765->8765` | `labby:dev` | High-impact. This respawned after a previous stop and owns Lab-managed MCP child processes. |
| `open-design` | `always` | `0.0.0.0:7456->7456` | `vanjayak/open-design:latest` | Persistent design daemon. Keep only if in active use. |
| `openmemory-mcp` | `unless-stopped` | `127.0.0.1:18765->8765` | `mem0/openmemory-mcp:axon` | Memory/MCP experiment. |
| `mem0-dev-mem0-1` | `no` | `0.0.0.0:50020->8000` | `mem0-dev-mem0` | Dev stack exposed on all interfaces. |
| `mem0-dev-postgres-1` | `on-failure` | `0.0.0.0:8432->5432` | `ankane/pgvector:v0.5.1` | Postgres exposed on all interfaces. Review. |
| `dockersocket` | `unless-stopped` | `0.0.0.0:2375->2375` | Docker socket proxy | High-risk if reachable beyond trusted network. |
| `axon` | `unless-stopped` | `0.0.0.0:8001->8001` | `ghcr.io/jmagar/axon:latest` | Intentional if Axon is active. |
| `axon-qdrant` | `unless-stopped` | `127.0.0.1:53333/53334` | `qdrant/qdrant:v1.13.1` | Heavy: about 2.26 GiB. |
| `axon-tei` | `unless-stopped` | `127.0.0.1:52000->80` | HF TEI | Heavy: about 1.2 GiB. |
| `axon-chrome` | `unless-stopped` | `127.0.0.1:6000`, `9222-9223` | `axon-axon-chrome` | Browser automation container. |
| `syslog-mcp` | `unless-stopped` | `1514 tcp/udp`, `3100` | `syslog-mcp:local-debug` | Active and chatty; about 169 MiB, 4% CPU sample. |
| `arcane-agent` | `unless-stopped` | `0.0.0.0:3553->3553` | `ghcr.io/getarcaneapp/arcane-headless` | Experiment/admin surface. Review. |
| `aurora-design-system` | `unless-stopped` | `0.0.0.0:50000->3000` | local | Heavy: about 1.19 GiB. |
| `reverent_chatelet` | `no` | none | `ghcr.io/getarcaneapp/tools` | Stray container name; likely safe cleanup if not actively attached. |
| `unraid-mcp` | `unless-stopped` | `0.0.0.0:40010` | `ghcr.io/jmagar/unraid-mcp` | Health check failing because `curl` is missing in image. |
| `gotify-mcp` | `unless-stopped` | `0.0.0.0:40020` | `ghcr.io/jmagar/gotify-mcp` | Health check failing because `curl` is missing in image. |
| `unifi-mcp` | `unless-stopped` | `0.0.0.0:40030` | `ghcr.io/jmagar/rustifi` | Playground MCP. |
| `tailscale-mcp` | `unless-stopped` | `0.0.0.0:40040` | `ghcr.io/jmagar/rustscale` | Playground MCP. |
| `apprise-mcp` | `unless-stopped` | `0.0.0.0:40050` | `ghcr.io/jmagar/apprise-mcp` | Playground MCP. |
| `example-mcp` | `unless-stopped` | `0.0.0.0:40060` | `ghcr.io/your-org/example-mcp` | Strong cleanup candidate; placeholder image/org. |

Docker health-check issue:

```text
unraid-mcp health: /bin/sh: 1: curl: not found
gotify-mcp health: /bin/sh: 1: curl: not found
```

This is image health-check drift, not necessarily app failure.

## Listening Ports

External or all-interface listeners worth reviewing:

| Port | Listener / source | Note |
| --- | --- | --- |
| `22` | SSH | Expected if remote shell access is needed. |
| `2375` | `dockersocket` container | High-risk Docker socket proxy surface. Should not be exposed broadly. |
| `3100` | `syslog-mcp` | Exposed on all interfaces. |
| `3553` | `arcane-agent` | Exposed on all interfaces. |
| `3847` | `python3` PID `1536232` | Unknown Python listener; investigate. |
| `7456` | `open-design` container | Exposed on all interfaces. |
| `8001` | `axon` container | Exposed on all interfaces. |
| `8002` | host `axon` PID `3658396` | Separate host Axon process in addition to container Axon. |
| `8080`, `8081` | Node `serve.ts` PID `3662248` | Host dev server. |
| `8432` | `mem0-dev-postgres-1` | Postgres exposed on all interfaces. |
| `8765` | `labby` | Lab UI/API exposed on all interfaces. |
| `40010-40060` | MCP containers | Playground MCP fleet exposed on all interfaces. |
| `50000` | `aurora-design-system` | Design-system dev app exposed on all interfaces. |
| `50020` | `mem0-dev-mem0-1` | Mem0 dev app exposed on all interfaces. |
| `1514 tcp/udp` | `syslog-mcp` | Syslog ingest exposed on all interfaces. |
| `21118`, `21119` | RustDesk | Remote desktop. |
| `3389` | GNOME remote desktop / RDP stack | Remote desktop. |

Loopback-only listeners that are probably lower risk:

```text
127.0.0.1:52000 axon-tei
127.0.0.1:18765 openmemory-mcp
127.0.0.1:53333/53334 axon-qdrant
127.0.0.1:9222/9223 axon-chrome
127.0.0.1:44619 dolt sql-server
127.0.0.1:631 CUPS
```

## Agent / MCP Process State

At audit time:

```text
matching agent/MCP process patterns: 110
agent-proc-reaper dry-run: wrappers=71 stale=10 reaped=0
```

Process name buckets:

```text
66 MainThread
13 codex
12 codex-acp
9 claude
2 npm
1 labby
1 docker-init
1 chrome-devtools
```

This confirms that the process storm is not a normal systemd-service problem alone. It is mostly active agent sessions plus Lab/Docker-managed MCPs. The `labby` container is especially important because it respawns Lab-owned MCP child processes and uses `restart=unless-stopped`.

## Snap Services and Packages

Active snap services:

```text
snapd-desktop-integration.snapd-desktop-integration active user service
tailscale.tailscaled active system service
```

Installed snaps include dev/user tools and several bases:

```text
btop, chezmoi, code-insiders, discord, firefox, gh, go, golangci-lint,
google-cloud-cli, helix, rustup, tailscale, snap-store, snapd,
desktop-security-center, prompting-client, GNOME/core bases.
```

Snap warning:

```text
snap "desktop-security-center" has bad plugs or slots: ubuntu-pro-control
unknown interface "ubuntu-pro-control"
```

Cleanup candidates if unused: `desktop-security-center`, `prompting-client`, `snap-store`, duplicate dev tools installed elsewhere (`go`, `rustup`, `gh`, `google-cloud-cli`, `code-insiders`).

## User Sessions and Linger

`loginctl` shows user linger enabled:

```text
1000 jmagar yes active
/var/lib/systemd/linger/jmagar exists
```

There are many active user sessions for UID `1000`. Linger means user services and timers can continue after logout, which is useful for intentional daemons but also keeps old experiments alive.

## Resource Hotspots

Top observed memory users:

| Process/container | Memory |
| --- | ---: |
| `axon-qdrant` / `qdrant` | about 2.26 GiB |
| `sccache.service` | about 1.5 GiB |
| `axon-tei` | about 1.2 GiB |
| `aurora-design-system` | about 1.19 GiB |
| `labby` container | about 1.21 GiB |
| `next-server` | about 1.08 GiB |
| `claude` / `codex-acp` sessions | several hundred MB each |
| duplicated MCP Node processes | dozens around 65-180 MB each |

## Recommended Cleanup Plan

High confidence cleanup candidates:

1. Remove disabled legacy user units:
   - `~/.config/systemd/user/agor.service`
   - `~/.config/systemd/user/moltbot-gateway.service`
   - `~/.config/systemd/user/oom-protect.timer` if abandoned
2. Fix or disable failed syslog AI user units:
   - `syslog-ai-watch.service` is enabled and failed.
   - `syslog-ai-index.service` is failed.
3. Remove disabled old system units if no longer used:
   - `/etc/systemd/system/coredns.service`
   - `/etc/systemd/system/ollama.service`
4. Stop/remove obvious Docker playground cruft:
   - `example-mcp`
   - `reverent_chatelet`
   - MCP containers on `40010-40060` if not actively used.
5. Decide whether `labby` should be persistent. If not, change/remove its restart policy and stop it:
   - Current: `restart=unless-stopped`
   - Impact: stops Lab UI and Lab-managed MCP respawns.

Needs explicit decision before removal:

1. Remote access stack:
   - `rustdesk.service`
   - `gnome-remote-desktop.service`
   - `ssh.service`
   - `snap.tailscale.tailscaled.service`
2. Docker socket proxy:
   - `dockersocket` exposes `2375` on all interfaces. This is high risk, but may be intentional for Arcane or homelab management.
3. Axon stack:
   - `axon`, `axon-qdrant`, `axon-tei`, `axon-chrome`, plus host Axon on port `8002`.
4. Open Design and design/dev containers:
   - `open-design`
   - `aurora-design-system`
5. Printing/discovery:
   - `cups`, `cups-browsed`, `avahi-daemon`

## Commands Used

```bash
systemctl --user status openclaw-gateway.service --no-pager
systemctl --user cat openclaw-gateway.service
systemctl --user disable --now openclaw-gateway.service
rm -f /home/jmagar/.config/systemd/user/openclaw-gateway.service
systemctl --user daemon-reload
systemctl --user list-units --type=service --state=running,failed --no-pager
systemctl list-units --type=service --state=running,failed --no-pager
systemctl --user list-unit-files --type=service --no-pager
systemctl list-unit-files --type=service --no-pager
systemctl --user list-timers --all --no-pager
systemctl list-timers --all --no-pager
systemctl --user list-sockets --all --no-pager
systemctl list-sockets --all --no-pager
find /home/jmagar/.config/systemd/user /etc/systemd/system -maxdepth 3 -type f
docker ps -a
docker inspect
docker stats --no-stream
ss -ltnup
snap list
snap services
snap warnings
loginctl list-users
loginctl list-sessions
agent-proc-reaper --once --log ''
ps -eo pid,ppid,stat,comm,rss,%mem,%cpu,args --sort=-rss
```
