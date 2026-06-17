# Lab — Development Commands

local_release_profile := "release-fast"

default:
    @just --list

# Check all crates compile
check:
    cargo check --workspace --all-features

# Run all tests
test:
    cargo nextest run --workspace --all-features

# Verify Cargo wrapper binary sync behavior
test-cargo-wrapper:
    scripts/test-cargo-rustc-wrapper.sh

# Regenerate code-owned documentation inventories
docs-generate:
    cargo run --package labby --all-features -- docs generate

# Verify generated documentation inventories are fresh
docs-check:
    cargo run --package labby --all-features -- docs check

# Run integration tests (requires running services)
test-integration:
    cargo nextest run --workspace --all-features --run-ignored ignored-only

# Lint
lint: skill-drift test-cargo-wrapper
    cargo clippy --workspace --all-features -- -D warnings
    cargo fmt --all -- --check

# Check hand-authored skills for known stale or unsafe patterns
skill-drift:
    scripts/check-dozzle-skill

# License and vulnerability audit
deny:
    cargo deny check

# Build debug binary with all features
build:
    cargo build --workspace --all-features

# Build release binary with all features.
# bin/labby is the container bind-mount (docker-compose.yml); the plugin does
# NOT ship a binary — hosts install labby via scripts/install.sh or cargo.
build-release:
    cargo build --workspace --all-features --release
    install -D -m 755 target/release/labby bin/labby
    just link-bin

# Symlink the compiled binary into PATH.
# Called automatically by `just build-release` and `just install`.
link-bin profile="release":
    #!/usr/bin/env bash
    set -euo pipefail
    profile="{{profile}}"
    LAB_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
    case "$LAB_TARGET_DIR" in
      /*) LABBY_BIN="$LAB_TARGET_DIR/$profile/labby" ;;
      *)  LABBY_BIN="$(pwd)/$LAB_TARGET_DIR/$profile/labby" ;;
    esac
    if [ ! -x "$LABBY_BIN" ]; then
      echo "$profile binary not found at $LABBY_BIN — run the matching build first" >&2
      exit 1
    fi
    mkdir -p ~/.local/bin
    ln -sf "$LABBY_BIN" ~/.local/bin/labby
    echo "labby → $LABBY_BIN"

# Build local release-fast binary when stale, sync PATH/container bind binary,
# rebuild the dev image only when runtime inputs changed, and restart container.
sync-container:
    #!/usr/bin/env bash
    set -euo pipefail
    repo="$(pwd)"
    profile="{{local_release_profile}}"
    if command -v mold >/dev/null 2>&1; then
      export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-fuse-ld=mold"
    fi

    LAB_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
    case "$LAB_TARGET_DIR" in
      /*) LABBY_BIN="$LAB_TARGET_DIR/$profile/labby" ;;
      *)  LABBY_BIN="$repo/$LAB_TARGET_DIR/$profile/labby" ;;
    esac

    release_stale=0
    if [ ! -x "$LABBY_BIN" ]; then
      release_stale=1
    else
      while IFS= read -r -d '' input; do
        if [ "$input" -nt "$LABBY_BIN" ]; then
          release_stale=1
          break
        fi
      done < <(git ls-files -z -- Cargo.toml Cargo.lock rust-toolchain.toml .cargo build.rs crates config apps/gateway-admin/out)
    fi
    if [ "$release_stale" -eq 1 ]; then
      CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-16}" cargo build --workspace --all-features --profile "$profile" --bin labby
    else
      echo "$profile binary is current: $LABBY_BIN"
    fi

    install -D -m 755 "$LABBY_BIN" bin/labby
    mkdir -p ~/.local/bin
    ln -sf "$LABBY_BIN" ~/.local/bin/labby
    echo "labby → $LABBY_BIN"

    compose=(docker compose -f docker-compose.yml)
    container_sentinel="$LAB_TARGET_DIR/.labby-container-built"
    image_stale=0
    if ! docker image inspect labby:dev >/dev/null 2>&1; then
      image_stale=1
    else
      while IFS= read -r -d '' input; do
        if [ "$input" -nt "$container_sentinel" ] 2>/dev/null; then
          image_stale=1
          break
        fi
      done < <(git ls-files -z -- config/Dockerfile.fast docker-compose.yml docker-compose.prod.yml config/acp-adapters.package.json)
    fi
    if [ "$image_stale" -eq 1 ]; then
      "${compose[@]}" build labby-master
      mkdir -p "$(dirname "$container_sentinel")"
      touch "$container_sentinel"
      "${compose[@]}" up -d labby-master --no-deps --no-build
    else
      echo "dev runtime image is current"
      "${compose[@]}" up -d labby-master --no-deps --no-build
    fi
    "${compose[@]}" restart labby-master
    "${compose[@]}" ps labby-master
    echo "container synced"

container-sync: sync-container

# Generate Claude Code marketplace tree from compiled service metadata
marketplace: build-release
    target/release/labby marketplace generate --out target/marketplace

# Install release binary to ~/.local/bin/labby (updates the host CLI)
install: build-release
    just link-bin

# Ensure host-side runtime directories are owned by the current user before
# Docker can claim them as root during bind-mount creation.
ensure-host-dirs:
    scripts/ensure-host-dirs

# Start the dev container for the first time (or after docker-compose changes)
dev-up: ensure-host-dirs
    docker compose -f docker-compose.yml up -d

# Release build + web assets → hot-swap into running dev container (no image rebuild)
dev: web-build build-release
    docker compose -f docker-compose.yml restart

# Debug build with Cranelift codegen (fastest compile) → hot-swap into running dev container.
# Uses nightly toolchain — RUSTFLAGS explicitly includes mold since env var overrides config.toml.
dev-debug:
    #!/usr/bin/env bash
    set -euo pipefail
    nightly_rustc=$(rustup which --toolchain nightly rustc)
    RUSTC="$nightly_rustc" RUSTC_WRAPPER="" RUSTFLAGS="-C link-arg=-fuse-ld=mold -Z codegen-backend=cranelift" \
        cargo build -p labby --all-features
    install -D -m 755 target/debug/labby bin/labby
    docker compose -f docker-compose.yml restart

# Verify Docker ACP provider config, provider health, and a minimal Codex ACP prompt.
acp-smoke *ARGS:
    scripts/acp-smoke-check {{ARGS}}

# Verify a public OAuth-protected MCP route through the deployed reverse proxy.
protected-mcp-smoke *ARGS:
    scripts/protected-mcp-smoke {{ARGS}}

# Rebuild static Labby web assets served by labby serve
web-build:
    cd apps/gateway-admin && pnpm build

# Rebuild static Labby web assets when frontend files change
web-watch:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v watchexec >/dev/null 2>&1; then
        echo "error: watchexec is required for web-watch" >&2
        echo "install: cargo install watchexec-cli" >&2
        exit 1
    fi
    echo "Building apps/gateway-admin once, then watching for changes..."
    watchexec \
      --project-origin . \
      --watch apps/gateway-admin \
      --ignore 'apps/gateway-admin/.next' \
      --ignore 'apps/gateway-admin/.next/**' \
      --ignore 'apps/gateway-admin/out' \
      --ignore 'apps/gateway-admin/out/**' \
      --ignore 'apps/gateway-admin/node_modules' \
      --ignore 'apps/gateway-admin/node_modules/**' \
      --debounce 1000ms \
      --on-busy-update queue \
      --wrap-process=none \
      'cd apps/gateway-admin && pnpm build'

# Run with args
run *ARGS:
    cargo run --all-features -- {{ARGS}}

# Run the binary-served static chat UI in local ACP mode
chat-local:
    #!/usr/bin/env bash
    set -euo pipefail
    export LAB_WEB_UI_AUTH_DISABLED=true
    export LAB_MCP_HTTP_TOKEN="${LAB_MCP_HTTP_TOKEN:-dev-token}"
    export LAB_CORS_ORIGINS="${LAB_CORS_ORIGINS:-http://dookie:3000,http://127.0.0.1:3000,http://localhost:3000}"
    export LAB_CHAT_LOCAL_PORT="${LAB_CHAT_LOCAL_PORT:-8766}"
    cargo run --all-features --bin labby -- serve --host 0.0.0.0 --port "${LAB_CHAT_LOCAL_PORT}"

# Format all code
fmt:
    cargo fmt --all

# Clean build artifacts
clean:
    cargo clean

# Release (version bump + tag + push)
release *ARGS:
    cargo release {{ARGS}}

# Generate a secure MCP HTTP bearer token and write it to .env
mcp-token:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -f .env ]; then
        echo "error: .env not found — copy .env.example first" >&2
        exit 1
    fi
    token=$(openssl rand -hex 32)
    if grep -q '^LAB_MCP_HTTP_TOKEN=' .env; then
        # macOS/BSD sed compat: write to tmp then move
        tmp=$(mktemp)
        awk -v t="$token" '/^LAB_MCP_HTTP_TOKEN=/{print "LAB_MCP_HTTP_TOKEN=" t; next} {print}' .env > "$tmp"
        mv "$tmp" .env
        echo "✓ rotated LAB_MCP_HTTP_TOKEN in .env"
    else
        echo "LAB_MCP_HTTP_TOKEN=$token" >> .env
        echo "✓ appended LAB_MCP_HTTP_TOKEN to .env"
    fi
    echo "  $token"

# Run the prod image locally with prod-like env (LAB_UPSTREAM_DISCOVERY_CONCURRENCY=3, no
# bind-mounted binary). Useful for testing spawn-storm safeguards and discovery timeouts that
# are masked by the dev stack's higher concurrency default (16). Starts detached, polls /health
# for up to 60s, then prints the container ID. Stop with: docker stop lab-prod-test
# See docs/OPERATIONS.md §Dev/Prod Container Drift for the full drift inventory.
prod-run: build-release
    #!/usr/bin/env bash
    set -euo pipefail
    docker stop lab-prod-test 2>/dev/null || true
    docker rm   lab-prod-test 2>/dev/null || true
    docker build -f config/Dockerfile.fast -t labby:prod-test .
    docker run -d --name lab-prod-test \
        -p 18765:8765 \
        -v "${HOME}/.lab:/home/lab/.lab" \
        -e LAB_MCP_HTTP_HOST=0.0.0.0 \
        -e LAB_MCP_HTTP_PORT=8765 \
        -e LAB_UPSTREAM_DISCOVERY_CONCURRENCY=3 \
        labby:prod-test
    echo "container started — polling http://localhost:18765/health (60s timeout)..."
    deadline=$(( $(date +%s) + 60 ))
    until curl -sf http://localhost:18765/health >/dev/null 2>&1; do
        if [ "$(date +%s)" -ge "$deadline" ]; then
            echo "TIMEOUT: /health did not return 200 within 60s" >&2
            docker logs lab-prod-test >&2
            docker stop lab-prod-test
            exit 1
        fi
        sleep 2
    done
    echo "healthy — container: lab-prod-test (host port 18765)"
    echo "stop with: docker stop lab-prod-test"

# Smoke-test the lab-bg3e.3 setup wizard end-to-end against a throw-away
# LAB_HOME. Used by CI to verify first-run detection + draft commit cycle
# without touching the operator's real ~/.lab/.
smoke-setup:
    rm -rf /tmp/lab-smoke-home
    LAB_HOME=/tmp/lab-smoke-home cargo run --all-features -- setup --no-browser --smoke

# Diagnose the sccache build cache. Reports daemon health (systemd-owned),
# binary-vs-daemon version skew, cache stats, distributed-build config, and the
# tail of the error log. Run this FIRST when builds behave oddly (stale/wrong
# artifacts) before reaching for a wipe. See docs/RUST.md §sccache troubleshooting.
sccache-doctor:
    #!/usr/bin/env bash
    set -uo pipefail
    # MUST match the systemd unit's socket, or sccache spawns an unmanaged
    # ephemeral server and reports misleading zero stats.
    export SCCACHE_SERVER_UDS=/tmp/sccache-jmagar.sock
    echo "── daemon (systemd --user) ─────────────────────────"
    systemctl --user is-active sccache.service 2>/dev/null && \
      systemctl --user show sccache.service -p MainPID -p ActiveEnterTimestamp -p NRestarts 2>/dev/null
    echo "── version skew (binary vs running daemon) ─────────"
    bin_ver=$(/home/jmagar/.local/sccache --version 2>/dev/null)
    echo "binary:  $bin_ver"
    echo "  (daemon is long-lived; if the mise-pinned binary changed, restart with 'just sccache-restart')"
    echo "── distributed build config ────────────────────────"
    if grep -qs '^\[dist\]' ~/.config/sccache/config; then
      echo "DIST ENABLED — scheduler: $(grep -soE 'https?://[^\"]+' ~/.config/sccache/config | head -1)"
      echo "  ⚠ remote builds can cache cross-machine artifacts; mismatched toolchains poison the cache."
    else
      echo "dist disabled (local-only) ✓"
    fi
    echo "── cache stats ─────────────────────────────────────"
    /home/jmagar/.local/sccache --show-stats 2>/dev/null | grep -iE "compile requests|cache hits|cache misses|errors|cache location|cache size" || true
    echo "── error log tail (real errors only) ───────────────"
    log=/home/jmagar/.local/state/sccache/error.log
    if [ -f "$log" ]; then
      sz=$(du -h "$log" | cut -f1); echo "log: $log ($sz)"
      grep -aE "ERROR|WARN|panic|corrupt|CacheReadError|CacheWriteError" "$log" 2>/dev/null | grep -avE "DEBUG|CannotCache" | tail -15 || echo "(no error/warn lines)"
    else
      echo "(no error log)"
    fi

# Cleanly restart the sccache daemon via systemd (NEVER use bare
# 'sccache --start-server' — Restart=always means systemd owns it and a manual
# start races the unit). Use after the mise-pinned sccache binary changes, or as
# the FIRST recovery step for suspected cache poisoning (fixes daemon-state
# causes without wiping on-disk artifacts). Only wipe the cache if this fails.
sccache-restart:
    #!/usr/bin/env bash
    set -euo pipefail
    export SCCACHE_SERVER_UDS=/tmp/sccache-jmagar.sock
    echo "restarting sccache.service (systemd --user)…"
    systemctl --user restart sccache.service
    sleep 1
    systemctl --user is-active sccache.service
    /home/jmagar/.local/sccache --show-stats 2>/dev/null | grep -iE "compile requests|cache location" || true
    echo "✓ restarted. If poisoning persists, wipe on-disk cache:"
    echo "    systemctl --user stop sccache.service && rm -rf ~/.cache/sccache && systemctl --user start sccache.service"
