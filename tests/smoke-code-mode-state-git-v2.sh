#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

export LAB_HOME="$TMP/lab-home"
mkdir -p "$LAB_HOME"

cd "$ROOT"
cargo run --all-features -- --json gateway code exec --code 'async () => {
  await state.mkdir({ path: "src" });
  await state.writeJson({ path: "src/config.json", value: { enabled: true }, pretty: true });
  await state.appendFile({ path: "src/app.rs", content: "fn main() {}\n" });
  const hash = await state.hashFile({ path: "src/config.json", algorithm: "sha256" });
  const detect = await state.detectFile({ path: "src/config.json" });
  await state.archiveCreate({ source: "src", destination: "out/src.tar" });
  const archive = await state.archiveList({ path: "out/src.tar", limit: 10 });
  await git.init({});
  await git.add({ path: "src/app.rs" });
  await git.commit({ message: "v2 smoke", authorName: "Lab", authorEmail: "lab@example.invalid" });
  await git.branch({ name: "feature/v2-smoke" });
  await git.checkout({ ref: "feature/v2-smoke" });
  const status = await git.status({});
  return { hash: hash.hex.length, json: detect.json, archive: archive.entries.length, status: status.stdout };
}'
