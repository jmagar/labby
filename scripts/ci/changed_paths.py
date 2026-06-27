#!/usr/bin/env python3
"""Classify changed files into Lab CI routing categories."""

from __future__ import annotations

import argparse
import os
import subprocess
from collections.abc import Callable
from pathlib import Path


OUTPUT_KEYS = [
    "all",
    "docs",
    "docs_check",
    "workflow",
    "rust_compile",
    "rust_test",
    "web",
    "docker",
    "security",
    "release",
]


def starts(path: str, *prefixes: str) -> bool:
    return any(path == prefix.rstrip("/") or path.startswith(prefix) for prefix in prefixes)


def any_match(paths: list[str], predicate: Callable[[str], bool]) -> bool:
    return any(predicate(path) for path in paths)


def classify(event: str, paths: list[str]) -> dict[str, bool]:
    if event in {"schedule", "workflow_dispatch"}:
        return {key: True for key in OUTPUT_KEYS}

    if not paths:
        return {key: True for key in OUTPUT_KEYS}

    workflow = any_match(
        paths,
        lambda p: starts(p, ".github/workflows/", ".github/actions/")
        or p
        in {
            ".github/actionlint.yaml",
            "scripts/ci/changed_paths.py",
            "crates/labby/tests/ci_changed_paths.rs",
        },
    )
    docs = any_match(
        paths,
        lambda p: starts(p, "docs/")
        or p in {"README.md", "CHANGELOG.md", "CLAUDE.md", "AGENTS.md", "GEMINI.md"},
    )
    docs_check = docs or any_match(
        paths,
        lambda p: starts(p, "docs/generated/")
        or p
        in {
            "crates/labby/tests/ci_changed_paths.rs",
            "scripts/ci/changed_paths.py",
            "Justfile",
        },
    )
    web = any_match(paths, lambda p: starts(p, "apps/gateway-admin/"))
    rust_sources = any_match(
        paths,
        lambda p: starts(
            p,
            "crates/",
            "tests/",
            ".cargo/",
        )
    )
    rust_manifests = any_match(
        paths,
        lambda p: p
        in {
            "Cargo.toml",
            "Cargo.lock",
            "Justfile",
            "rust-toolchain.toml",
            "build.rs",
            "clippy.toml",
            "deny.toml",
        },
    )
    rust_compile = rust_sources or rust_manifests
    rust_test = rust_sources
    security = any_match(
        paths,
        lambda p: p in {"Cargo.lock", "deny.toml", ".gitleaksignore"} or starts(p, ".cargo/"),
    )
    security = security or rust_sources
    docs_check = docs_check or rust_sources
    docker_inputs = any_match(
        paths,
        lambda p: starts(p, "config/", "scripts/")
        or p
        in {
            ".dockerignore",
            ".env.example",
            "docker-compose.yml",
            "docker-compose.yaml",
            "docker-compose.prod.yml",
            "docker-compose.prod.yaml",
        },
    )
    docker = rust_compile or web or docker_inputs
    release = rust_compile or web or any_match(paths, lambda p: starts(p, "release/"))

    result = {
        "all": False,
        "docs": docs,
        "docs_check": docs_check,
        "workflow": workflow,
        "rust_compile": rust_compile,
        "rust_test": rust_test,
        "web": web,
        "docker": docker,
        "security": security,
        "release": release,
    }

    if workflow:
        for key in OUTPUT_KEYS:
            result[key] = True

    return result


def read_paths(path: Path) -> list[str]:
    if not path.exists():
        return []
    return [line.strip() for line in path.read_text().splitlines() if line.strip()]


def git_path_exists(rev: str) -> bool:
    return subprocess.run(
        ["git", "cat-file", "-e", rev],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode == 0


def git_output(*args: str) -> str:
    return subprocess.check_output(["git", *args], text=True, stderr=subprocess.DEVNULL).strip()


def resolve_paths(event: str) -> list[str]:
    if event in {"schedule", "workflow_dispatch"}:
        return []

    env = os.environ
    base = ""
    head = env.get("HEAD_SHA") or env.get("GITHUB_SHA") or "HEAD"

    if event == "pull_request":
        base = env.get("PR_BASE_SHA", "")
        head = env.get("PR_HEAD_SHA") or head
    elif event == "push":
        if env.get("GITHUB_REF", "").startswith("refs/tags/"):
            return []
        base = env.get("PUSH_BEFORE_SHA", "")
    else:
        return []

    if not base or set(base) == {"0"} or not git_path_exists(base):
        try:
            base = git_output("rev-parse", "HEAD^")
        except subprocess.CalledProcessError:
            base = ""

    if not base:
        return []

    try:
        raw = git_output("diff", "--name-only", base, head)
    except subprocess.CalledProcessError:
        return []

    return [line.strip() for line in raw.splitlines() if line.strip()]


def write_outputs(path: Path, values: dict[str, bool]) -> None:
    lines = [f"{key}={'true' if values[key] else 'false'}" for key in OUTPUT_KEYS]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--event", required=True)
    parser.add_argument("--changed-files", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--write-changed-files", type=Path)
    args = parser.parse_args()

    paths = read_paths(args.changed_files) if args.changed_files else resolve_paths(args.event)
    if args.write_changed_files:
        args.write_changed_files.write_text("\n".join(paths) + ("\n" if paths else ""))

    values = classify(args.event, paths)
    write_outputs(args.output, values)
    for key in OUTPUT_KEYS:
        print(f"{key}={str(values[key]).lower()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
