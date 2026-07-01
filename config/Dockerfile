# ─── Stage 1: Build ───────────────────────────────────────────────────────────
# Requires Rust 1.92+ (Cargo.toml: rust-version = "1.92", edition = "2024")
# TODO: pin by digest (FROM rust:1-slim@sha256:...) for reproducible builds —
#       floating tag resolves a fresh image per build.
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
COPY crates/labby/Cargo.toml      crates/labby/Cargo.toml
COPY crates/labby-apis/Cargo.toml crates/labby-apis/Cargo.toml
COPY crates/labby-auth/Cargo.toml crates/labby-auth/Cargo.toml
COPY crates/labby-runtime/Cargo.toml crates/labby-runtime/Cargo.toml
COPY crates/labby-codemode/Cargo.toml crates/labby-codemode/Cargo.toml
COPY crates/labby-gateway/Cargo.toml crates/labby-gateway/Cargo.toml
COPY crates/labby-web/Cargo.toml crates/labby-web/Cargo.toml
COPY crates/labby-winjob/Cargo.toml crates/labby-winjob/Cargo.toml

# Create stub source files so cargo can parse ALL manifests without error.
#
# labby has both [[bin]] (main.rs) and [lib] (lib.rs) targets — stub both.
#
# We also need apps/gateway-admin/out/ to exist for include_dir!() in web.rs,
# but we copy the real one below — for the dep-cache step a stub dir suffices.
RUN mkdir -p \
      crates/labby/src \
      crates/labby-apis/src \
      crates/labby-auth/src \
      crates/labby-runtime/src \
      crates/labby-codemode/src \
      crates/labby-gateway/src \
      crates/labby-web/src \
      crates/labby-winjob/src \
      apps/gateway-admin/out \
 && echo 'fn main(){}' > crates/labby/src/main.rs \
 && echo 'fn main(){}' > crates/labby-web/build.rs \
 && touch \
      crates/labby/src/lib.rs \
      crates/labby-apis/src/lib.rs \
      crates/labby-auth/src/lib.rs \
      crates/labby-runtime/src/lib.rs \
      crates/labby-codemode/src/lib.rs \
      crates/labby-gateway/src/lib.rs \
      crates/labby-web/src/lib.rs \
      crates/labby-winjob/src/lib.rs \
 && cargo build --workspace --all-features --release \
 && cargo clean -p labby \
 && cargo clean -p labby-apis \
 && cargo clean -p labby-auth \
 && cargo clean -p labby-runtime \
 && cargo clean -p labby-codemode \
 && cargo clean -p labby-gateway \
 && cargo clean -p labby-web \
 && cargo clean -p labby-winjob \
 && rm -rf \
      crates/*/src \
      apps/gateway-admin/out \
      target/release/labby

# ── Real source + frontend assets ─────────────────────────────────────────────
# crates/labby/build.rs embeds the Next.js static export (apps/gateway-admin/out)
# into the binary via include_bytes!. The out/ dir must exist before `cargo build`.
# (.dockerignore excludes apps/gateway-admin/{node_modules,.next,...} — only
# apps/gateway-admin/out is sent in the build context.)
COPY apps/gateway-admin/out apps/gateway-admin/out
COPY docs/generated/env-reference.json docs/generated/env-reference.json
COPY scripts/incus-bootstrap.sh scripts/incus-bootstrap.sh
COPY scripts/install.sh scripts/install.sh
COPY config/incus/labby-backup.yaml config/incus/labby-backup.yaml
COPY config/incus/labby-gateway-profile.yaml config/incus/labby-gateway-profile.yaml
COPY config/incus/labby-image.yaml config/incus/labby-image.yaml
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
# TODO: pin by digest (FROM debian:trixie-slim@sha256:...) for reproducible
#       builds — floating tag resolves a fresh image per build.
FROM debian:trixie-slim AS runtime

# ca-certificates: reqwest uses rustls for TLS but still validates against the
# system CA bundle — required for outbound HTTPS calls.
# git: the marketplace feature shells out to `git` (fetch/show/rev-parse) for
# its update/fork flow — without it that feature is dead in the shipped image.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git gnupg \
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
#
# Reproducibility: the astral.sh installer pins a version when UV_VERSION is set
# (it fetches that exact release instead of "latest"). UV_VERSION is currently
# empty, so this tracks the latest uv at build time.
# TODO: pin uv by setting UV_VERSION to a verified release tag (e.g. "0.x.y")
#       so the runtime image is reproducible across builds.
ARG UV_VERSION=""
RUN curl -LsSf https://astral.sh/uv/install.sh \
    | env UV_INSTALL_DIR=/usr/local/bin UV_VERSION="${UV_VERSION}" sh \
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

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -sf http://localhost:8765/health || exit 1

ENTRYPOINT ["/usr/local/bin/labby"]
# Master HTTP mode by default.
# Override CMD to ["serve","mcp","--stdio"] for the stdio-only MCP node.
CMD ["serve"]
