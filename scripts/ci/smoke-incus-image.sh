#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

export_dir="${EXPORT_DIR:-$repo_root/target/incus-image-dist}"
image_alias="${IMAGE_ALIAS:-labby-incus-smoke}"
container_name="${SMOKE_CONTAINER_NAME:-labby-incus-image-smoke}"
profile_name="${SMOKE_PROFILE_NAME:-labby-gateway-smoke}"
profile_yaml="${SMOKE_PROFILE_YAML:-$repo_root/config/incus/labby-gateway-profile.yaml}"
image_tar="${IMAGE_TAR:-}"

log() {
    printf '[labby-incus] %s\n' "$*"
}

die() {
    printf '[labby-incus] error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

sudo_cmd() {
    if [[ "$(id -u)" -eq 0 ]]; then
        "$@"
    else
        sudo "$@"
    fi
}

INCUS_USE_SUDO=0

incus_cmd() {
    if [[ "$INCUS_USE_SUDO" == "1" ]]; then
        sudo incus "$@"
    else
        incus "$@"
    fi
}

install_incus_if_needed() {
    if have incus; then
        return
    fi

    have apt-get || die "incus is not installed and this script only knows apt-get installation"
    log "installing incus"
    sudo_cmd apt-get update
    sudo_cmd apt-get install -y incus uidmap squashfs-tools
}

ensure_incus_ready() {
    if incus info >/dev/null 2>&1; then
        return
    fi

    if have sudo && sudo incus info >/dev/null 2>&1; then
        INCUS_USE_SUDO=1
        return
    fi

    log "initializing incus with minimal defaults"
    sudo_cmd incus admin init --minimal || true

    if incus info >/dev/null 2>&1; then
        return
    fi
    if have sudo && sudo incus info >/dev/null 2>&1; then
        INCUS_USE_SUDO=1
        return
    fi

    die "incus did not become usable after initialization"
}

default_storage_pool() {
    local pool
    pool="$(incus_cmd profile device get default root pool 2>/dev/null || true)"
    if [[ -n "$pool" ]]; then
        printf '%s\n' "$pool"
        return
    fi
    pool="$(incus_cmd storage list --format csv | awk -F, 'NF {print $1; exit}' || true)"
    [[ -n "$pool" ]] || die "could not determine an Incus storage pool"
    printf '%s\n' "$pool"
}

ensure_smoke_profile() {
    local pool
    local rendered

    [[ -f "$profile_yaml" ]] || die "missing profile YAML: $profile_yaml"
    pool="$(default_storage_pool)"
    rendered="$(mktemp)"
    sed \
        -e "s/^name: .*/name: $profile_name/" \
        -e "s/^    pool: .*/    pool: $pool/" \
        "$profile_yaml" >"$rendered"

    if ! incus_cmd profile show "$profile_name" >/dev/null 2>&1; then
        log "creating Incus smoke profile $profile_name"
        incus_cmd profile create "$profile_name"
    fi
    log "applying smoke profile $profile_name with storage pool $pool"
    incus_cmd profile edit "$profile_name" <"$rendered"
    rm -f "$rendered"
}

wait_for_running() {
    local name="$1"
    local state

    for _ in $(seq 1 90); do
        state="$(incus_cmd info "$name" 2>/dev/null | awk -F': ' '$1 == "Status" {print $2; exit}' || true)"
        if [[ "$state" == "RUNNING" ]]; then
            return
        fi
        sleep 1
    done

    die "$name did not reach RUNNING state"
}

container_file_exists() {
    local name="$1"
    local path="$2"
    local tmp

    tmp="$(mktemp)"
    if incus_cmd file pull "$name$path" "$tmp" >/dev/null 2>&1; then
        rm -f "$tmp"
        return 0
    fi
    rm -f "$tmp"
    return 1
}

if [[ -z "$image_tar" ]]; then
    image_tar="$(find "$export_dir" -maxdepth 1 -type f -name 'labby-incus-*.tar.xz' -print -quit)"
fi
[[ -n "$image_tar" && -f "$image_tar" ]] || die "missing exported image tarball in $export_dir"

install_incus_if_needed
ensure_incus_ready
ensure_smoke_profile

incus_cmd delete "$container_name" --force >/dev/null 2>&1 || true
incus_cmd image delete "$image_alias" >/dev/null 2>&1 || true

log "importing $image_tar as $image_alias"
fingerprint="$(sha256sum "$image_tar" | awk '{print $1}')"
if incus_cmd image info "$fingerprint" >/dev/null 2>&1; then
    log "image fingerprint $fingerprint already exists; reusing it"
    incus_cmd image alias create "$image_alias" "$fingerprint"
else
    incus_cmd image import "$image_tar" --alias "$image_alias"
fi

log "launching $container_name"
incus_cmd init "$image_alias" "$container_name" --profile default --profile "$profile_name"

log "checking stopped image does not contain persisted runtime state"
for path in \
    /home/lab/.lab/.env \
    /root/.lab/.env \
    /run/labby-ts-authkey \
    /var/lib/tailscale/tailscaled.state
do
    if container_file_exists "$container_name" "$path"; then
        echo "forbidden baked runtime state exists: $path" >&2
        exit 1
    fi
done

incus_cmd start "$container_name"
wait_for_running "$container_name"

log "checking baked toolchain"
incus_cmd exec "$container_name" -- su - lab -c 'set -e
node --version
npm --version
uv --version
python --version
rustc --version
cargo --version
go version
claude --version
codex --version
gemini --version'

log "checking root-level tools"
incus_cmd exec "$container_name" -- sh -lc 'set -e
ffmpeg -version | head -1
adb version | head -2
tailscale version | head -1
labby --version'

log "checking image does not contain runtime secrets"
# shellcheck disable=SC2016
incus_cmd exec "$container_name" -- sh -lc 'set -eu
for path in \
    /home/lab/.lab/.env \
    /root/.lab/.env \
    /run/labby-ts-authkey
do
    if test -e "$path"; then
        echo "forbidden runtime state exists: $path" >&2
        exit 1
    fi
done
if env | grep -E "^(TS_AUTHKEY|LAB_MCP_HTTP_TOKEN|OPENAI_API_KEY|ANTHROPIC_API_KEY|GITHUB_TOKEN|GH_TOKEN|NPM_TOKEN|CARGO_REGISTRY_TOKEN)=" >&2; then
    exit 1
fi'

log "checking provision convergence"
incus_cmd exec "$container_name" -- labby setup --provision --yes
incus_cmd exec "$container_name" -- systemctl is-active labby
incus_cmd exec "$container_name" -- curl -fsS http://127.0.0.1:8765/ready

log "image smoke test passed"
