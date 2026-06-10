#!/bin/sh
# Install labby — the Lab homelab control plane binary.
#
#   curl -fsSL https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.sh | sh
#
# Downloads the latest GitHub release archive for this platform, verifies its
# SHA-256, and installs the binary to ~/.local/bin/labby. When no release
# asset exists (or the platform has no prebuilt archive) it falls back to
# `cargo install --git` if a Rust toolchain is available.
#
# This script's ONLY job is bootstrap: getting `labby` onto PATH. Everything
# after that is owned by the binary — run `labby setup` for the first-run
# flow (config, credentials, connectivity checks).
#
# Environment overrides:
#   LAB_INSTALL_DIR     install directory       (default: ~/.local/bin)
#   LAB_INSTALL_REPO    owner/repo to fetch     (default: jmagar/lab)
#   LAB_INSTALL_VERSION release tag, e.g. v0.22.2 (default: latest)

set -eu

REPO="${LAB_INSTALL_REPO:-jmagar/lab}"
INSTALL_DIR="${LAB_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${LAB_INSTALL_VERSION:-latest}"

say() { printf '%s\n' "$*" >&2; }
fail() { say "install.sh: $*"; exit 1; }

target_triple() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Linux)
            case "$arch" in
                x86_64) echo "x86_64-unknown-linux-gnu" ;;
                # aarch64 has no prebuilt archive (rquickjs does not
                # cross-compile); ARM falls through to the cargo fallback.
                *) return 1 ;;
            esac
            ;;
        *) return 1 ;;
    esac
}

sha256_check() {
    # $1 = file, $2 = expected-checksum file (file is "<hex>  <name>" format)
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$(dirname "$1")" && sha256sum -c "$2" >/dev/null 2>&1) && return 0
        # Some checksum files carry only the bare hex digest.
        expected="$(awk '{print $1}' "$2")"
        actual="$(sha256sum "$1" | awk '{print $1}')"
        [ "$expected" = "$actual" ]
    elif command -v shasum >/dev/null 2>&1; then
        expected="$(awk '{print $1}' "$2")"
        actual="$(shasum -a 256 "$1" | awk '{print $1}')"
        [ "$expected" = "$actual" ]
    else
        say "warning: no sha256sum/shasum found — skipping checksum verification"
        return 0
    fi
}

install_from_release() {
    triple="$(target_triple)" || return 1
    asset="lab-${triple}.tar.gz"
    if [ "$VERSION" = "latest" ]; then
        base="https://github.com/${REPO}/releases/latest/download"
    else
        base="https://github.com/${REPO}/releases/download/${VERSION}"
    fi

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT

    say "downloading ${base}/${asset} ..."
    curl -fsSL --retry 3 -o "$tmp/$asset" "${base}/${asset}" || return 1
    if curl -fsSL --retry 3 -o "$tmp/$asset.sha256" "${base}/${asset}.sha256"; then
        sha256_check "$tmp/$asset" "$tmp/$asset.sha256" \
            || fail "checksum verification FAILED for $asset — aborting"
        say "sha256 verified"
    else
        say "warning: no .sha256 asset published — skipping checksum verification"
    fi

    tar -xzf "$tmp/$asset" -C "$tmp"
    bin="$(find "$tmp" -type f -name labby | head -n 1)"
    [ -n "$bin" ] || fail "archive $asset did not contain a 'labby' binary"

    mkdir -p "$INSTALL_DIR"
    install -m 755 "$bin" "$INSTALL_DIR/labby"
    return 0
}

install_from_source() {
    command -v cargo >/dev/null 2>&1 || return 1
    say "no release asset available — building from source (this takes a while) ..."
    cargo install --git "https://github.com/${REPO}" --bin labby --all-features --root "${INSTALL_DIR%/bin}" \
        || cargo install --git "https://github.com/${REPO}" --bin labby --all-features
}

main() {
    if install_from_release; then
        :
    elif install_from_source; then
        :
    else
        fail "could not install: no prebuilt release for $(uname -s)/$(uname -m) and no cargo toolchain found.
Install a Rust toolchain (https://rustup.rs) and re-run, or build from a clone:
  git clone https://github.com/${REPO} && cd lab && cargo install --path crates/lab --bin labby --all-features"
    fi

    if ! command -v labby >/dev/null 2>&1; then
        say ""
        say "NOTE: $INSTALL_DIR is not on your PATH. Add it, e.g.:"
        say "  export PATH=\"$INSTALL_DIR:\$PATH\""
    fi

    say ""
    say "labby installed: $("$INSTALL_DIR/labby" --version 2>/dev/null || echo "$INSTALL_DIR/labby")"
    say "next: run 'labby setup' to start the first-run flow"
}

main "$@"
