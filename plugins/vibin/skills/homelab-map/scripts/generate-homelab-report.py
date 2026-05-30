#!/usr/bin/env python3
"""Generate the WillyNet homelab report from live host checks.

The script intentionally uses only stdlib Python plus non-interactive SSH so it
can run outside an agent session. It avoids secrets and records collection
failures inline instead of silently preserving stale values.
"""

from __future__ import annotations

import argparse
import datetime as dt
import html
import json
import shutil
import socket
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parents[4]
DEFAULT_TEMPLATE = REPO_ROOT / "src/skills/homelab-map/references/homelab.md"
DEFAULT_DIR = Path.home() / ".homelab"
DEFAULT_OUTPUT = DEFAULT_DIR / "homelab.md"
DEFAULT_JSON_OUTPUT = DEFAULT_DIR / "homelab.json"
DEFAULT_HTML_OUTPUT = DEFAULT_DIR / "index.html"
DEFAULT_SERVE_BIND = "0.0.0.0"
DEFAULT_SERVE_PORT = 40500
DEFAULT_TAILSCALE_HTTPS_PORT = 8447


@dataclass(frozen=True)
class HostSpec:
    key: str
    label: str
    ssh_host: str
    role: str
    os_note: str
    ssh_port: int | None = None
    docker: bool = True
    zfs: bool = False
    unraid: bool = False


@dataclass
class CommandResult:
    ok: bool
    stdout: str = ""
    stderr: str = ""
    returncode: int | None = None


@dataclass
class HostSnapshot:
    spec: HostSpec
    hostname: str = ""
    kernel: str = ""
    uptime: str = ""
    memory: str = ""
    tailscale_ip: str = ""
    ipv4: list[str] = field(default_factory=list)
    containers: list[dict[str, str]] = field(default_factory=list)
    zpool: list[list[str]] = field(default_factory=list)
    df: list[str] = field(default_factory=list)
    extras: dict[str, str] = field(default_factory=dict)
    errors: list[str] = field(default_factory=list)


HOSTS = [
    HostSpec("tootie", "tootie", "tootie", "Primary NAS / app server", "Unraid", ssh_port=29229, zfs=True, unraid=True),
    HostSpec("dookie", "dookie", "dookie", "Dev / AI / MCP hub", "Linux KVM guest on tootie", zfs=False),
    HostSpec("squirts", "squirts", "squirts", "Edge services", "Ubuntu", zfs=True),
    HostSpec("shart", "shart", "shart", "ZFS backup target", "Unraid", zfs=True, unraid=True),
    HostSpec("steamy", "steamy / steamy-wsl", "steamy-wsl", "GPU workloads", "Windows 11 + WSL2"),
    HostSpec("vivobook", "vivobook / vivobook-wsl", "vivobook-wsl", "Mobile dev laptop", "Windows 11 + WSL2"),
]

SERVICE_HINTS = {
    "tootie": [
        "plex", "sonarr", "radarr", "bazarr", "prowlarr", "qbittorrent", "sabnzbd",
        "tautulli", "immich", "audiobookshelf", "kavita", "navidrome", "minio",
        "loggifly", "notifiarr", "apprise-api", "olivetin", "zipline",
    ],
    "dookie": [
        "axon", "axon-qdrant", "axon-tei", "axon-chrome", "syslog-mcp",
        "arcane-mcp", "unraid-mcp", "gotify-mcp", "unifi-mcp", "tailscale-mcp",
        "apprise-mcp", "labby", "agent-os-win11",
    ],
    "squirts": [
        "swag", "authelia", "adguard", "gotify", "vaultwarden", "paperless",
        "linkding", "karakeep", "bytestash", "memos", "radicale", "searxng",
        "dockge", "dozzle", "rustdesk", "multi-scrobbler", "maloja",
    ],
    "shart": ["arcane-agent", "portainer_agent", "dockersocket"],
    "steamy": ["crawl4r-qdrant", "arcane-agent"],
    "vivobook": ["arcane-agent"],
}

MCP_HINTS = [
    ("syslog-mcp", "dookie", "1514 TCP/UDP, 3100 HTTP"),
    ("arcane-mcp", "dookie", "44332"),
    ("unraid-mcp", "dookie", "40010"),
    ("gotify-mcp", "dookie", "40020"),
    ("unifi-mcp", "dookie", "40030"),
    ("tailscale-mcp", "dookie", "40040"),
    ("apprise-mcp", "dookie", "40050"),
    ("example-mcp", "dookie", "40060"),
    ("swag-mcp", "squirts", "8012 localhost binding"),
]


def run_local(argv: list[str], timeout: int = 20) -> CommandResult:
    try:
        proc = subprocess.run(argv, text=True, capture_output=True, timeout=timeout, check=False)
        return CommandResult(proc.returncode == 0, proc.stdout.strip(), proc.stderr.strip(), proc.returncode)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return CommandResult(False, "", str(exc), None)


def ssh_command(spec: HostSpec, remote_command: str, timeout: int = 20) -> CommandResult:
    argv = [
        "ssh",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=5",
    ]
    if spec.ssh_port:
        argv.extend(["-p", str(spec.ssh_port)])
    argv.extend([spec.ssh_host, remote_command])
    return run_local(argv, timeout=timeout)


def first_line(text: str) -> str:
    return text.splitlines()[0].strip() if text.strip() else ""


def parse_docker_json(lines: Iterable[str]) -> list[dict[str, str]]:
    containers: list[dict[str, str]] = []
    for line in lines:
        if not line.strip():
            continue
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        containers.append({
            "name": item.get("Names", ""),
            "image": item.get("Image", ""),
            "ports": item.get("Ports", ""),
            "status": item.get("Status", ""),
        })
    return sorted(containers, key=lambda item: item["name"])


def collect_host(spec: HostSpec) -> HostSnapshot:
    snap = HostSnapshot(spec)

    basics = ssh_command(
        spec,
        "printf 'HOSTNAME='; hostname; "
        "printf 'KERNEL='; uname -sr; "
        "printf 'UPTIME='; uptime -p; "
        "printf 'MEMORY='; free -h | awk '/Mem:/ {print $3 \" / \" $2 \" used\"}'; "
        "printf 'TS='; tailscale ip -4 2>/dev/null || true; "
        "printf 'IPV4\\n'; ip -4 -brief addr 2>/dev/null || true",
    )
    if basics.ok:
        ipv4_mode = False
        for line in basics.stdout.splitlines():
            if line == "IPV4":
                ipv4_mode = True
                continue
            if ipv4_mode:
                if line.strip():
                    snap.ipv4.append(line.strip())
                continue
            if line.startswith("HOSTNAME="):
                snap.hostname = line.removeprefix("HOSTNAME=").strip()
            elif line.startswith("KERNEL="):
                snap.kernel = line.removeprefix("KERNEL=").strip()
            elif line.startswith("UPTIME="):
                snap.uptime = line.removeprefix("UPTIME=").strip()
            elif line.startswith("MEMORY="):
                snap.memory = line.removeprefix("MEMORY=").strip()
            elif line.startswith("TS="):
                snap.tailscale_ip = line.removeprefix("TS=").strip()
    else:
        snap.errors.append(f"basic SSH collection failed: {basics.stderr or basics.returncode}")

    if spec.docker:
        docker = ssh_command(spec, "docker ps --format '{{json .}}'", timeout=30)
        if docker.ok:
            snap.containers = parse_docker_json(docker.stdout.splitlines())
        else:
            snap.errors.append(f"docker ps failed: {docker.stderr or docker.returncode}")

    if spec.zfs:
        zpool = ssh_command(spec, "zpool list -H -o name,size,alloc,free,frag,health 2>/dev/null || true")
        if zpool.ok and zpool.stdout:
            snap.zpool = [line.split("\t") for line in zpool.stdout.splitlines() if line.strip()]

    if spec.unraid:
        df = ssh_command(spec, "df -h /mnt/user /mnt/cache 2>/dev/null || df -h /mnt/user 2>/dev/null || true")
        if df.ok and df.stdout:
            snap.df = df.stdout.splitlines()
        if spec.key == "tootie":
            parity = ssh_command(
                spec,
                "mdcmd status 2>/dev/null | egrep 'mdNumDisabled|diskName\\.0|rdevName\\.0|diskSize\\.0' || true",
            )
            snap.extras["parity"] = parity.stdout if parity.ok else ""
            lsblk = ssh_command(spec, "lsblk -d -o NAME,SIZE,MODEL,TYPE 2>/dev/null | sed -n '1,40p' || true")
            snap.extras["lsblk"] = lsblk.stdout if lsblk.ok else ""

    if spec.key == "squirts":
        swag = ssh_command(
            spec,
            "d=/mnt/appdata/swag/nginx/proxy-confs; "
            "if [ -d \"$d\" ]; then find \"$d\" -maxdepth 1 -type f -name '*.conf' -printf '%f\\n' | sort; fi",
            timeout=20,
        )
        if swag.ok:
            snap.extras["swag_configs"] = swag.stdout
        else:
            snap.errors.append(f"SWAG config listing failed: {swag.stderr or swag.returncode}")

    return snap


def lan_ip(snapshot: HostSnapshot) -> str:
    for line in snapshot.ipv4:
        for token in line.split():
            if token.startswith("10.1.0."):
                return token.split("/")[0]
    return ""


def container_count(snapshot: HostSnapshot) -> str:
    return str(len(snapshot.containers)) if snapshot.containers else "not collected"


def has_container(snapshot: HostSnapshot, name: str) -> bool:
    return any(c["name"] == name or name in c["name"] for c in snapshot.containers)


def find_container(snapshot: HostSnapshot, name: str) -> dict[str, str] | None:
    for container in snapshot.containers:
        if container["name"] == name or name in container["name"]:
            return container
    return None


def table(headers: list[str], rows: list[list[str]]) -> str:
    lines = ["| " + " | ".join(headers) + " |", "|" + "|".join(["---"] * len(headers)) + "|"]
    lines.extend("| " + " | ".join(cell.replace("\n", "<br>") for cell in row) + " |" for row in rows)
    return "\n".join(lines)


def bullet_list(items: Iterable[str]) -> str:
    values = [item for item in items if item]
    return "\n".join(f"- {item}" for item in values) if values else "- none observed"


def format_container_table(snapshot: HostSnapshot) -> str:
    rows = []
    for container in snapshot.containers:
        rows.append([container["name"], container["image"], container["ports"] or "-", container["status"] or "-"])
    return table(["Container", "Image", "Ports", "Status"], rows) if rows else "_No containers collected._"


def format_zpool(snapshot: HostSnapshot) -> str:
    rows = []
    for entry in snapshot.zpool:
        padded = entry + [""] * (6 - len(entry))
        rows.append(padded[:6])
    return table(["Pool", "Size", "Allocated", "Free", "Frag", "Health"], rows) if rows else "_No ZFS pool data collected._"


def service_host_rows(snapshots: dict[str, HostSnapshot]) -> list[list[str]]:
    rows = []
    for host, services in SERVICE_HINTS.items():
        snapshot = snapshots[host]
        observed = []
        missing = []
        for service in services:
            (observed if has_container(snapshot, service) else missing).append(service)
        rows.append([
            snapshot.spec.label,
            ", ".join(observed) if observed else "-",
            ", ".join(missing) if missing else "-",
        ])
    return rows


def snapshot_to_dict(snapshot: HostSnapshot) -> dict[str, object]:
    return {
        "key": snapshot.spec.key,
        "label": snapshot.spec.label,
        "ssh_host": snapshot.spec.ssh_host,
        "role": snapshot.spec.role,
        "os_note": snapshot.spec.os_note,
        "ssh_port": snapshot.spec.ssh_port,
        "hostname": snapshot.hostname,
        "kernel": snapshot.kernel,
        "uptime": snapshot.uptime,
        "memory": snapshot.memory,
        "lan_ip": lan_ip(snapshot),
        "tailscale_ip": snapshot.tailscale_ip,
        "ipv4": snapshot.ipv4,
        "containers": snapshot.containers,
        "container_count": len(snapshot.containers),
        "zpool": [
            {
                "name": (entry + [""] * 6)[0],
                "size": (entry + [""] * 6)[1],
                "allocated": (entry + [""] * 6)[2],
                "free": (entry + [""] * 6)[3],
                "fragmentation": (entry + [""] * 6)[4],
                "health": (entry + [""] * 6)[5],
            }
            for entry in snapshot.zpool
        ],
        "df": snapshot.df,
        "extras": snapshot.extras,
        "errors": snapshot.errors,
    }


def report_payload(snapshots: dict[str, HostSnapshot], generated_at: dt.datetime) -> dict[str, object]:
    swag_configs = snapshots["squirts"].extras.get("swag_configs", "")
    swag_names = [line for line in swag_configs.splitlines() if line.strip()]
    total_containers = sum(len(s.containers) for s in snapshots.values())

    service_summary = []
    for host, services in SERVICE_HINTS.items():
        snapshot = snapshots[host]
        observed = [service for service in services if has_container(snapshot, service)]
        missing = [service for service in services if not has_container(snapshot, service)]
        service_summary.append({
            "host": snapshot.spec.key,
            "label": snapshot.spec.label,
            "observed": observed,
            "missing": missing,
        })

    mcp_servers = []
    for name, host, port in MCP_HINTS:
        snapshot = snapshots[host]
        container = find_container(snapshot, name)
        mcp_servers.append({
            "name": name,
            "host": host,
            "host_label": snapshot.spec.label,
            "port": port,
            "image": container["image"] if container else None,
            "status": container["status"] if container else None,
        })

    collection_errors = [
        {"host": snapshot.spec.key, "label": snapshot.spec.label, "error": error}
        for snapshot in snapshots.values()
        for error in snapshot.errors
    ]

    return {
        "schema_version": 1,
        "generated_at": generated_at.isoformat(),
        "generated_at_display": generated_at.strftime("%Y-%m-%d %H:%M:%S %Z"),
        "generator": "src/skills/homelab-map/scripts/generate-homelab-report.py",
        "collection_method": "non-interactive SSH, Docker CLI, ZFS CLI, Unraid shell commands, and SWAG config files",
        "network": "WillyNet / 10.1.0.0/24 plus Tailscale mesh",
        "primary_public_domain": "*.tootie.tv via SWAG on squirts",
        "overview": {
            "total_nodes": len(snapshots),
            "total_containers_running": total_containers,
            "active_swag_proxy_configs": len(swag_names) if swag_names else None,
        },
        "nodes": [snapshot_to_dict(snapshots[key]) for key in ["tootie", "dookie", "squirts", "shart", "steamy", "vivobook"]],
        "service_summary": service_summary,
        "mcp_servers": mcp_servers,
        "swag_proxy_configs": swag_names,
        "collection_errors": collection_errors,
        "known_follow_up_checks": [
            "If tootie parity excerpt shows diskName.0= or diskSize.0=0, parity is not assigned.",
            "If Arcane marks a host offline but SSH works, reconcile Arcane environment registration.",
            "If an expected service appears under missing, check whether it moved, stopped, or changed container name.",
            "Confirm backup freshness from Sanoid/Syncoid logs; this report does not prove backup success.",
        ],
        "key_urls": [
            {"service": "Unraid Web UI", "url": "http://10.1.0.2:6969"},
            {"service": "Syslog MCP", "url": "http://dookie:3100"},
            {"service": "Arcane UI", "url": "https://arcane.tootie.tv"},
            {"service": "Arcane MCP", "url": "http://dookie:44332"},
            {"service": "Unraid MCP", "url": "http://dookie:40010"},
            {"service": "Plex", "url": "http://10.1.0.2:32400"},
            {"service": "Windows sandbox noVNC", "url": "http://dookie:8006"},
            {"service": "Windows sandbox RDP", "url": "dookie:33890"},
        ],
    }


def render_report(snapshots: dict[str, HostSnapshot], generated_at: dt.datetime) -> str:
    swag_configs = snapshots["squirts"].extras.get("swag_configs", "")
    swag_names = [line for line in swag_configs.splitlines() if line.strip()]
    total_containers = sum(len(s.containers) for s in snapshots.values())
    tootie_df = "\n".join(snapshots["tootie"].df)
    tootie_parity = snapshots["tootie"].extras.get("parity", "").strip()

    node_rows = []
    for key in ["tootie", "dookie", "squirts", "shart", "steamy", "vivobook"]:
        snap = snapshots[key]
        node_rows.append([
            snap.spec.label,
            snap.spec.role,
            lan_ip(snap) or "not observed",
            snap.tailscale_ip or "not observed",
            snap.spec.os_note,
            snap.kernel or "not observed",
            snap.uptime or "not observed",
            snap.memory or "not observed",
            container_count(snap),
        ])

    mcp_rows = []
    for name, host, port in MCP_HINTS:
        snap = snapshots[host]
        container = find_container(snap, name)
        mcp_rows.append([
            name,
            snap.spec.label,
            port,
            container["image"] if container else "not observed",
            container["status"] if container else "not observed",
        ])

    public_examples = ", ".join(f"`{name.removesuffix('.subdomain.conf').removesuffix('.subfolder.conf')}`" for name in swag_names[:80])
    collection_errors = []
    for snap in snapshots.values():
        for error in snap.errors:
            collection_errors.append(f"{snap.spec.label}: {error}")

    return f"""# WillyNet Homelab - Infrastructure Documentation

> Generated: {generated_at.strftime('%Y-%m-%d %H:%M:%S %Z')}
> Generator: `src/skills/homelab-map/scripts/generate-homelab-report.py`
> Collection method: non-interactive SSH, Docker CLI, ZFS CLI, Unraid shell commands, and SWAG config files.

---

## Overview

{table(["Metric", "Value"], [
    ["Total nodes", str(len(snapshots))],
    ["Total containers running", str(total_containers)],
    ["Active SWAG proxy configs", str(len(swag_names)) if swag_names else "not collected"],
    ["Network", "WillyNet / 10.1.0.0/24 plus Tailscale mesh"],
    ["Primary public domain", "*.tootie.tv via SWAG on squirts"],
])}

## Collection Notes

{bullet_list(collection_errors) if collection_errors else "- All configured host collection commands completed successfully."}

Values in this document are a fresh runtime snapshot. Re-run the generator before making operational decisions:

```bash
python3 src/skills/homelab-map/scripts/generate-homelab-report.py
```

## Nodes

{table(["Name", "Role", "LAN IP", "Tailscale IP", "OS", "Kernel", "Uptime", "Memory", "Containers"], node_rows)}

### Network Interfaces

{chr(10).join(f"#### {snap.spec.label}{chr(10)}{bullet_list(f'`{line}`' for line in snap.ipv4)}" for snap in snapshots.values())}

## Service Location Summary

Observed means the expected container name was found in the live `docker ps` output for that host. Missing may mean the service moved, is stopped, has a different container name, or is only represented by a SWAG config.

{table(["Host", "Expected services observed", "Expected services not observed"], service_host_rows(snapshots))}

## Host Container Inventory

{chr(10).join(f"### {snap.spec.label}{chr(10)}{format_container_table(snap)}" for snap in snapshots.values())}

## Storage Architecture

### tootie - Unraid Array and Cache

```text
{tootie_df or 'not collected'}
```

Parity status excerpt:

```text
{tootie_parity or 'not collected'}
```

Block devices excerpt:

```text
{snapshots["tootie"].extras.get("lsblk", "not collected")}
```

### ZFS Pools

#### tootie

{format_zpool(snapshots["tootie"])}

#### squirts

{format_zpool(snapshots["squirts"])}

#### shart

{format_zpool(snapshots["shart"])}

## Reverse Proxy & Public Services

SWAG is expected on `squirts`. Active proxy config count is generated from `/mnt/appdata/swag/nginx/proxy-confs`.

{table(["Metric", "Value"], [
    ["Active config files", str(len(swag_names)) if swag_names else "not collected"],
    ["First 80 config-derived service names", public_examples or "not collected"],
])}

## AI / RAG / Agent Stack

### Axon on dookie

{format_container_table(HostSnapshot(
    spec=snapshots["dookie"].spec,
    containers=[c for c in snapshots["dookie"].containers if c["name"].startswith("axon") or c["name"] in {"labby", "agentmemory-iii-engine-1"}],
))}

### GPU Inference on steamy

{format_container_table(snapshots["steamy"])}

## MCP Server Ecosystem

{table(["MCP server", "Host", "Port", "Observed image", "Status"], mcp_rows)}

## Backup Strategy

- `shart` is the ZFS receive target.
- Backup job freshness is not inferred by this script. Check Sanoid/Syncoid logs or Gotify notifications before relying on current backup health.
- `shart` currently reports these ZFS pools:

{format_zpool(snapshots["shart"])}

## Monitoring & Notifications

- `syslog-mcp` is expected on dookie at ports 1514 and 3100.
- Gotify is expected on squirts.
- This generator does not query application APIs or notification contents; it records container and host state only.

## Virtual Machines

- `dookie` is treated as the active Linux KVM guest hosted on tootie.
- VM inventory is not inferred by this script yet. Add `virsh list --all` collection on tootie if VM state needs to be authoritative here.

## Security Posture

- Public entrypoint: SWAG on squirts.
- Inter-node access: Tailscale and LAN SSH aliases.
- Vulnerability scan data is not generated here. Run Arcane/Trivy before acting on CVE status.
- tootie parity status is collected above; an empty parity slot remains a critical risk when observed.

## Known Issues & Follow-Up Checks

- If tootie parity excerpt shows `diskName.0=` or `diskSize.0=0`, parity is not assigned.
- If Arcane marks a host offline but SSH works, reconcile Arcane environment registration.
- If an expected service appears under "not observed", check whether it moved, stopped, or changed container name.
- Confirm backup freshness from Sanoid/Syncoid logs; this report does not prove backup success.

## Appendix: Key URLs

{table(["Service", "URL"], [
    ["Unraid Web UI", "http://10.1.0.2:6969"],
    ["Syslog MCP", "http://dookie:3100"],
    ["Arcane UI", "https://arcane.tootie.tv"],
    ["Arcane MCP", "http://dookie:44332"],
    ["Unraid MCP", "http://dookie:40010"],
    ["Plex", "http://10.1.0.2:32400"],
    ["Windows sandbox noVNC", "http://dookie:8006"],
    ["Windows sandbox RDP", "dookie:33890"],
])}
"""


def render_with_template(report_body: str, template_path: Path) -> str:
    template = template_path.read_text()
    placeholder = "{{generated_report}}"
    if placeholder not in template:
        raise ValueError(f"template must contain {placeholder}")
    return template.replace(placeholder, report_body.rstrip() + "\n")


def render_html_viewer(payload: dict[str, object]) -> str:
    payload_json = json.dumps(payload, indent=2, sort_keys=True)
    embedded_json = html.escape(payload_json)
    script_json = payload_json.replace("</", "<\\/")
    return f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>WillyNet Homelab</title>
  <style>
    :root {{
      color-scheme: light dark;
      --bg: #f7f7f4;
      --panel: #ffffff;
      --text: #202124;
      --muted: #626866;
      --line: #d9ddd8;
      --accent: #136f63;
      --warn: #a15c00;
      --bad: #9b1c31;
    }}
    @media (prefers-color-scheme: dark) {{
      :root {{
        --bg: #101412;
        --panel: #171d1a;
        --text: #eef1ed;
        --muted: #a8b0ac;
        --line: #2d3833;
        --accent: #62c7b6;
        --warn: #efb35c;
        --bad: #ff7b8d;
      }}
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      background: var(--bg);
      color: var(--text);
      font: 14px/1.5 ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    }}
    header {{
      padding: 24px clamp(16px, 3vw, 36px) 16px;
      border-bottom: 1px solid var(--line);
    }}
    main {{
      padding: 20px clamp(16px, 3vw, 36px) 36px;
      display: grid;
      gap: 20px;
    }}
    h1, h2, h3 {{ margin: 0; line-height: 1.2; }}
    h1 {{ font-size: 28px; }}
    h2 {{ font-size: 19px; margin-bottom: 10px; }}
    h3 {{ font-size: 15px; margin-bottom: 8px; }}
    a {{ color: var(--accent); }}
    .meta {{ color: var(--muted); margin-top: 6px; }}
    .grid {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
      gap: 12px;
    }}
    .metric, section {{
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 8px;
      padding: 14px;
    }}
    .metric strong {{ display: block; font-size: 22px; }}
    .metric span {{ color: var(--muted); }}
    .table-wrap {{ overflow-x: auto; }}
    table {{ width: 100%; border-collapse: collapse; min-width: 720px; }}
    th, td {{ padding: 8px 10px; border-bottom: 1px solid var(--line); text-align: left; vertical-align: top; }}
    th {{ color: var(--muted); font-weight: 600; }}
    code, pre {{ font-family: ui-monospace, "SFMono-Regular", Consolas, monospace; }}
    pre {{
      white-space: pre-wrap;
      overflow-wrap: anywhere;
      background: color-mix(in srgb, var(--panel) 80%, var(--bg));
      border: 1px solid var(--line);
      border-radius: 8px;
      padding: 12px;
      max-height: 70vh;
      overflow: auto;
    }}
    .pill {{
      display: inline-block;
      padding: 2px 7px;
      border: 1px solid var(--line);
      border-radius: 999px;
      margin: 2px;
      color: var(--muted);
      white-space: nowrap;
    }}
    .ok {{ color: var(--accent); }}
    .warn {{ color: var(--warn); }}
    .bad {{ color: var(--bad); }}
  </style>
</head>
<body>
  <header>
    <h1>WillyNet Homelab</h1>
    <div class="meta">Generated <span id="generated"></span> from <code>homelab.json</code></div>
  </header>
  <main>
    <div class="grid" id="metrics"></div>

    <section>
      <h2>Nodes</h2>
      <div class="table-wrap"><table id="nodes"></table></div>
    </section>

    <section>
      <h2>Service Summary</h2>
      <div class="table-wrap"><table id="services"></table></div>
    </section>

    <section>
      <h2>MCP Servers</h2>
      <div class="table-wrap"><table id="mcp"></table></div>
    </section>

    <section>
      <h2>Containers</h2>
      <div id="containers"></div>
    </section>

    <section>
      <h2>Raw JSON</h2>
      <p class="meta"><a href="homelab.json">Open homelab.json</a></p>
      <pre id="raw-json">{embedded_json}</pre>
    </section>
  </main>
  <script type="application/json" id="homelab-data">{script_json}</script>
  <script>
    const data = JSON.parse(document.getElementById('homelab-data').textContent);
    const text = (value) => value === null || value === undefined || value === '' ? '-' : String(value);
    const esc = (value) => text(value).replace(/[&<>"']/g, (ch) => ({{'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}}[ch]));
    const table = (headers, rows) => {{
      const head = '<thead><tr>' + headers.map((h) => `<th>${{esc(h)}}</th>`).join('') + '</tr></thead>';
      const body = '<tbody>' + rows.map((row) => '<tr>' + row.map((c) => `<td>${{c}}</td>`).join('') + '</tr>').join('') + '</tbody>';
      return head + body;
    }};
    const pills = (items, cls = '') => items.length ? items.map((item) => `<span class="pill ${{cls}}">${{esc(item)}}</span>`).join('') : '-';

    document.getElementById('generated').textContent = data.generated_at_display || data.generated_at;
    document.getElementById('metrics').innerHTML = [
      ['Nodes', data.overview.total_nodes],
      ['Running containers', data.overview.total_containers_running],
      ['SWAG proxy configs', data.overview.active_swag_proxy_configs ?? 'not collected'],
      ['Network', data.network],
    ].map(([label, value]) => `<div class="metric"><strong>${{esc(value)}}</strong><span>${{esc(label)}}</span></div>`).join('');

    document.getElementById('nodes').innerHTML = table(
      ['Name', 'Role', 'LAN', 'Tailscale', 'OS', 'Kernel', 'Uptime', 'Memory', 'Containers'],
      data.nodes.map((node) => [
        esc(node.label),
        esc(node.role),
        esc(node.lan_ip || 'not observed'),
        esc(node.tailscale_ip || 'not observed'),
        esc(node.os_note),
        esc(node.kernel || 'not observed'),
        esc(node.uptime || 'not observed'),
        esc(node.memory || 'not observed'),
        esc(node.container_count),
      ])
    );

    document.getElementById('services').innerHTML = table(
      ['Host', 'Observed', 'Missing'],
      data.service_summary.map((entry) => [
        esc(entry.label),
        pills(entry.observed, 'ok'),
        pills(entry.missing, entry.missing.length ? 'warn' : ''),
      ])
    );

    document.getElementById('mcp').innerHTML = table(
      ['Server', 'Host', 'Port', 'Image', 'Status'],
      data.mcp_servers.map((server) => [
        esc(server.name),
        esc(server.host_label),
        esc(server.port),
        esc(server.image || 'not observed'),
        esc(server.status || 'not observed'),
      ])
    );

    document.getElementById('containers').innerHTML = data.nodes.map((node) => `
      <h3>${{esc(node.label)}} <span class="meta">(${{esc(node.container_count)}})</span></h3>
      <div class="table-wrap"><table>${{table(
        ['Name', 'Image', 'Ports', 'Status'],
        node.containers.map((container) => [
          esc(container.name),
          esc(container.image),
          esc(container.ports || '-'),
          esc(container.status || '-'),
        ])
      )}}</table></div>
    `).join('');
  </script>
</body>
</html>
"""


def is_tcp_port_open(host: str, port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.settimeout(0.5)
        return sock.connect_ex((host, port)) == 0


def ensure_local_viewer_server(directory: Path, bind_host: str, port: int) -> tuple[bool, str]:
    probe_host = "127.0.0.1" if bind_host == "0.0.0.0" else bind_host
    if is_tcp_port_open(probe_host, port):
        return True, f"Local viewer server already listening at http://{probe_host}:{port}/"

    log_path = directory / "http-server.log"
    log_file = log_path.open("ab")
    try:
        subprocess.Popen(
            [
                sys.executable,
                "-m",
                "http.server",
                str(port),
                "--bind",
                bind_host,
                "--directory",
                str(directory),
            ],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            close_fds=True,
            start_new_session=True,
        )
    except OSError as exc:
        log_file.close()
        return False, f"Could not start local viewer server: {exc}"
    else:
        log_file.close()

    time.sleep(0.4)
    if is_tcp_port_open(probe_host, port):
        return True, f"Started local viewer server at http://{probe_host}:{port}/ (bind {bind_host})"
    return False, f"Local viewer server did not start; see {log_path}"


def tailscale_is_usable() -> tuple[bool, str]:
    tailscale = shutil.which("tailscale")
    if not tailscale:
        return False, "tailscale command not found; skipping Tailscale Serve."

    status = run_local([tailscale, "status"], timeout=10)
    if not status.ok:
        detail = status.stderr or status.stdout or str(status.returncode)
        return False, f"tailscale status failed; skipping Tailscale Serve: {detail}"
    return True, tailscale


def try_tailscale_serve(local_port: int, https_port: int) -> tuple[bool, str]:
    ok, detail = tailscale_is_usable()
    if not ok:
        return False, detail

    tailscale = detail
    target = f"http://127.0.0.1:{local_port}"
    serve = run_local(
        [tailscale, "serve", "--https", str(https_port), "--bg", "--yes", target],
        timeout=15,
    )
    if serve.ok:
        return True, serve.stdout or f"Tailscale Serve is proxying HTTPS port {https_port} to {target}"
    return False, f"tailscale serve failed: {serve.stderr or serve.stdout or serve.returncode}"


def maybe_serve_viewer(directory: Path, bind_host: str, local_port: int, tailscale_https_port: int) -> None:
    local_ok, local_message = ensure_local_viewer_server(directory, bind_host, local_port)
    print(local_message, file=sys.stderr)
    if not local_ok:
        return

    tailscale_ok, tailscale_message = try_tailscale_serve(local_port, tailscale_https_port)
    print(tailscale_message, file=sys.stderr)
    if tailscale_ok:
        print(f"Tailnet viewer requested on HTTPS port {tailscale_https_port}.", file=sys.stderr)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Generate the WillyNet homelab report artifacts.")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT, help=f"Output file. Default: {DEFAULT_OUTPUT}")
    parser.add_argument("--json-output", type=Path, default=DEFAULT_JSON_OUTPUT, help=f"JSON output file. Default: {DEFAULT_JSON_OUTPUT}")
    parser.add_argument("--html-output", type=Path, default=DEFAULT_HTML_OUTPUT, help=f"HTML viewer output file. Default: {DEFAULT_HTML_OUTPUT}")
    parser.add_argument("--template", type=Path, default=DEFAULT_TEMPLATE, help=f"Template file. Default: {DEFAULT_TEMPLATE}")
    parser.add_argument("--no-json", action="store_true", help="Do not write the JSON artifact.")
    parser.add_argument("--no-html", action="store_true", help="Do not write the HTML viewer artifact.")
    parser.add_argument("--no-serve", action="store_true", help="Do not start or update the local/Tailscale viewer service.")
    parser.add_argument("--serve-bind", default=DEFAULT_SERVE_BIND, help=f"Local viewer bind address. Default: {DEFAULT_SERVE_BIND}")
    parser.add_argument("--serve-port", type=int, default=DEFAULT_SERVE_PORT, help=f"Local viewer HTTP port. Default: {DEFAULT_SERVE_PORT}")
    parser.add_argument("--tailscale-https-port", type=int, default=DEFAULT_TAILSCALE_HTTPS_PORT, help=f"Tailscale Serve HTTPS port. Default: {DEFAULT_TAILSCALE_HTTPS_PORT}")
    parser.add_argument("--stdout", action="store_true", help="Print report instead of writing it.")
    parser.add_argument("--no-write", action="store_true", help="Collect and render, but do not write the output file.")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    snapshots = {spec.key: collect_host(spec) for spec in HOSTS}
    now = dt.datetime.now().astimezone()
    payload = report_payload(snapshots, now)
    report_body = render_report(snapshots, now)
    report = render_with_template(report_body, args.template)
    json_report = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    html_report = render_html_viewer(payload)

    if args.stdout:
        print(report)

    if not args.stdout and not args.no_write:
        output = args.output.resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(report)
        written = [output]
        if not args.no_json:
            json_output = args.json_output.resolve()
            json_output.parent.mkdir(parents=True, exist_ok=True)
            json_output.write_text(json_report)
            written.append(json_output)
        if not args.no_html:
            html_output = args.html_output.resolve()
            html_output.parent.mkdir(parents=True, exist_ok=True)
            html_output.write_text(html_report)
            written.append(html_output)
        for path in written:
            print(path)
        if not args.no_serve and not args.no_html:
            maybe_serve_viewer(args.html_output.resolve().parent, args.serve_bind, args.serve_port, args.tailscale_https_port)
    elif args.no_write:
        print("Rendered report without writing.", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
