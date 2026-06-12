#!/usr/bin/env python3
"""Summarize repo_context.sh --json output for the repo-status skill."""

from __future__ import annotations

import json
import sys
from pathlib import Path


USAGE = """Usage: summarize_context.py [repo-status.json]

Summarize repo_context.sh --json output as a compact Markdown table.
Reads stdin when no file is provided.
"""


def load(path: str | None) -> dict:
    if path:
        source = Path(path)
        if not source.exists():
            raise SystemExit(f"error: JSON file not found: {source}")
        return json.loads(source.read_text(encoding="utf-8"))
    return json.load(sys.stdin)


def command_clean(command: dict | None) -> str:
    if not command:
        return "unknown"
    stdout = command.get("stdout", "")
    dirty = [line for line in stdout.splitlines() if line and not line.startswith("#")]
    return "dirty" if dirty else "clean"


def ahead_behind(command: dict | None) -> str:
    if not command or command.get("exit") != 0:
        return "unknown"
    parts = command.get("stdout", "").strip().split()
    if len(parts) != 2:
        return "unknown"
    return f"ahead {parts[1]} / behind {parts[0]}"


def risk_keys(branch: dict) -> str:
    risks = branch.get("risk_signals") or {}
    keys = [key for key, value in risks.items() if value]
    return ",".join(keys) if keys else "-"


def md_cell(value: object) -> str:
    text = "-" if value is None or value == "" else str(value)
    return text.replace("\\", "\\\\").replace("|", "\\|").replace("\n", "<br>")


def stale_summary(branch: dict) -> str:
    stale = branch.get("stale_evidence") or {}
    bits = []
    if stale.get("merged_into_base") is True:
        bits.append("merged")
    elif stale.get("merged_into_base") is None:
        bits.append("merged?")
    if stale.get("days_since_last_commit") is not None:
        bits.append(f"{stale['days_since_last_commit']}d")
    if stale.get("upstream_branch_exists") is False:
        bits.append("no-upstream")
    if stale.get("same_named_remote_exists") is False:
        bits.append("no-same-remote")
    if stale.get("worktree_missing_or_prunable"):
        bits.append("worktree-missing/prunable")
    return ",".join(bits) if bits else "-"


def branch_gh(data: dict, branch_name: str) -> dict:
    github = data.get("github") or {}
    return (github.get("branches") or {}).get(branch_name) or {}


def parse_json_command(command: dict | None, default: object) -> object:
    if not command or command.get("exit") != 0:
        return default
    try:
        return json.loads(command.get("stdout") or "")
    except json.JSONDecodeError:
        return default


def pr_summary(gh_branch: dict) -> tuple[str, str]:
    pr = parse_json_command(gh_branch.get("pr_view"), None)
    if not isinstance(pr, dict) or not pr:
        return "-", "-"
    number = pr.get("number")
    state = "draft" if pr.get("isDraft") else "open"
    mergeable = pr.get("mergeable") or "unknown"
    review = pr.get("reviewDecision") or "unknown"
    return f"#{number} {state} {mergeable}", review


def ci_summary(gh_branch: dict) -> str:
    latest = gh_branch.get("latest_run_for_head")
    exact = True
    if not latest:
        runs = parse_json_command(gh_branch.get("run_list"), [])
        latest = runs[0] if isinstance(runs, list) and runs else None
        exact = False
    if not latest:
        return "-"
    workflow = latest.get("workflowName") or "run"
    status = latest.get("conclusion") or latest.get("status") or "unknown"
    suffix = "" if exact else " branch"
    return f"{workflow}:{status}{suffix}"


def main() -> int:
    if len(sys.argv) > 2:
        raise SystemExit(USAGE.rstrip())
    if len(sys.argv) == 2 and sys.argv[1] in {"-h", "--help"}:
        print(USAGE.rstrip())
        return 0

    path = sys.argv[1] if len(sys.argv) > 1 else None
    data = load(path)
    print(f"Repo: {data.get('root', '-')}")
    print(f"Default base: {data.get('default_base') or '-'} ({data.get('default_base_rationale') or '-'})")
    print(f"Detailed branches: {data.get('branches_collected', len(data.get('branches', [])))}/{data.get('branches_total', len(data.get('branches', [])))}")
    print(f"Worktrees: {len(data.get('worktrees', []))}")
    print()
    print("| Branch | Scope | Worktree | Dirty | Ahead/Behind | PR | CI | Review | Stale Evidence | Risk Signals |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for branch in data.get("branches", []):
        name = branch.get("name", "-")
        worktree = branch.get("worktreepath") or "-"
        status = None
        for wt in data.get("worktrees", []):
            if wt.get("branch") == name:
                status = wt.get("status_porcelain_v2")
                break
        gh_branch = branch_gh(data, name)
        pr, review = pr_summary(gh_branch)
        print(
            "| {name} | {scope} | {worktree} | {dirty} | {ab} | {pr} | {ci} | {review} | {stale} | {risks} |".format(
                name=md_cell(name),
                scope=md_cell("limited" if branch.get("limited") else "detailed"),
                worktree=md_cell(worktree),
                dirty=md_cell(command_clean(status)),
                ab=md_cell(ahead_behind(branch.get("ahead_behind"))),
                pr=md_cell(pr),
                ci=md_cell(ci_summary(gh_branch)),
                review=md_cell(review),
                stale=md_cell(stale_summary(branch)),
                risks=md_cell(risk_keys(branch)),
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
