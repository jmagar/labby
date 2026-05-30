#!/usr/bin/env python3
"""Audit CLI references in local agent skills."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
from collections import Counter, defaultdict
from dataclasses import asdict, dataclass
from pathlib import Path


HOME = Path.home()
DEFAULT_ROOTS = [
    HOME / ".claude",
    HOME / ".codex" / "skills",
    HOME / ".codex" / "plugins" / "cache",
    HOME / ".config" / "github-copilot",
    HOME / ".config" / "gh",
    HOME / ".agents" / "shared" / "skills",
]

TEXT_EXTENSIONS = {
    ".md",
    ".markdown",
    ".yaml",
    ".yml",
    ".json",
    ".toml",
    ".sh",
    ".bash",
    ".zsh",
}

SHELL_KEYWORDS = {
    "alias",
    "break",
    "case",
    "cat",
    "cd",
    "command",
    "continue",
    "do",
    "done",
    "echo",
    "elif",
    "else",
    "env",
    "eval",
    "exec",
    "exit",
    "export",
    "fi",
    "for",
    "function",
    "if",
    "in",
    "local",
    "mkdir",
    "popd",
    "printf",
    "pushd",
    "pwd",
    "read",
    "return",
    "set",
    "shift",
    "source",
    "test",
    "then",
    "time",
    "trap",
    "type",
    "ulimit",
    "umask",
    "unset",
    "until",
    "while",
    "xargs",
}

COMMON_NON_DEPS = {
    "awk",
    "basename",
    "chmod",
    "chown",
    "cp",
    "curl",
    "cut",
    "date",
    "dirname",
    "find",
    "grep",
    "head",
    "jq",
    "ln",
    "ls",
    "mv",
    "python",
    "python3",
    "rm",
    "sed",
    "sort",
    "tail",
    "tee",
    "touch",
    "tr",
    "wc",
    "which",
}

PROSE_TOKENS = {
    "active",
    "allowed-tools",
    "disabled",
    "installed",
    "unknown",
    "version",
}

KNOWN_CLI_TOKENS = {
    "cargo",
    "claude",
    "codex",
    "copilot",
    "docker",
    "gh",
    "git",
    "go",
    "lab",
    "make",
    "node",
    "npm",
    "pnpm",
    "python3",
    "rtk",
    "rustc",
    "uv",
    "yarn",
}

CODE_FENCE_RE = re.compile(r"```(?:bash|sh|zsh|shell|console)?\n(.*?)```", re.DOTALL)
INLINE_CODE_RE = re.compile(r"`([^`\n]+)`")
YAML_BANG_RE = re.compile(r"!\s*`([^`\n]+)`")
ALLOWED_TOOLS_RE = re.compile(r"allowed-tools:\s*([^\n]+)", re.IGNORECASE)
COMMANDISH_RE = re.compile(r"\b([a-zA-Z][a-zA-Z0-9_.+-]{1,48})(?:\s|$)")


@dataclass
class CliRef:
    ecosystem: str
    status: str
    skill: str
    skill_path: str
    cli: str
    source: str
    resolution: str
    all_resolutions: list[str]
    version: str


def run(argv: list[str], timeout: float = 3.0) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            argv,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return 1, str(exc)
    return proc.returncode, proc.stdout.strip()


def discover_skill_files(roots: list[Path]) -> list[Path]:
    files: list[Path] = []
    for root in roots:
        if not root.exists():
            continue
        files.extend(root.rglob("SKILL.md"))
        files.extend(root.rglob("skill.md"))
    return sorted(set(files))


def ecosystem_for(path: Path) -> str:
    text = str(path)
    if "/.claude/" in text or "/claude-" in text:
        return "claude"
    if "/.codex/" in text or "/.agents/" in text:
        return "codex"
    if "copilot" in text or "/.config/gh/" in text:
        return "copilot"
    return "unknown"


def status_for(path: Path, name: str, active_names: set[str], disabled_names: set[str]) -> str:
    if name in disabled_names or path.parent.name in disabled_names:
        return "disabled"
    if name in active_names or path.parent.name in active_names:
        return "active"
    parts = [p.lower() for p in path.parts]
    joined = "/".join(parts)
    if any(p.endswith(".disabled") for p in parts):
        return "disabled"
    if any(p in {"disabled", ".disabled", "inactive"} for p in parts):
        return "disabled"
    if "/.agents/shared/skills/" in joined:
        return "active"
    if "/.codex/skills/.system/" in joined:
        return "active"
    if "/.codex/skills/" in joined and "/plugins/cache/" not in joined:
        return "active"
    if "/plugins/cache/" in joined:
        return "installed"
    return "unknown"


def skill_name(path: Path) -> str:
    try:
        text = path.read_text(errors="ignore")
    except OSError:
        return path.parent.name
    # Scope to the YAML frontmatter block so a later `name:` line in prose
    # or examples can't shadow the real value.
    fm_match = re.match(r"^---\s*\n(.*?)\n---\s*\n", text, re.DOTALL)
    scope = fm_match.group(1) if fm_match else text
    match = re.search(r"^name:\s*([^\n]+)$", scope, re.MULTILINE)
    if not match:
        return path.parent.name
    return match.group(1).strip().strip('"').strip("'")


def first_command(line: str) -> str | None:
    line = line.strip()
    if not line or line.startswith("#"):
        return None
    line = re.sub(r"^(?:sudo|env|time|noglob)\s+", "", line)
    line = re.sub(r"^[A-Z_][A-Z0-9_]*=[^\s]+\s+", "", line)
    line = line.lstrip("$>").strip()
    if not line:
        return None
    match = COMMANDISH_RE.match(line)
    if not match:
        return None
    cmd = match.group(1)
    if cmd in SHELL_KEYWORDS:
        return None
    if "/" in cmd or cmd.startswith("."):
        return None
    return cmd


def extract_candidates(path: Path) -> dict[str, set[str]]:
    try:
        text = path.read_text(errors="ignore")
    except OSError:
        return {}

    candidates: dict[str, set[str]] = defaultdict(set)

    for match in CODE_FENCE_RE.finditer(text):
        for line in match.group(1).splitlines():
            cmd = first_command(line)
            if cmd:
                candidates[cmd].add(f"{path.name}:code-fence")

    for match in YAML_BANG_RE.finditer(text):
        cmd = first_command(match.group(1))
        if cmd:
            candidates[cmd].add(f"{path.name}:yaml-shell")

    for match in INLINE_CODE_RE.finditer(text):
        snippet = match.group(1).strip()
        if " " in snippet or snippet.startswith(("/", "$", "./")):
            cmd = first_command(snippet)
            if cmd:
                candidates[cmd].add(f"{path.name}:inline")
            continue
        if re.fullmatch(r"[a-zA-Z][a-zA-Z0-9_.+-]{1,48}", snippet):
            if snippet in PROSE_TOKENS or "_" in snippet:
                continue
            if "-" in snippet or snippet in KNOWN_CLI_TOKENS:
                candidates[snippet].add(f"{path.name}:inline-token")

    for match in ALLOWED_TOOLS_RE.finditer(text):
        for token in re.split(r"[, ]+", match.group(1)):
            token = token.strip().strip("[]")
            if token and token.lower() not in {"read", "write", "edit", "bash"}:
                candidates[token].add(f"{path.name}:allowed-tools")

    return candidates


def which_all(cli: str) -> list[str]:
    rc, out = run(["which", "-a", cli])
    if rc != 0:
        return []
    return [line for line in out.splitlines() if line.strip()]


def version_for(cli: str, resolved: str) -> str:
    probes = [
        [resolved, "--version"],
        [resolved, "version"],
        [resolved, "-V"],
    ]
    for probe in probes:
        rc, out = run(probe)
        if rc == 0 and out:
            first = out.splitlines()[0].strip()
            return first[:180]
    return ""


def audit(
    roots: list[Path],
    include_common: bool,
    active_names: set[str],
    disabled_names: set[str],
) -> list[CliRef]:
    refs: list[CliRef] = []
    for skill_file in discover_skill_files(roots):
        ecosystem = ecosystem_for(skill_file)
        name = skill_name(skill_file)
        status = status_for(skill_file, name, active_names, disabled_names)
        candidates = extract_candidates(skill_file)
        for cli, sources in sorted(candidates.items()):
            if not include_common and cli in COMMON_NON_DEPS:
                continue
            if cli in SHELL_KEYWORDS:
                continue
            resolutions = which_all(cli)
            resolved = shutil.which(cli) or ""
            refs.append(
                CliRef(
                    ecosystem=ecosystem,
                    status=status,
                    skill=name,
                    skill_path=str(skill_file),
                    cli=cli,
                    source=", ".join(sorted(sources)),
                    resolution=resolved,
                    all_resolutions=resolutions,
                    version=version_for(cli, resolved) if resolved else "",
                )
            )
    return refs


def write_markdown(path: Path, refs: list[CliRef]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    by_key: dict[tuple[str, str], list[CliRef]] = defaultdict(list)
    for ref in refs:
        by_key[(ref.cli, ref.resolution)].append(ref)

    missing = [ref for ref in refs if not ref.resolution]
    suspicious = [
        ref
        for ref in refs
        if ref.resolution and len(ref.all_resolutions) > 1
    ]
    counts = Counter((ref.ecosystem, ref.status) for ref in refs)
    ecosystems = sorted({ref.ecosystem for ref in refs})

    lines = ["# Skill CLI Audit", ""]
    lines.extend(["## Summary", ""])
    lines.append("| ecosystem | active | installed | disabled | unknown | cli refs | missing |")
    lines.append("| --- | ---: | ---: | ---: | ---: | ---: | ---: |")
    for eco in ecosystems:
        total = sum(1 for ref in refs if ref.ecosystem == eco)
        miss = sum(1 for ref in missing if ref.ecosystem == eco)
        lines.append(
            f"| {eco} | {counts[(eco, 'active')]} | {counts[(eco, 'installed')]} | "
            f"{counts[(eco, 'disabled')]} | {counts[(eco, 'unknown')]} | {total} | {miss} |"
        )

    lines.extend(["", "## Findings", "", "### Missing", ""])
    if missing:
        for cli, items in sorted(group_by_cli(missing).items()):
            skill_list = "; ".join(sorted({f"{i.skill} ({i.ecosystem}/{i.status})" for i in items}))
            lines.append(f"- `{cli}`: referenced by {skill_list}")
    else:
        lines.append("- None found.")

    lines.extend(["", "### Multiple Resolutions", ""])
    if suspicious:
        for ref in suspicious:
            all_paths = ", ".join(f"`{p}`" for p in ref.all_resolutions)
            lines.append(f"- `{ref.cli}` for `{ref.skill}` resolves to `{ref.resolution}`; all: {all_paths}")
    else:
        lines.append("- None found.")

    lines.extend(["", "## Inventory", ""])
    lines.append("| ecosystem | status | skill | cli | resolution | version | source |")
    lines.append("| --- | --- | --- | --- | --- | --- | --- |")
    for ref in refs:
        resolution = f"`{ref.resolution}`" if ref.resolution else "MISSING"
        version = ref.version.replace("|", "\\|") if ref.version else ""
        lines.append(
            f"| {ref.ecosystem} | {ref.status} | `{ref.skill}` | `{ref.cli}` | "
            f"{resolution} | {version} | {ref.source} |"
        )

    path.write_text("\n".join(lines) + "\n")


def group_by_cli(refs: list[CliRef]) -> dict[str, list[CliRef]]:
    grouped: dict[str, list[CliRef]] = defaultdict(list)
    for ref in refs:
        grouped[ref.cli].append(ref)
    return grouped


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", action="append", type=Path, help="Additional or replacement skill root")
    parser.add_argument("--only-root", action="store_true", help="Use only --root paths")
    parser.add_argument("--output", type=Path, help="Markdown report path")
    parser.add_argument("--json", dest="json_path", type=Path, help="JSON report path")
    parser.add_argument("--include-common", action="store_true", help="Include common POSIX utilities")
    parser.add_argument("--active-skill", action="append", default=[], help="Skill name known to be active")
    parser.add_argument("--active-skills-file", type=Path, help="Newline-delimited active skill names")
    parser.add_argument("--disabled-skill", action="append", default=[], help="Skill name known to be disabled")
    parser.add_argument("--disabled-skills-file", type=Path, help="Newline-delimited disabled skill names")
    args = parser.parse_args()

    roots = args.root or []
    if not args.only_root:
        roots = DEFAULT_ROOTS + roots
    roots = [root.expanduser() for root in roots]

    active_names = load_names(args.active_skill, args.active_skills_file)
    disabled_names = load_names(args.disabled_skill, args.disabled_skills_file)

    refs = audit(
        roots,
        include_common=args.include_common,
        active_names=active_names,
        disabled_names=disabled_names,
    )
    if args.output:
        write_markdown(args.output.expanduser(), refs)
        print(f"Wrote markdown report: {args.output.expanduser()}")
    if args.json_path:
        args.json_path.expanduser().parent.mkdir(parents=True, exist_ok=True)
        args.json_path.expanduser().write_text(json.dumps([asdict(ref) for ref in refs], indent=2) + "\n")
        print(f"Wrote JSON report: {args.json_path.expanduser()}")
    if not args.output and not args.json_path:
        print(json.dumps([asdict(ref) for ref in refs], indent=2))

    missing_count = sum(1 for ref in refs if not ref.resolution)
    print(f"Scanned {len(refs)} CLI references; missing={missing_count}")
    return 1 if missing_count else 0


def load_names(values: list[str], file_path: Path | None) -> set[str]:
    names = {value.strip() for value in values if value.strip()}
    if file_path:
        try:
            for line in file_path.expanduser().read_text(errors="ignore").splitlines():
                line = line.strip()
                if line and not line.startswith("#"):
                    names.add(line)
        except OSError as exc:
            raise SystemExit(f"Failed to read {file_path}: {exc}") from exc
    return names


if __name__ == "__main__":
    raise SystemExit(main())
