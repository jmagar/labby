#!/bin/sh
# Install labby — the Lab homelab control plane binary.
#
#   curl -fsSL https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.sh | sh
#
# Downloads the latest GitHub release archive for this platform, verifies its
# SHA-256, and installs the binary to ~/.local/bin/labby. When explicitly
# enabled with LAB_ALLOW_SOURCE_FALLBACK=1, a release failure falls back to
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
#   LAB_REQUIRE_CHECKSUM fail if the .sha256 asset is absent (default: 1)
#   LAB_ALLOW_SOURCE_FALLBACK allow cargo fallback after release failure (default: 0)

set -eu

REPO="${LAB_INSTALL_REPO:-jmagar/lab}"
INSTALL_DIR="${LAB_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${LAB_INSTALL_VERSION:-latest}"
REQUIRE_CHECKSUM="${LAB_REQUIRE_CHECKSUM:-1}"
ALLOW_SOURCE_FALLBACK="${LAB_ALLOW_SOURCE_FALLBACK:-0}"
TMP_DIRS=""

cleanup() {
    for dir in $TMP_DIRS; do
        rm -rf "$dir"
    done
}
trap cleanup EXIT

make_tmp_dir() {
    dir="$(mktemp -d)"
    TMP_DIRS="${TMP_DIRS} ${dir}"
    printf '%s' "$dir"
}

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
        fail "no sha256sum/shasum found and checksum verification is required"
    fi
}

install_binary_atomic() {
    # $1 = source binary, installs atomically as "$INSTALL_DIR/labby".
    mkdir -p "$INSTALL_DIR"
    tmp_bin="$(mktemp "$INSTALL_DIR/.labby.XXXXXX")"
    if ! install -m 755 "$1" "$tmp_bin"; then
        rm -f "$tmp_bin"
        return 1
    fi
    mv -f "$tmp_bin" "$INSTALL_DIR/labby"
}

install_from_release() {
    # aarch64/arm64 ships no prebuilt release archive (rquickjs-sys does not
    # cross-compile; no aarch64 fleet host). Skip the release-download path
    # entirely so we don't attempt a URL that 404s, and go straight to the
    # build-from-source fallback below.
    arch="$(uname -m)"
    case "$arch" in
        aarch64 | arm64)
            if [ "$ALLOW_SOURCE_FALLBACK" = "1" ]; then
                say "no prebuilt release archive for $arch — using the build-from-source fallback"
            else
                say "no prebuilt release archive for $arch"
            fi
            return 1
            ;;
    esac

    triple="$(target_triple)" || return 1
    asset="lab-${triple}.tar.gz"
    if [ "$VERSION" = "latest" ]; then
        base="https://github.com/${REPO}/releases/latest/download"
    else
        base="https://github.com/${REPO}/releases/download/${VERSION}"
    fi

    tmp="$(make_tmp_dir)"

    say "downloading ${base}/${asset} ..."
    curl -fsSL --retry 3 -o "$tmp/$asset" "${base}/${asset}" || return 1
    if curl -fsSL --retry 3 -o "$tmp/$asset.sha256" "${base}/${asset}.sha256"; then
        sha256_check "$tmp/$asset" "$tmp/$asset.sha256" \
            || fail "checksum verification FAILED for $asset — aborting"
        say "sha256 verified"
    else
        [ "$REQUIRE_CHECKSUM" = "1" ] && fail "no .sha256 asset published for $asset and LAB_REQUIRE_CHECKSUM=1"
        say "warning: no .sha256 asset published — skipping checksum verification"
    fi

    tar -xzf "$tmp/$asset" -C "$tmp"
    bin="$(find "$tmp" -type f -name labby | head -n 1)"
    [ -n "$bin" ] || fail "archive $asset did not contain a 'labby' binary"

    install_binary_atomic "$bin"
    return 0
}

install_from_source() {
    command -v cargo >/dev/null 2>&1 || return 1
    say "no release asset available — building from source (this takes a while) ..."
    cargo_root="$(make_tmp_dir)"
    if [ "$VERSION" = "latest" ]; then
        cargo install --git "https://github.com/${REPO}" --bin labby --all-features --root "$cargo_root"
    else
        cargo install --git "https://github.com/${REPO}" --tag "$VERSION" --bin labby --all-features --root "$cargo_root"
    fi
    install_binary_atomic "$cargo_root/bin/labby"
}

main() {
    if install_from_release; then
        :
    elif [ "$ALLOW_SOURCE_FALLBACK" != "1" ]; then
        fail "could not install: release install failed and LAB_ALLOW_SOURCE_FALLBACK=$ALLOW_SOURCE_FALLBACK disables source fallback.
Choose a supported prebuilt release or re-run with LAB_ALLOW_SOURCE_FALLBACK=1 to build from source."
    elif install_from_source; then
        :
    else
        fail "could not install: no prebuilt release for $(uname -s)/$(uname -m) and no cargo toolchain found.
Install a Rust toolchain (https://rustup.rs) and re-run, or build from a clone:
  git clone https://github.com/${REPO} && cd lab && cargo install --path crates/labby --bin labby --all-features"
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
