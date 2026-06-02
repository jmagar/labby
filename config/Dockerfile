# ─── Stage 1: Build ───────────────────────────────────────────────────────────
# Requires Rust 1.92+ (Cargo.toml: rust-version = "1.92", edition = "2024")
FROM rust:1-slim AS builder

WORKDIR /build

# GitHub's container builder has less memory than the release-smoke runners.
# Keep the container release binary optimized but avoid ThinLTO/codegen-units=1
# peak memory during the final link.
ENV CARGO_PROFILE_RELEASE_LTO=false \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16

# System build deps:
#   pkg-config     — lets -sys crates find system headers
#   build-essential — C compiler for rusqlite's bundled SQLite + other -sys crates
#   libssl-dev     — headers/pkg-config metadata for crates pulling openssl-sys
#   libclang-dev   — libclang for rquickjs-sys bindgen during Code Mode builds
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config \
      build-essential \
      libssl-dev \
      libclang-dev \
 && rm -rf /var/lib/apt/lists/*

# ── Dependency-caching layer ──────────────────────────────────────────────────
# Copy only manifests so the heavy dep-compile layer is invalidated only when
# Cargo.toml / Cargo.lock changes, not on every src edit.
COPY Cargo.toml Cargo.lock ./
COPY crates/lab/Cargo.toml      crates/lab/Cargo.toml
COPY crates/lab-apis/Cargo.toml crates/lab-apis/Cargo.toml
COPY crates/lab-auth/Cargo.toml crates/lab-auth/Cargo.toml
COPY crates/vendor/agent-client-protocol/Cargo.toml crates/vendor/agent-client-protocol/Cargo.toml

# Create stub source + test files so cargo can parse ALL manifests without error.
#
# lab-apis/Cargo.toml declares explicit [[test]] targets with required-features:
#   plex_client, gotify_client, openacp_client, jellyfin_client → need matching
#   files under crates/lab-apis/tests/
# Cargo validates file existence at manifest-parse time even when the required
# feature is disabled, so we must stub them.
#
# lab/ has both [[bin]] (main.rs) and [lib] (lib.rs) targets — stub both.
#
# The workspace patches agent-client-protocol to a vendored local crate, so the
# dependency-cache build also needs a stub for that patched crate.
#
# We also need apps/gateway-admin/out/ to exist for include_dir!() in web.rs,
# but we copy the real one below — for the dep-cache step a stub dir suffices.
RUN mkdir -p \
      crates/lab/src \
      crates/lab-apis/src \
      crates/lab-apis/tests \
      crates/lab-auth/src \
      crates/vendor/agent-client-protocol/src \
      crates/vendor/agent-client-protocol/examples \
      crates/vendor/agent-client-protocol/tests \
      apps/gateway-admin/out \
 && echo 'fn main(){}' > crates/lab/src/main.rs \
 && touch \
      crates/lab/src/lib.rs \
      crates/lab-apis/src/lib.rs \
      crates/lab-apis/tests/plex_client.rs \
      crates/lab-apis/tests/gotify_client.rs \
      crates/lab-apis/tests/openacp_client.rs \
      crates/lab-apis/tests/jellyfin_client.rs \
      crates/lab-auth/src/lib.rs \
      crates/vendor/agent-client-protocol/src/lib.rs \
      crates/vendor/agent-client-protocol/examples/simple_agent.rs \
      crates/vendor/agent-client-protocol/examples/yolo_one_shot_client.rs \
      crates/vendor/agent-client-protocol/tests/derive_macros.rs \
      crates/vendor/agent-client-protocol/tests/jsonrpc_advanced.rs \
      crates/vendor/agent-client-protocol/tests/jsonrpc_connection_builder.rs \
      crates/vendor/agent-client-protocol/tests/jsonrpc_edge_cases.rs \
      crates/vendor/agent-client-protocol/tests/jsonrpc_error_handling.rs \
      crates/vendor/agent-client-protocol/tests/jsonrpc_hello.rs \
      crates/vendor/agent-client-protocol/tests/match_dispatch.rs \
 && cargo build --workspace --all-features --release \
 && cargo clean -p labby \
 && cargo clean -p lab-apis \
 && cargo clean -p lab-auth \
 && rm -rf \
      crates/*/src \
      crates/vendor/agent-client-protocol/src \
      crates/vendor/agent-client-protocol/examples \
      crates/vendor/agent-client-protocol/tests \
      crates/lab-apis/tests \
      apps/gateway-admin/out \
      target/release/labby

# ── Real source + frontend assets ─────────────────────────────────────────────
# crates/lab/build.rs embeds the Next.js static export (apps/gateway-admin/out)
# into the binary via include_bytes!. The out/ dir must exist before `cargo build`.
# (.dockerignore excludes apps/gateway-admin/{node_modules,.next,...} — only
# apps/gateway-admin/out is sent in the build context.)
COPY apps/gateway-admin/out apps/gateway-admin/out
COPY crates/ crates/
# BuildKit normalizes COPYed file mtimes to a fixed past epoch. That timestamp is
# older than the stub rlibs built in the dep-cache layer above, so cargo's
# fingerprint considers the real workspace sources "unchanged" and skips
# rebuilding them — linking the empty stub crates (esp. the patched
# agent-client-protocol, which is not cargo-cleaned) and producing hundreds of
# "cannot find X in lab_apis/lab_auth/agent_client_protocol" errors. Touch the
# real sources so their mtimes are newer, forcing a from-source rebuild of every
# workspace crate while keeping the cached third-party dependency artifacts.
RUN find crates -type f -exec touch {} + \
 && cargo build --workspace --all-features --release

# ─── Stage 2: Runtime ─────────────────────────────────────────────────────────
FROM debian:trixie-slim AS runtime

# ca-certificates: reqwest uses rustls for TLS but still validates against the
# system CA bundle — required for outbound HTTPS calls.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl gnupg \
 && mkdir -p /etc/apt/keyrings \
 && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
    | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
 && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_24.x nodistro main" \
    > /etc/apt/sources.list.d/nodesource.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends nodejs zsh \
 && rm -rf /var/lib/apt/lists/*

# Agent CLIs used by ACP adapters and by stdio MCP servers that shell out to
# provider tools. Keep these deterministic so container behavior matches the
# host credential/runtime format expected by local development.
RUN mkdir -p /opt/agent-clis \
 && cd /opt/agent-clis \
 && npm init -y >/dev/null \
 && npm install --omit=dev --no-audit --no-fund \
      @anthropic-ai/claude-code@2.1.132 \
      @openai/codex@0.130.0 \
 && ln -sf /opt/agent-clis/node_modules/.bin/claude /usr/local/bin/claude \
 && ln -sf /opt/agent-clis/node_modules/.bin/codex /usr/local/bin/codex \
 && npm cache clean --force \
 && claude --version \
 && codex --version

# Python MCP servers commonly use `uvx` as their stdio launcher. Install uv
# into PATH for gateway child processes.
RUN curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh \
 && uv --version \
 && uvx --version

# Pre-create XDG directories so the binary never needs mkdir at runtime.
#   ~/.local/state/lab  — master.lock and runtime state
#   ~/.local/share/lab  — SQLite databases (node log, registry, auth)
#   ~/.config/lab       — config.toml (mounted read-only from host)
RUN mkdir -p \
      /home/lab/.config/lab \
      /home/lab/.local/state/lab \
      /home/lab/.local/share/lab \
 && chown -R 1000:1000 /home/lab

COPY --from=builder /build/target/release/labby /usr/local/bin/labby

ENV HOME=/home/lab
USER 1000:1000
WORKDIR /home/lab

# Default HTTP port.  Override with LAB_MCP_HTTP_PORT env var or config.toml.
EXPOSE 8765

ENTRYPOINT ["/usr/local/bin/labby"]
# Master HTTP mode by default.
# Override CMD to ["serve","mcp","--stdio"] for the stdio-only MCP node.
CMD ["serve"]
