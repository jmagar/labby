#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run cargo check -p labby-apis --no-default-features
run cargo check -p labby-apis --no-default-features --features all

run cargo check -p labby-auth --no-default-features --all-targets
run cargo check -p labby-auth --no-default-features --features http-axum --all-targets
run cargo check -p labby-auth --no-default-features --features upstream-oauth-rmcp --all-targets
run cargo check -p labby-auth --no-default-features --features http-axum,upstream-oauth-rmcp --all-targets

labby_runtime_features=(
  ""
  "marketplace"
  "acp_registry"
  "deploy"
  "marketplace,acp_registry,deploy"
)

for features in "${labby_runtime_features[@]}"; do
  if [[ -z "$features" ]]; then
    run cargo check -p labby-runtime --no-default-features --all-targets
  else
    run cargo check -p labby-runtime --no-default-features --features "$features" --all-targets
  fi
done

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
