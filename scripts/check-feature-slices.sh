#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run cargo check -p labby-apis --no-default-features
run cargo check -p labby-apis --no-default-features --features all

labby_product_features=(
  ""
  "gateway"
  "marketplace"
  "fs"
  "deploy"
  "acp_registry"
  "gateway,marketplace"
  "all"
)

for features in "${labby_product_features[@]}"; do
  if [[ -z "$features" ]]; then
    run cargo check -p labby --no-default-features --all-targets
  else
    run cargo check -p labby --no-default-features --features "$features" --all-targets
  fi
done

run cargo check -p labby --no-default-features --features mcpregistry --all-targets
run cargo check -p labby --all-features --all-targets
