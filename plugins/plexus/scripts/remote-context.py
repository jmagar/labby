#!/usr/bin/env python3
"""Build an agent-ready context block for a Plexus remote host."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


PLUGIN_ROOT = Path(__file__).resolve().parents[1]
TEMPLATE_REMOTES_DIR = PLUGIN_ROOT / "templates" / "remotes"


@dataclass
class CommandResult:
    command: list[str]
    ok: bool
    stdout: str
    stderr: str


def run(command: list[str], timeout: int) -> CommandResult:
    try:
        proc = subprocess.run(
            command,
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return CommandResult(command, False, "", str(exc))

    return CommandResult(
        command=command,
        ok=proc.returncode == 0,
        stdout=proc.stdout.strip(),
        stderr=proc.stderr.strip(),
    )


def ssh(host: str, remote_command: str, timeout: int) -> CommandResult:
    return run(
        [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            f"ConnectTimeout={timeout}",
            host,
            remote_command,
        ],
        timeout=timeout + 2,
    )


def plugin_data_dir(override: str | None = None) -> Path:
    if override:
        return Path(override).expanduser()
    for name in ("PLEXUS_DATA_DIR", "CLAUDE_PLUGIN_DATA", "CODEX_PLUGIN_DATA"):
        value = os.environ.get(name)
        if value:
            return Path(value).expanduser()
    return Path.home() / ".plexus"


def bootstrap_profiles(data_dir: Path) -> list[dict[str, str]]:
    copied: list[dict[str, str]] = []
    if not TEMPLATE_REMOTES_DIR.exists():
        return copied

    for template in sorted(TEMPLATE_REMOTES_DIR.glob("*/REMOTE.md")):
        host = template.parent.name
        target = data_dir / "remotes" / host / "REMOTE.md"
        if target.exists():
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(template, target)
        copied.append({"host": host, "from": str(template), "to": str(target)})
    return copied


def profile_path(host: str, data_dir: Path) -> Path:
    return data_dir / "remotes" / host / "REMOTE.md"


def probe_ssh(host: str, timeout: int) -> dict[str, Any]:
    probes = {
        "identity": "hostname; uname -a; printf 'user='; whoami",
        "uptime": "uptime",
        "resources": "free -h 2>/dev/null || true; df -h -x tmpfs -x devtmpfs 2>/dev/null | head -40 || true",
        "network": "hostname -I 2>/dev/null || true; ip route get 1.1.1.1 2>/dev/null || true",
        "systemd_failed": "systemctl --failed --no-pager 2>/dev/null || true",
        "docker": "if command -v docker >/dev/null 2>&1; then docker ps --format 'table {{.Names}}\\t{{.Image}}\\t{{.Status}}\\t{{.Ports}}'; else echo 'docker not found'; fi",
        "listeners": "if command -v ss >/dev/null 2>&1; then ss -tulpen 2>/dev/null | head -80; else echo 'ss not found'; fi",
    }
    return {name: vars(ssh(host, command, timeout)) for name, command in probes.items()}


def probe_tailscale(host: str, timeout: int) -> dict[str, Any] | None:
    if not shutil.which("tailscale"):
        return None
    result = run(["tailscale", "status", "--json"], timeout=timeout)
    if not result.ok or not result.stdout:
        return vars(result)
    try:
        status = json.loads(result.stdout)
    except json.JSONDecodeError:
        return vars(result)

    peers = status.get("Peer", {})
    matches = []
    for peer in peers.values():
        dns_name = peer.get("DNSName", "").rstrip(".")
        host_name = peer.get("HostName", "")
        if host in {dns_name, host_name} or dns_name.startswith(f"{host}."):
            matches.append(peer)
    return {"matches": matches, "count": len(matches)}


def probe_syslog(host: str, timeout: int) -> dict[str, Any] | None:
    if not shutil.which("syslog"):
        return None
    commands = {
        "tail": ["syslog", "tail", "-n", "20", "--hostname", host, "--json"],
        "errors": ["syslog", "errors", "--from", "24h", "--json"],
        "sessions": ["syslog", "sessions", "--hostname", host, "--limit", "10", "--json"],
    }
    return {name: vars(run(command, timeout=timeout)) for name, command in commands.items()}


def build_context(host: str, probe: bool, timeout: int, data_dir: Path) -> dict[str, Any]:
    bootstrap_profiles(data_dir)
    remote_path = profile_path(host, data_dir)
    if not remote_path.exists():
        raise SystemExit(
            f"No REMOTE.md profile found at {remote_path}. "
            f"Create it or add a default template under {TEMPLATE_REMOTES_DIR / host / 'REMOTE.md'}."
        )
    remote_md = remote_path.read_text(encoding="utf-8")
    context: dict[str, Any] = {
        "host": host,
        "data_dir": str(data_dir),
        "profile_path": str(remote_path),
        "template_path": str(TEMPLATE_REMOTES_DIR / host / "REMOTE.md"),
        "remote_md": remote_md,
        "live": {},
    }
    if probe:
        context["live"]["ssh"] = probe_ssh(host, timeout)
        context["live"]["tailscale"] = probe_tailscale(host, timeout)
        context["live"]["syslog"] = probe_syslog(host, timeout)
    return context


def render_command_result(result: dict[str, Any]) -> str:
    status = "ok" if result.get("ok") else "failed"
    stdout = result.get("stdout") or ""
    stderr = result.get("stderr") or ""
    body = stdout if stdout else stderr
    if not body:
        body = "(no output)"
    return f"_status: {status}_\n\n```text\n{body}\n```"


def render_markdown(context: dict[str, Any]) -> str:
    lines = [
        f"# Plexus Remote Context: {context['host']}",
        "",
        f"Profile: `{context['profile_path']}`",
        "",
        "## Durable REMOTE.md Memory",
        "",
        context["remote_md"].rstrip(),
        "",
    ]

    live = context.get("live") or {}
    if not live:
        lines.extend(["## Live Context", "", "Live probes were skipped."])
        return "\n".join(lines).rstrip() + "\n"

    lines.extend(["## Live SSH Context", ""])
    for name, result in (live.get("ssh") or {}).items():
        lines.extend([f"### {name}", "", render_command_result(result), ""])

    tailscale = live.get("tailscale")
    if tailscale is not None:
        lines.extend(["## Tailscale", "", "```json", json.dumps(tailscale, indent=2), "```", ""])

    syslog = live.get("syslog")
    if syslog is not None:
        lines.extend(["## syslog-mcp", ""])
        for name, result in syslog.items():
            lines.extend([f"### {name}", "", render_command_result(result), ""])

    return "\n".join(lines).rstrip() + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("host", nargs="?", help="Remote host profile name under remotes/<host>/REMOTE.md")
    parser.add_argument("--format", choices=["markdown", "json"], default="markdown")
    parser.add_argument("--json", action="store_true", help="Alias for --format json")
    parser.add_argument("--init", action="store_true", help="Seed missing persistent REMOTE.md profiles from bundled templates")
    parser.add_argument("--no-probe", action="store_true", help="Read REMOTE.md without SSH/Tailscale/syslog probes")
    parser.add_argument("--data-dir", help="Override persistent data dir; defaults to PLEXUS_DATA_DIR, CLAUDE_PLUGIN_DATA, CODEX_PLUGIN_DATA, then ~/.plexus")
    parser.add_argument("--timeout", type=int, default=6, help="Per-command timeout in seconds")
    args = parser.parse_args()

    data_dir = plugin_data_dir(args.data_dir)
    output_format = "json" if args.json else args.format
    if args.init:
        copied = bootstrap_profiles(data_dir)
        payload = {"data_dir": str(data_dir), "copied": copied}
        if output_format == "json":
            print(json.dumps(payload, indent=2))
        else:
            print(f"Plexus data dir: {data_dir}")
            if copied:
                for entry in copied:
                    print(f"Seeded {entry['host']}: {entry['to']}")
            else:
                print("No profiles needed seeding.")
        return 0

    if not args.host:
        parser.error("host is required unless --init is used")

    context = build_context(args.host, probe=not args.no_probe, timeout=args.timeout, data_dir=data_dir)
    if output_format == "json":
        print(json.dumps(context, indent=2))
    else:
        print(render_markdown(context), end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
