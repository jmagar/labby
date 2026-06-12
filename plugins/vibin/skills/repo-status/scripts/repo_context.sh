#!/usr/bin/env bash
set -u

no_fetch="${REPO_STATUS_NO_FETCH:-0}"
json=0
include_gh=0
focus_branch=""
output_path=""
force_output=0
max_branches=""

usage() {
  cat <<'USAGE'
Usage: repo_context.sh [--no-fetch] [--json] [--branch <name>] [--include-gh] [--output <file>] [--force-output] [--max-branches <n>]

Collect read-only Git context for the repo-status skill.

Options:
  --no-fetch     Skip the networked git fetch --dry-run step.
  --json         Emit a machine-readable JSON snapshot.
  --branch NAME  Include focused branch context for NAME.
  --include-gh   Include per-branch GitHub PR/CI summaries when gh is available.
  --output FILE   Write output to FILE instead of stdout; fails if FILE exists.
  --force-output  Allow --output to overwrite an existing file.
  --max-branches N
                 Limit detailed per-branch diff/PR collection to N branches.
  -h, --help     Show this help.
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --no-fetch)
      no_fetch=1
      shift
      ;;
    --json)
      json=1
      shift
      ;;
    --branch)
      if [ "$#" -lt 2 ]; then
        printf 'error: --branch requires a value\n' >&2
        exit 2
      fi
      focus_branch="$2"
      shift 2
      ;;
    --include-gh)
      include_gh=1
      shift
      ;;
    --output)
      if [ "$#" -lt 2 ]; then
        printf 'error: --output requires a value\n' >&2
        exit 2
      fi
      output_path="$2"
      shift 2
      ;;
    --force-output)
      force_output=1
      shift
      ;;
    --max-branches)
      if [ "$#" -lt 2 ]; then
        printf 'error: --max-branches requires a value\n' >&2
        exit 2
      fi
      max_branches="$2"
      case "$max_branches" in
        ''|*[!0-9]*)
          printf 'error: --max-branches must be a non-negative integer\n' >&2
          exit 2
          ;;
      esac
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

root=$(git rev-parse --show-toplevel 2>/dev/null) || {
  printf 'error: not inside a Git repository\n' >&2
  exit 2
}

if [ -n "$output_path" ] && [ -e "$output_path" ] && [ "$force_output" -ne 1 ]; then
  printf 'error: output file already exists: %s (use --force-output to overwrite)\n' "$output_path" >&2
  exit 2
fi

REPO_STATUS_ROOT="$root" \
REPO_STATUS_NO_FETCH="$no_fetch" \
REPO_STATUS_INCLUDE_GH="$include_gh" \
REPO_STATUS_BRANCH="$focus_branch" \
REPO_STATUS_JSON="$json" \
REPO_STATUS_OUTPUT="$output_path" \
REPO_STATUS_FORCE_OUTPUT="$force_output" \
REPO_STATUS_MAX_BRANCHES="$max_branches" \
python3 - <<'PY'
import json
import os
import re
import subprocess
import sys
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

root = os.environ["REPO_STATUS_ROOT"]
no_fetch = os.environ.get("REPO_STATUS_NO_FETCH") == "1"
include_gh = os.environ.get("REPO_STATUS_INCLUDE_GH") == "1"
focus_branch = os.environ.get("REPO_STATUS_BRANCH", "")
json_mode = os.environ.get("REPO_STATUS_JSON") == "1"
output_path = os.environ.get("REPO_STATUS_OUTPUT", "")
max_branches_raw = os.environ.get("REPO_STATUS_MAX_BRANCHES", "")
branch_limit = int(max_branches_raw) if max_branches_raw else None

if output_path:
    output_file = open(output_path, "w", encoding="utf-8")
    sys.stdout = output_file

def run(label, args, cwd=root):
    proc = subprocess.run(args, cwd=cwd, text=True, capture_output=True)
    return {
        "label": label,
        "command": args,
        "exit": proc.returncode,
        "stdout": proc.stdout,
        "stderr": proc.stderr,
    }

def out(args, cwd=root):
    return run(" ".join(args), args, cwd)["stdout"].strip()

def ref_exists(ref):
    return subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", "--end-of-options", ref],
        cwd=root,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode == 0

def remotes():
    text = out(["git", "remote"])
    return [line for line in text.splitlines() if line]

def current_upstream_remote():
    upstream = out(["git", "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
    if "/" in upstream:
        return upstream.split("/", 1)[0]
    return ""

def default_base():
    available_remotes = remotes()
    preferred = current_upstream_remote()
    ordered_remotes = []
    for remote in [preferred, "origin", *available_remotes]:
        if remote and remote not in ordered_remotes:
            ordered_remotes.append(remote)

    for remote in ordered_remotes:
        head = out(["git", "symbolic-ref", "--quiet", "--short", f"refs/remotes/{remote}/HEAD"])
        if head:
            return {"ref": head, "rationale": f"{remote}/HEAD"}

    for remote in ordered_remotes:
        for name in ["main", "master", "trunk", "develop"]:
            candidate = f"{remote}/{name}"
            if ref_exists(candidate):
                return {"ref": candidate, "rationale": f"first existing remote default candidate on {remote}"}

    for candidate in ["main", "master", "trunk", "develop"]:
        if ref_exists(candidate):
            return {"ref": candidate, "rationale": "first existing local default candidate"}

    return {"ref": "", "rationale": "no default base found"}

def parse_worktrees(text):
    records = []
    current = None
    for line in text.splitlines():
        if not line:
            if current:
                records.append(current)
                current = None
            continue
        key, _, value = line.partition(" ")
        if key == "worktree":
            if current:
                records.append(current)
            current = {"path": value, "locked": False, "prunable": False, "detached": False, "bare": False}
        elif current is not None:
            if key == "HEAD":
                current["head"] = value
            elif key == "branch":
                current["branch_ref"] = value
                current["branch"] = value.removeprefix("refs/heads/")
            elif key == "detached":
                current["detached"] = True
            elif key == "locked":
                current["locked"] = True
                current["locked_reason"] = value
            elif key == "prunable":
                current["prunable"] = True
                current["prunable_reason"] = value
            elif key == "bare":
                current["bare"] = True
    if current:
        records.append(current)
    return records

def branch_inventory():
    result = run("branch inventory", [
        "git", "for-each-ref", "refs/heads",
        "--format=%(refname:short)%09%(objectname)%09%(upstream:short)%09%(upstream:track)%09%(committerdate:iso8601)%09%(worktreepath)%09%(subject)",
    ])
    branches = []
    for line in result["stdout"].splitlines():
        parts = line.split("\t", 6)
        while len(parts) < 7:
            parts.append("")
        branches.append({
            "name": parts[0],
            "object": parts[1],
            "upstream": parts[2],
            "upstream_track": parts[3],
            "committerdate": parts[4],
            "worktreepath": parts[5],
            "subject": parts[6],
        })
    return result, branches

def integration_ref(base_name):
    if not base_name:
        return ""
    for candidate in [f"origin/{base_name}", base_name]:
        if ref_exists(candidate):
            return candidate
    return base_name

def github_context(branch_names):
    if not include_gh:
        return None, {}
    if subprocess.run(["sh", "-c", "command -v gh >/dev/null 2>&1"], cwd=root).returncode != 0:
        return {"skipped": "gh not found"}, {}

    gh = {
        "pr_list": run("gh pr list", ["gh", "pr", "list", "--state", "open", "--limit", "200", "--json",
                                      "number,title,headRefName,baseRefName,isDraft,mergeable,reviewDecision,statusCheckRollup,updatedAt,url"]),
        "run_list": run("gh run list", ["gh", "run", "list", "--limit", "50", "--json",
                                        "databaseId,headBranch,headSha,status,conclusion,workflowName,updatedAt,url"]),
        "branches": {},
        "evidence_note": "run_list entries are branch-level evidence unless latest_run_for_head is populated with a matching headSha.",
    }
    pr_heads = set()
    try:
        for pr in json.loads(gh["pr_list"]["stdout"] or "[]"):
            head = pr.get("headRefName")
            if head:
                pr_heads.add(head)
    except json.JSONDecodeError:
        pass

    for branch in sorted(set(branch_names) | pr_heads | ({focus_branch} if focus_branch else set())):
        if not branch:
            continue
        branch_data = {
            "pr_view": run(f"gh pr view {branch}", ["gh", "pr", "view", branch, "--json",
                                                    "number,title,headRefName,baseRefName,isDraft,mergeable,reviewDecision,statusCheckRollup,reviews,comments,updatedAt,url"]),
            "run_list": run(f"gh run list {branch}", ["gh", "run", "list", "--branch", branch, "--limit", "10", "--json",
                                                      "databaseId,headBranch,headSha,status,conclusion,workflowName,updatedAt,url"]),
            "latest_run_for_head": None,
        }
        head = out(["git", "rev-parse", "--verify", "--end-of-options", branch])
        if head:
            try:
                runs = json.loads(branch_data["run_list"]["stdout"] or "[]")
                for run_item in runs:
                    if run_item.get("headSha") == head:
                        branch_data["latest_run_for_head"] = run_item
                        break
            except json.JSONDecodeError:
                pass
        gh["branches"][branch] = branch_data
    pr_base_by_branch = {}
    for branch, data in gh["branches"].items():
        if data["pr_view"]["exit"] != 0:
            continue
        try:
            pr = json.loads(data["pr_view"]["stdout"] or "{}")
        except json.JSONDecodeError:
            continue
        if pr.get("baseRefName"):
            pr_base_by_branch[branch] = integration_ref(pr["baseRefName"])
    return gh, pr_base_by_branch

TEST_RE = re.compile(r"(^|/)(tests?|spec|__tests__)/|(_test|\\.test|\\.spec)\\.")
LOCK_RE = re.compile(r"(^|/)(Cargo\\.lock|package-lock\\.json|pnpm-lock\\.yaml|yarn\\.lock|uv\\.lock|poetry\\.lock|go\\.sum|Gemfile\\.lock)$")
SENSITIVE_RE = re.compile(r"(^|/)(\\.env|.*secret.*|.*token.*|.*auth.*|config|configs|\\.github|deploy|deployment|migrations?)(/|$)", re.I)
GENERATED_RE = re.compile(r"(^|/)(dist|build|target|generated|vendor|node_modules|\\.next)(/|$)")
BINARY_RE = re.compile(r"\\.(png|jpg|jpeg|gif|webp|ico|pdf|zip|gz|tar|mp4|mov|wasm|jar|bin)$", re.I)

def risk_signals(branch, base_ref, diff_names, diff_status):
    names = [line for line in diff_names["stdout"].splitlines() if line]
    statuses = [line.split("\t", 1) for line in diff_status["stdout"].splitlines() if line]
    deleted = [rest for status, rest in statuses if status.startswith("D")]
    grep = run(f"risk grep {branch}", ["git", "diff", "-GTODO|FIXME|WIP|XXX", "--name-only", f"{base_ref}...{branch}"]) if base_ref else {"stdout": "", "exit": 0}
    return {
        "todo_wip_files": [line for line in grep["stdout"].splitlines() if line],
        "test_files_changed": [name for name in names if TEST_RE.search(name)],
        "test_files_deleted": [name for name in deleted if TEST_RE.search(name)],
        "lockfiles_changed": [name for name in names if LOCK_RE.search(name)],
        "generated_or_binary_changed": [name for name in names if GENERATED_RE.search(name) or BINARY_RE.search(name)],
        "sensitive_paths_changed": [name for name in names if SENSITIVE_RE.search(name)],
    }

base_info = default_base()
base_ref = base_info["ref"]
commands = [
    run("pwd", ["pwd"]),
    run("git root", ["git", "rev-parse", "--show-toplevel"]),
    run("status", ["git", "status", "--short", "--branch"]),
    run("status porcelain v2", ["git", "status", "--porcelain=v2", "--branch"]),
    run("current branch", ["git", "branch", "--show-current"]),
    run("worktrees", ["git", "worktree", "list", "--porcelain"]),
    run("branches", ["git", "branch", "--all", "--verbose", "--no-abbrev"]),
]
branch_inventory_command, branches = branch_inventory()
commands.append(branch_inventory_command)
commands.extend([
    run("remotes", ["git", "remote", "-v"]),
    run("default remote head", ["git", "symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]),
])
if no_fetch:
    commands.append({"label": "fetch dry run", "command": ["git", "fetch", "--all", "--prune", "--dry-run"], "exit": 0,
                     "stdout": "[skipped: --no-fetch or REPO_STATUS_NO_FETCH=1]\n", "stderr": ""})
else:
    commands.append(run("fetch dry run", ["git", "fetch", "--all", "--prune", "--dry-run"]))

branch_names = [b["name"] for b in branches]

def preflight_pr_heads():
    if not include_gh:
        return set()
    if subprocess.run(["sh", "-c", "command -v gh >/dev/null 2>&1"], cwd=root).returncode != 0:
        return set()
    result = run("gh pr head preflight", ["gh", "pr", "list", "--state", "open", "--limit", "200", "--json", "headRefName"])
    try:
        return {pr["headRefName"] for pr in json.loads(result["stdout"] or "[]") if pr.get("headRefName")}
    except json.JSONDecodeError:
        return set()

def parse_worktree_branches():
    return {wt.get("branch", "") for wt in parse_worktrees(commands[5]["stdout"]) if wt.get("branch")}

def date_sort_key(branch):
    text = branch.get("committerdate", "")
    try:
        return datetime.strptime(text, "%Y-%m-%d %H:%M:%S %z").timestamp()
    except ValueError:
        return 0

def priority_branch_names():
    current = out(["git", "branch", "--show-current"])
    priority = []
    for name in [current, *sorted(parse_worktree_branches()), focus_branch, *sorted(preflight_pr_heads())]:
        if name and name not in priority:
            priority.append(name)
    return priority

priority_names = priority_branch_names()
by_name = {branch["name"]: branch for branch in branches}
ordered_branches = [by_name[name] for name in priority_names if name in by_name]
remaining = [branch for branch in branches if branch["name"] not in {b["name"] for b in ordered_branches}]
remaining.sort(key=lambda branch: (date_sort_key(branch), branch["name"]), reverse=True)
detail_branches = ordered_branches + remaining
if branch_limit is not None:
    detail_branches = detail_branches[:branch_limit]
detail_branch_names = [b["name"] for b in detail_branches]
github, pr_base_by_branch = github_context(detail_branch_names)

changed_by_branch = {}
branch_details = {}
def parse_ahead_behind(command):
    if not command or command.get("exit") != 0:
        return {"ahead": None, "behind": None}
    parts = command["stdout"].strip().split()
    if len(parts) != 2:
        return {"ahead": None, "behind": None}
    return {"behind": int(parts[0]), "ahead": int(parts[1])}

def days_since(date_text):
    if not date_text:
        return None
    try:
        parsed = datetime.strptime(date_text, "%Y-%m-%d %H:%M:%S %z")
    except ValueError:
        return None
    return (datetime.now(timezone.utc) - parsed.astimezone(timezone.utc)).days

def same_named_remote_exists(name):
    return any(ref_exists(f"{remote}/{name}") for remote in remotes())

def upstream_branch_exists(branch):
    upstream = branch.get("upstream", "")
    if not upstream:
        return None
    return ref_exists(upstream)

for branch in detail_branches:
    name = branch["name"]
    chosen_base = pr_base_by_branch.get(name, base_ref)
    detail = {
        **branch,
        "base": chosen_base,
        "base_rationale": "PR base" if name in pr_base_by_branch else base_info["rationale"],
        "ahead_behind": None,
        "log": None,
        "diff_stat": None,
        "diff_names": None,
        "diff_status": None,
        "risk_signals": {},
    }
    if chosen_base and ref_exists(name) and ref_exists(chosen_base):
        detail["ahead_behind"] = run(f"ahead behind {name}", ["git", "rev-list", "--left-right", "--count", f"{chosen_base}...{name}"])
        detail["log"] = run(f"log {name}", ["git", "log", "--oneline", "--decorate", "--max-count=20", f"{chosen_base}..{name}"])
        detail["diff_stat"] = run(f"diff stat {name}", ["git", "diff", "--stat", f"{chosen_base}...{name}"])
        detail["diff_names"] = run(f"diff names {name}", ["git", "diff", "--name-only", f"{chosen_base}...{name}"])
        detail["diff_status"] = run(f"diff status {name}", ["git", "diff", "--name-status", f"{chosen_base}...{name}"])
        changed_by_branch[name] = set(detail["diff_names"]["stdout"].splitlines())
        detail["risk_signals"] = risk_signals(name, chosen_base, detail["diff_names"], detail["diff_status"])
    ab = parse_ahead_behind(detail["ahead_behind"])
    merged_into_base = None
    has_unique_commits = None
    if chosen_base and ref_exists(name) and ref_exists(chosen_base):
        merged_check = run(f"merged into base {name}", ["git", "merge-base", "--is-ancestor", name, chosen_base])
        merged_into_base = merged_check["exit"] == 0
        if ab["ahead"] is not None:
            has_unique_commits = ab["ahead"] > 0
    detail["stale_evidence"] = {
        "merged_into_base": merged_into_base,
        "days_since_last_commit": days_since(detail["committerdate"]),
        "has_unique_commits": has_unique_commits,
        "upstream_branch_exists": upstream_branch_exists(branch),
        "same_named_remote_exists": same_named_remote_exists(name),
        "worktree_missing_or_prunable": False,
    }
    branch_details[name] = detail

for branch in branches:
    if branch["name"] not in branch_details:
        branch_details[branch["name"]] = {
            **branch,
            "base": base_ref,
            "base_rationale": "not collected due to --max-branches",
            "ahead_behind": None,
            "log": None,
            "diff_stat": None,
            "diff_names": None,
            "diff_status": None,
            "risk_signals": {},
            "stale_evidence": {
                "merged_into_base": None,
                "days_since_last_commit": days_since(branch["committerdate"]),
                "has_unique_commits": None,
                "upstream_branch_exists": upstream_branch_exists(branch),
                "same_named_remote_exists": same_named_remote_exists(branch["name"]),
                "worktree_missing_or_prunable": False,
            },
            "limited": True,
        }

file_to_branches = defaultdict(set)
for branch, files in changed_by_branch.items():
    for filename in files:
        if filename:
            file_to_branches[filename].add(branch)
for branch, detail in branch_details.items():
    overlaps = sorted(filename for filename, owners in file_to_branches.items() if branch in owners and len(owners) > 1)
    detail["risk_signals"]["overlap_files"] = overlaps

worktrees = []
for wt in parse_worktrees(commands[5]["stdout"]):
    path = wt["path"]
    branch = wt.get("branch", "")
    wt["exists"] = Path(path).exists()
    wt["base"] = branch_details.get(branch, {}).get("base", base_ref)
    wt["branch_detail"] = branch_details.get(branch)
    wt["status"] = run(f"worktree status {path}", ["git", "-C", path, "status", "--short", "--branch"]) if wt["exists"] else None
    wt["status_porcelain_v2"] = run(f"worktree status porcelain {path}", ["git", "-C", path, "status", "--porcelain=v2", "--branch"]) if wt["exists"] else None
    worktrees.append(wt)

for wt in worktrees:
    branch = wt.get("branch", "")
    if branch in branch_details and branch_details[branch].get("stale_evidence"):
        branch_details[branch]["stale_evidence"]["worktree_missing_or_prunable"] = (not wt["exists"]) or wt.get("prunable", False)

focused = None
if focus_branch:
    chosen_base = pr_base_by_branch.get(focus_branch, base_ref)
    focused = {
        "branch": focus_branch,
        "base": chosen_base,
        "worktreepath": branch_details.get(focus_branch, {}).get("worktreepath", ""),
        "worktree_status": None,
        "rev_parse": run("focused branch rev-parse", ["git", "rev-parse", "--verify", "--end-of-options", focus_branch]),
    }
    worktreepath = focused["worktreepath"]
    if worktreepath:
        focused["worktree_status"] = run("focused branch worktree status", ["git", "-C", worktreepath, "status", "--porcelain=v2", "--branch"])
    if chosen_base:
        focused["merge_base"] = run("focused branch merge-base", ["git", "merge-base", chosen_base, focus_branch])
        focused["log"] = run("focused branch log", ["git", "log", "--oneline", "--decorate", "--max-count=20", f"{chosen_base}..{focus_branch}"])
        focused["diff_stat"] = run("focused branch diff stat", ["git", "diff", "--stat", f"{chosen_base}...{focus_branch}"])
        focused["diff_names"] = run("focused branch diff names", ["git", "diff", "--name-only", f"{chosen_base}...{focus_branch}"])

snapshot = {
    "schema": "repo-status.context.v2",
    "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "root": root,
    "default_base": base_ref,
    "default_base_rationale": base_info["rationale"],
    "no_fetch": no_fetch,
    "branch_limit": branch_limit,
    "branches_total": len(branches),
    "branches_collected": len(detail_branches),
    "branches_truncated": branch_limit is not None and len(branches) > len(detail_branches),
    "focus_branch": focus_branch or None,
    "commands": commands,
    "branches": list(branch_details.values()),
    "worktrees": worktrees,
    "focused_branch": focused,
    "github": github,
}

if json_mode:
    print(json.dumps(snapshot, indent=2))
    if output_path:
        output_file.close()
    sys.exit(0)

def emit_command(cmd):
    print(f"\n## {cmd['label']}")
    print("$ " + " ".join(cmd["command"]))
    if cmd["stdout"]:
        print(cmd["stdout"], end="" if cmd["stdout"].endswith("\n") else "\n")
    if cmd["stderr"]:
        print(cmd["stderr"], end="" if cmd["stderr"].endswith("\n") else "\n")
    if cmd["exit"] != 0:
        print(f"[exit {cmd['exit']}]")

print("# repo-status context")
print(f"generated_at: {snapshot['generated_at']}")
print(f"default_base: {snapshot['default_base']}")
print(f"default_base_rationale: {snapshot['default_base_rationale']}")
print(f"branches_collected: {snapshot['branches_collected']}/{snapshot['branches_total']}")
if snapshot["branches_truncated"]:
    print(f"branches_truncated: true (--max-branches {snapshot['branch_limit']})")
if focus_branch:
    print(f"focus_branch: {focus_branch}")
for cmd in commands:
    emit_command(cmd)
print("\n## structured branches")
for branch in snapshot["branches"]:
    print(f"- {branch['name']} {branch['object']} upstream={branch['upstream'] or '-'} track={branch['upstream_track'] or '-'} base={branch['base'] or '-'} worktree={branch['worktreepath'] or '-'}")
    risk = branch["risk_signals"]
    signals = [key for key, value in risk.items() if value]
    stale = branch.get("stale_evidence", {})
    print(f"  risk_signals={','.join(signals) if signals else '-'} stale={stale}")
print("\n## structured worktrees")
for wt in snapshot["worktrees"]:
    print(f"- path={wt['path']} branch={wt.get('branch', '-') or '-'} head={wt.get('head', '-') or '-'} exists={wt['exists']} locked={wt.get('locked', False)} prunable={wt.get('prunable', False)} base={wt.get('base') or '-'}")
    if wt.get("status_porcelain_v2") and wt["status_porcelain_v2"]["stdout"]:
        print(wt["status_porcelain_v2"]["stdout"], end="" if wt["status_porcelain_v2"]["stdout"].endswith("\n") else "\n")
if focused:
    print("\n## focused branch")
    print(f"branch={focused['branch']} base={focused['base'] or '-'} worktree={focused['worktreepath'] or '-'}")
    for key in ["rev_parse", "worktree_status", "merge_base", "log", "diff_stat", "diff_names"]:
        if focused.get(key):
            emit_command(focused[key])
if github is not None:
    print("\n## github")
    if github.get("skipped"):
        print(f"[skipped: {github['skipped']}]")
    else:
        emit_command(github["pr_list"])
        emit_command(github["run_list"])
        for branch, data in github["branches"].items():
            print(f"\n### github branch: {branch}")
            emit_command(data["pr_view"])
            emit_command(data["run_list"])
if output_path:
    output_file.close()
PY
