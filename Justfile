# Lab — Development Commands

default:
    @just --list

# Check all crates compile
check:
    cargo check --workspace --all-features

# Run all tests
test:
    cargo nextest run --workspace --all-features

# Regenerate code-owned documentation inventories
docs-generate:
    cargo run --package labby --all-features -- docs generate

# Verify generated documentation inventories are fresh
docs-check:
    cargo run --package labby --all-features -- docs check

# Run integration tests (requires running services)
test-integration:
    cargo nextest run --workspace --all-features -- --ignored

# Lint
lint:
    cargo clippy --workspace --all-features -- -D warnings
    cargo fmt --all -- --check

# License and vulnerability audit
deny:
    cargo deny check

# Build debug binary with all features
build:
    cargo build --workspace --all-features

# Build release binary with all features
build-release:
    cargo build --workspace --all-features --release
    install -D -m 755 target/release/labby bin/labby

# Generate Claude Code marketplace tree from compiled service metadata
marketplace: build-release
    target/release/labby marketplace generate --out target/marketplace --binary target/release/labby

# Install release binary to ~/.local/bin/labby (updates the host CLI)
install: build-release
    install -D -m 755 bin/labby ~/.local/bin/labby

# Start the dev container for the first time (or after docker-compose changes)
dev-up:
    docker compose -f docker-compose.yml -f docker-compose.dev.yml up -d

# Release build → hot-swap binary into running dev container (no image rebuild)
dev: build-release
    docker compose -f docker-compose.yml -f docker-compose.dev.yml restart

# Debug build with Cranelift codegen (fastest compile) → hot-swap into running dev container.
# Uses nightly toolchain — RUSTFLAGS explicitly includes mold since env var overrides config.toml.
dev-debug:
    RUSTFLAGS="-C link-arg=-fuse-ld=mold -Z codegen-backend=cranelift" \
        cargo +nightly build -p labby --all-features
    install -D -m 755 target/debug/labby bin/labby
    docker compose -f docker-compose.yml -f docker-compose.dev.yml restart

# Verify Docker ACP provider config, provider health, and a minimal Codex ACP prompt.
acp-smoke *ARGS:
    scripts/acp-smoke-check {{ARGS}}

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

# Smoke-test the lab-bg3e.3 setup wizard end-to-end against a throw-away
# LAB_HOME. Used by CI to verify first-run detection + draft commit cycle
# without touching the operator's real ~/.lab/.
smoke-setup:
    rm -rf /tmp/lab-smoke-home
    LAB_HOME=/tmp/lab-smoke-home cargo run --all-features -- setup --no-browser --smoke
