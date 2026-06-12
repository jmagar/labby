#!/usr/bin/env bash
set -u

usage() {
  cat <<'USAGE'
Usage: check_mergeability.sh <base> <branch>

Check whether <branch> merges into <base> in a temporary worktree.
Prints conflicted files when the merge is not clean.
USAGE
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

if [ "$#" -ne 2 ]; then
  usage >&2
  exit 2
fi

base="$1"
branch="$2"

root=$(git rev-parse --show-toplevel 2>/dev/null) || {
  printf 'error: not inside a Git repository\n' >&2
  exit 2
}

cd "$root" || exit 2

printf '# repo-status mergeability\n'
printf 'base: %s\n' "$base"
printf 'branch: %s\n' "$branch"

git rev-parse --verify --quiet --end-of-options "$base" >/dev/null || {
  printf 'error: unknown base ref: %s\n' "$base" >&2
  printf 'mergeable: unknown\n'
  printf 'failure_kind: unknown_base_ref\n'
  exit 2
}

git rev-parse --verify --quiet --end-of-options "$branch" >/dev/null || {
  printf 'error: unknown branch/ref: %s\n' "$branch" >&2
  printf 'mergeable: unknown\n'
  printf 'failure_kind: unknown_branch_ref\n'
  exit 2
}

tmp_parent=$(mktemp -d)
tmp="$tmp_parent/worktree"
merge_output="$tmp_parent/merge.out"
add_output="$tmp_parent/worktree-add.out"

# shellcheck disable=SC2329 # Invoked by trap.
cleanup() {
  local cleanup_error=0
  git worktree remove --force "$tmp" >/dev/null 2>&1 || true
  rm -f "$merge_output" "$add_output" 2>/dev/null || cleanup_error=1
  rmdir "$tmp_parent" 2>/dev/null || {
    rm -rf "$tmp_parent" 2>/dev/null || cleanup_error=1
  }
  if [ "$cleanup_error" -ne 0 ]; then
    printf 'warning: cleanup incomplete for %s\n' "$tmp_parent" >&2
  fi
}
trap cleanup EXIT HUP INT TERM

if ! git worktree add --detach "$tmp" "$base" >"$add_output" 2>&1; then
  cat "$add_output"
  printf 'mergeable: unknown\n'
  printf 'failure_kind: worktree_add_failed\n'
  exit 1
fi

if git -C "$tmp" merge --no-commit --no-ff -- "$branch" >"$merge_output" 2>&1; then
  cat "$merge_output"
  printf 'mergeable: yes\n'
  exit 0
fi

cat "$merge_output"
printf 'mergeable: no\n'
conflicted_files=$(git -C "$tmp" diff --name-only --diff-filter=U)
if [ -z "$conflicted_files" ]; then
  printf 'failure_kind: merge_failed_without_unmerged_paths\n'
else
  printf 'failure_kind: conflicts\n'
fi
printf 'conflicted_files:\n'
printf '%s\n' "$conflicted_files" | sed '/^$/d; s/^/- /'
exit 1
