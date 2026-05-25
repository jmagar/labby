#!/usr/bin/env bash
set -euo pipefail

version="${JAVY_VERSION:-v7.0.0}"
dest="${1:-crates/lab/src/dispatch/gateway/code_mode_wasm/plugin.wasm}"
base_url="https://github.com/bytecodealliance/javy/releases/download/${version}"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

mkdir -p "$(dirname "$dest")"
curl -fsSL "${base_url}/plugin.wasm.gz" -o "$tmpdir/plugin.wasm.gz"
curl -fsSL "${base_url}/plugin.wasm.gz.sha256" -o "$tmpdir/plugin.wasm.gz.sha256"

(
  cd "$tmpdir"
  sha256sum -c plugin.wasm.gz.sha256
  gzip -dc plugin.wasm.gz > plugin.wasm
)

cp "$tmpdir/plugin.wasm" "$dest"
sha256sum "$dest"
