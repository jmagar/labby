#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

export LAB_HOME="$TMP/lab-home"
mkdir -p "$LAB_HOME"

cd "$ROOT"
cargo run --all-features -- --json gateway code exec --code '
async () => {
  await state.writeFile({ path: "/src/app.rs", content: "fn main() { println!(\"hi\"); }\n" });
  const read = await state.readFile({ path: "/src/app.rs" });
  const matches = await state.searchFiles({ pattern: "src/**/*.rs", query: "println" });
  await git.init({});
  await git.add({ path: "/src/app.rs" });
  await git.commit({ message: "initial state", authorName: "Lab", authorEmail: "lab@example.invalid" });
  const status = await git.status({});
  const log = await git.log({ limit: 1 });
  return { read: read.content, matches: matches.matches.length, status, log };
}
'
