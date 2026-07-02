#!/bin/sh
# Bootstrap Labby into an Incus Ubuntu 24.04 system container.

set -eu

NAME="labby"
IMAGE="images:ubuntu/24.04"
PROFILE_NAME="labby-gateway"
PROFILE_FILE="config/incus/labby-gateway-profile.yaml"
BACKUP_CONFIG_FILE="${LABBY_INCUS_BACKUP_CONFIG:-config/incus/labby-backup.yaml}"
STORAGE_POOL_DRIVER="${LABBY_INCUS_STORAGE_DRIVER:-zfs}"
STORAGE_POOL_NAME="${LABBY_INCUS_STORAGE_POOL:-}"
STORAGE_POOL_SOURCE="${LABBY_INCUS_STORAGE_SOURCE:-${LABBY_INCUS_ZFS_SOURCE:-}}"
RUNTIME_PROFILE_NAME=""
VERSION="${LAB_INSTALL_VERSION:-}"
LOCAL_BINARY=""
SKIP_INSTALL=0
DRY_RUN=0
TAILSCALE_SSH=0
TAILSCALE_HOSTNAME=""
ALLOW_SOURCE_FALLBACK=0
APPLY_BACKUP_CONFIG=1

say() { printf '%s\n' "$*"; }
err() { printf '%s\n' "$*" >&2; }
fail() { err "incus-bootstrap.sh: $*"; exit 1; }

usage() {
    cat <<'USAGE'
Usage: scripts/incus-bootstrap.sh --version vX.Y.Z [options]

Options:
  --name NAME                 Container name (default: labby)
  --image IMAGE               Incus image alias (default: images:ubuntu/24.04)
  --profile-name NAME          Incus profile name (default: labby-gateway)
  --profile-file PATH          Incus profile YAML (default: config/incus/labby-gateway-profile.yaml)
  --backup-config PATH         Incus snapshot policy YAML (default: config/incus/labby-backup.yaml)
  --no-backup-config           Do not apply an Incus snapshot policy
  --runtime-profile-name NAME  Rootless profile for existing containers with a different root pool
  --storage-driver DRIVER      Incus storage driver: zfs, btrfs, or dir (default: zfs)
  --storage-pool NAME          Incus storage pool used by the profile root disk
  --storage-source SOURCE      Incus storage source path/dataset for the pool
  --zfs-source DATASET         Back-compat alias for --storage-source with zfs
  --version TAG               Labby release tag to install, e.g. v0.22.2
  --local-binary PATH          Push a locally built labby binary instead of downloading a release
  --skip-install              Use the labby binary already baked into the selected image
  --dry-run                   Print commands only
  --tailscale-ssh             Run tailscale up with --ssh when TS_AUTHKEY is set
  --tailscale-hostname NAME    Tailscale hostname (default: container name)
  --allow-source-fallback     Allow install.sh cargo fallback if release asset is unavailable
  -h, --help                  Show this help

Environment:
  TS_AUTHKEY                  Optional Tailscale auth key for in-container join
  LABBY_INCUS_STORAGE_DRIVER  Optional Incus storage driver: zfs, btrfs, or dir
  LABBY_INCUS_STORAGE_POOL    Optional Incus storage pool name
  LABBY_INCUS_STORAGE_SOURCE  Optional Incus storage pool source path/dataset
  LABBY_INCUS_ZFS_SOURCE      Back-compat ZFS dataset source env
  LABBY_INCUS_BACKUP_CONFIG   Optional Incus snapshot policy YAML path
USAGE
}

quote() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '+'
        for arg in "$@"; do
            printf ' %s' "$(quote "$arg")"
        done
        printf '\n'
    else
        "$@"
    fi
}

normalize_profile_yaml() {
    awk '
        /^used_by:/ { print "used_by: []"; in_used_by = 1; next }
        in_used_by && /^- / { next }
        in_used_by { in_used_by = 0 }
        /^project:/ { next }
        { print }
    '
}

verify_container_substrate() {
    [ "$DRY_RUN" -eq 1 ] && return 0

    arch="$(incus exec "$NAME" -- uname -m | tr -d '\r')"
    case "$arch" in
        x86_64 | amd64) ;;
        aarch64 | arm64)
            if [ -n "$LOCAL_BINARY" ] || [ "$ALLOW_SOURCE_FALLBACK" -eq 1 ]; then
                say "$NAME is $arch; continuing because --local-binary or --allow-source-fallback was provided"
            else
                fail "$NAME must be amd64/x86_64 for the release install path; found architecture: $arch. Use --local-binary or --allow-source-fallback for arm64."
            fi
            ;;
        *) fail "$NAME must be amd64/x86_64 for the supported Labby runtime; found architecture: $arch" ;;
    esac

    os_release="$(incus exec "$NAME" -- sh -c ". /etc/os-release; printf '%s %s' \"\$ID\" \"\$VERSION_ID\"")"
    [ "$os_release" = "ubuntu 24.04" ] \
        || fail "$NAME must be Ubuntu 24.04 for the supported Labby runtime; found: $os_release"
}

ensure_tun_device() {
    if [ "$DRY_RUN" -eq 1 ]; then
        say "+ incus config show $(quote "$NAME") --expanded # verify profile-provided tun access"
        return
    fi

    expanded_config="$(incus config show "$NAME" --expanded)"
    tun_path="$(
        printf '%s\n' "$expanded_config" |
            awk '
                /^devices:/ { in_devices = 1; next }
                in_devices && /^  tun:/ { in_tun = 1; next }
                in_tun && /^    path:/ { print $2; exit }
                in_tun && /^  [^ ]/ { in_tun = 0 }
            '
    )"
    if [ "$tun_path" = "/dev/net/tun" ]; then
        say "TUN passthrough configured by profile device"
    elif printf '%s\n' "$expanded_config" | grep -Fq "lxc.mount.entry = /dev/net/tun dev/net/tun none bind,create=file 0 0"; then
        say "TUN passthrough configured by raw.lxc bind mount"
    else
        fail "Incus profile '$PROFILE_NAME' must provide /dev/net/tun via a tun device or raw.lxc bind mount"
    fi

    if [ "$DRY_RUN" -eq 0 ]; then
        incus exec "$NAME" -- test -c /dev/net/tun || fail "$NAME is missing /dev/net/tun"
        incus exec "$NAME" -- sh -c "ip tuntap add dev labby-tun-probe mode tun && ip link delete labby-tun-probe" \
            || fail "$NAME cannot create a test TUN interface; Tailscale will not work with the current Incus profile"
    fi
}

ensure_apparmor_signal_rule() {
    if [ "$DRY_RUN" -eq 1 ]; then
        say "+ incus config show $(quote "$NAME") --expanded # verify AppArmor signal peer rule"
        return
    fi

    expanded_config="$(incus config show "$NAME" --expanded)"
    if printf '%s\n' "$expanded_config" | grep -Fq "signal peer=@{profile_name}//&unconfined,"; then
        say "AppArmor signal peer rule configured"
    else
        fail "Incus profile '$PROFILE_NAME' must set raw.apparmor='signal peer=@{profile_name}//&unconfined,' so systemd can stop services inside the container"
    fi
}

ensure_container_networking() {
    if [ "$DRY_RUN" -eq 1 ]; then
        say "+ incus exec $(quote "$NAME") -- sh -c 'write Incus DHCP netplan, enable systemd-networkd, generate networkd config, verify IPv4/DNS'"
        return
    fi

    incus exec "$NAME" -- sh -eu <<'SCRIPT'
install -d -m 0755 /etc/netplan
cat > /etc/netplan/10-lxc.yaml <<'EOF'
network:
  version: 2
  ethernets:
    eth0:
      dhcp4: true
      dhcp-identifier: mac
EOF
chmod 0600 /etc/netplan/10-lxc.yaml
systemctl enable systemd-networkd systemd-resolved >/dev/null
netplan_err="$(mktemp)"
if ! netplan generate 2>"$netplan_err"; then
    cat "$netplan_err" >&2
    rm -f "$netplan_err"
    exit 1
fi
if [ -s "$netplan_err" ]; then
    grep -v '^Failed to send reload request: No such file or directory$' "$netplan_err" >&2 || true
fi
rm -f "$netplan_err"
if ip -4 addr show dev eth0 | grep -q 'inet ' && getent hosts tailscale.com >/dev/null 2>&1; then
    exit 0
fi
systemctl restart systemd-networkd systemd-resolved
SCRIPT

    i=0
    while [ "$i" -lt 30 ]; do
        if incus exec "$NAME" -- sh -c "ip -4 addr show dev eth0 | grep -q 'inet '"; then
            break
        fi
        i=$((i + 1))
        sleep 1
    done
    incus exec "$NAME" -- sh -c "ip -4 addr show dev eth0 | grep -q 'inet '" \
        || fail "$NAME did not acquire an IPv4 address on eth0"

    if ! incus exec "$NAME" -- getent hosts tailscale.com >/dev/null 2>&1; then
        incus exec "$NAME" -- resolvectl status || true
        fail "$NAME cannot resolve tailscale.com after network convergence"
    fi
    say "container networking ready: eth0 has IPv4 and DNS resolves"
}

cleanup_ts_authkey() {
    incus exec "$NAME" -- rm -f /run/labby-ts-authkey >/dev/null 2>&1 || true
}

verify_labby_ready() {
    if [ "$DRY_RUN" -eq 1 ]; then
        say "+ incus exec $(quote "$NAME") -- curl -fsS http://127.0.0.1:8765/ready"
        return
    fi
    incus exec "$NAME" -- curl -fsS http://127.0.0.1:8765/ready >/dev/null
}

parse_backup_config() {
    awk '
        function trim(value) {
            sub(/^[[:space:]]+/, "", value)
            sub(/[[:space:]]+$/, "", value)
            return value
        }
        /^config:[[:space:]]*$/ { in_config = 1; next }
        in_config && /^[^[:space:]]/ { in_config = 0 }
        !in_config { next }
        /^[[:space:]]*(#|$)/ { next }
        {
            line = $0
            sub(/^[[:space:]]+/, "", line)
            colon = index(line, ":")
            if (colon == 0) {
                next
            }
            key = trim(substr(line, 1, colon - 1))
            value = trim(substr(line, colon + 1))
            if ((value ~ /^".*"$/) || (value ~ /^'\''.*'\''$/)) {
                value = substr(value, 2, length(value) - 2)
            }
            print key "=" value
        }
    ' "$BACKUP_CONFIG_FILE"
}

validate_backup_key() {
    case "$1" in
        snapshots.schedule | snapshots.expiry | snapshots.pattern | snapshots.schedule.stopped) ;;
        *) fail "unsupported backup config key '$1' in $BACKUP_CONFIG_FILE" ;;
    esac
}

host_labby_supports_incus_backup() {
    command -v labby >/dev/null 2>&1 && labby setup incusbackup --help >/dev/null 2>&1
}

apply_backup_config_with_shell() {
    applied=0
    while IFS= read -r entry; do
        [ -n "$entry" ] || continue
        key="${entry%%=*}"
        value="${entry#*=}"
        validate_backup_key "$key"
        run incus config set "$NAME" "$key" "$value"
        applied=$((applied + 1))
    done <<EOF
$(parse_backup_config)
EOF
    [ "$applied" -gt 0 ] || fail "$BACKUP_CONFIG_FILE must contain at least one supported config key"
}

validate_backup_config_with_shell() {
    validated=0
    while IFS= read -r entry; do
        [ -n "$entry" ] || continue
        key="${entry%%=*}"
        validate_backup_key "$key"
        validated=$((validated + 1))
    done <<EOF
$(parse_backup_config)
EOF
    [ "$validated" -gt 0 ] || fail "$BACKUP_CONFIG_FILE must contain at least one supported config key"
}

apply_backup_config() {
    [ "$APPLY_BACKUP_CONFIG" -eq 1 ] || return 0

    [ -f "$BACKUP_CONFIG_FILE" ] \
        || fail "--backup-config path does not exist: $BACKUP_CONFIG_FILE"

    if [ "$DRY_RUN" -eq 1 ]; then
        if host_labby_supports_incus_backup; then
            labby setup incusbackup validate --config "$BACKUP_CONFIG_FILE" >/dev/null
        else
            validate_backup_config_with_shell
        fi
        say "+ labby setup incusbackup apply --name $(quote "$NAME") --config $(quote "$BACKUP_CONFIG_FILE") --dry-run"
        return
    fi

    if host_labby_supports_incus_backup; then
        run labby setup incusbackup apply --name "$NAME" --config "$BACKUP_CONFIG_FILE" --yes
    else
        apply_backup_config_with_shell
    fi
}

ensure_storage_pool() {
    case "$STORAGE_POOL_DRIVER" in
        zfs | btrfs | dir) ;;
        *) fail "--storage-driver must be 'zfs', 'btrfs', or 'dir', got: $STORAGE_POOL_DRIVER" ;;
    esac

    if [ "$DRY_RUN" -eq 1 ]; then
        if [ -n "$STORAGE_POOL_SOURCE" ]; then
            say "+ incus storage show $(quote "$STORAGE_POOL_NAME") >/dev/null 2>&1 || incus storage create $(quote "$STORAGE_POOL_NAME") $(quote "$STORAGE_POOL_DRIVER") source=$(quote "$STORAGE_POOL_SOURCE")"
        else
            say "+ incus storage show $(quote "$STORAGE_POOL_NAME") >/dev/null 2>&1 || incus storage create $(quote "$STORAGE_POOL_NAME") $(quote "$STORAGE_POOL_DRIVER")"
        fi
        return
    fi

    if incus storage show "$STORAGE_POOL_NAME" >/dev/null 2>&1; then
        driver="$(incus storage show "$STORAGE_POOL_NAME" | awk '$1 == "driver:" { print $2; exit }')"
        [ "$driver" = "$STORAGE_POOL_DRIVER" ] \
            || fail "Incus storage pool '$STORAGE_POOL_NAME' exists but uses driver '$driver', expected $STORAGE_POOL_DRIVER"
        say "storage pool already exists: $STORAGE_POOL_NAME"
    else
        if [ -n "$STORAGE_POOL_SOURCE" ]; then
            run incus storage create "$STORAGE_POOL_NAME" "$STORAGE_POOL_DRIVER" source="$STORAGE_POOL_SOURCE"
        else
            run incus storage create "$STORAGE_POOL_NAME" "$STORAGE_POOL_DRIVER"
        fi
    fi
}

ensure_profile() {
    [ "$DRY_RUN" -eq 0 ] && [ -f "$PROFILE_FILE" ] \
        || [ "$DRY_RUN" -eq 1 ] \
        || fail "--profile-file path does not exist: $PROFILE_FILE"

    if [ "$DRY_RUN" -eq 1 ]; then
        say "+ incus profile show $(quote "$PROFILE_NAME") >/dev/null 2>&1 || incus profile create $(quote "$PROFILE_NAME")"
        if [ "$STORAGE_POOL_NAME" = "labby-zfs" ]; then
            say "+ incus profile edit $(quote "$PROFILE_NAME") < $(quote "$PROFILE_FILE")"
        else
            say "+ sed 's/^    pool: .*/    pool: $STORAGE_POOL_NAME/' $(quote "$PROFILE_FILE") | incus profile edit $(quote "$PROFILE_NAME")"
        fi
        return
    fi

    if ! incus profile show "$PROFILE_NAME" >/dev/null 2>&1; then
        run incus profile create "$PROFILE_NAME"
    fi
    profile_source="$PROFILE_FILE"
    profile_tmp=""
    if [ "$STORAGE_POOL_NAME" != "labby-zfs" ]; then
        profile_tmp="$(mktemp)"
        sed "s/^    pool: .*/    pool: $STORAGE_POOL_NAME/" "$PROFILE_FILE" > "$profile_tmp"
        profile_source="$profile_tmp"
    fi
    current_tmp="$(mktemp)"
    desired_tmp="$(mktemp)"
    incus profile show "$PROFILE_NAME" | normalize_profile_yaml > "$current_tmp"
    normalize_profile_yaml < "$profile_source" > "$desired_tmp"
    if cmp -s "$current_tmp" "$desired_tmp"; then
        say "profile already matches: $PROFILE_NAME"
        rm -f "$current_tmp" "$desired_tmp"
        if [ -n "$profile_tmp" ]; then
            rm -f "$profile_tmp"
        fi
        return
    fi
    rm -f "$current_tmp" "$desired_tmp"
    timeout 60 incus profile edit "$PROFILE_NAME" < "$profile_source" \
        || fail "timed out updating Incus profile '$PROFILE_NAME'"
    if [ -n "$profile_tmp" ]; then
        rm -f "$profile_tmp"
    fi
}

write_rootless_profile() {
    runtime_name="$1"
    awk -v runtime="$runtime_name" '
        /^name:/ { print "name: " runtime; next }
        /^used_by:/ { print "used_by: []"; next }
        /^devices:/ { in_devices = 1; print; next }
        in_devices && /^  root:/ { skip = 1; next }
        skip && /^  [^ ]/ { skip = 0 }
        skip && /^[^ ]/ { skip = 0; in_devices = 0 }
        in_devices && /^[^ ]/ { in_devices = 0 }
        !skip { print }
    ' "$PROFILE_FILE"
}

ensure_runtime_profile() {
    runtime_name="$1"

    if [ "$DRY_RUN" -eq 1 ]; then
        say "+ incus profile show $(quote "$runtime_name") >/dev/null 2>&1 || incus profile create $(quote "$runtime_name")"
        say "+ derive $(quote "$runtime_name") from $(quote "$PROFILE_FILE") without devices.root, then incus profile edit $(quote "$runtime_name")"
        return
    fi

    if ! incus profile show "$runtime_name" >/dev/null 2>&1; then
        run incus profile create "$runtime_name"
    fi

    profile_tmp="$(mktemp)"
    write_rootless_profile "$runtime_name" > "$profile_tmp"
    timeout 60 incus profile edit "$runtime_name" < "$profile_tmp" \
        || fail "timed out updating Incus profile '$runtime_name'"
    rm -f "$profile_tmp"
}

container_has_profile() {
    profile="$1"
    incus config show "$NAME" |
        awk -v profile="$profile" '
            /^profiles:/ { in_profiles = 1; next }
            in_profiles && /^- / { if (substr($0, 3) == profile) found = 1; next }
            in_profiles && /^[^ ]/ { in_profiles = 0 }
            END { exit found ? 0 : 1 }
        '
}

container_root_pool() {
    incus config show "$NAME" --expanded |
        awk '
            /^devices:/ { in_devices = 1; next }
            in_devices && /^  root:/ { in_root = 1; next }
            in_root && /^    pool:/ { print $2; exit }
            in_root && /^  [^ ]/ { in_root = 0 }
            in_devices && /^[^ ]/ { in_devices = 0 }
        '
}

profile_root_pool() {
    incus profile device get "$PROFILE_NAME" root pool 2>/dev/null || true
}

ensure_container_profile() {
    if [ "$DRY_RUN" -eq 1 ]; then
        say "+ add $(quote "$PROFILE_NAME") unless existing root pool differs; then add rootless runtime profile"
        return
    fi

    container_pool="$(container_root_pool)"
    profile_pool="$(profile_root_pool)"
    profile_to_add="$PROFILE_NAME"

    if [ -n "$container_pool" ] && [ -n "$profile_pool" ] && [ "$container_pool" != "$profile_pool" ]; then
        runtime_name="${RUNTIME_PROFILE_NAME:-$PROFILE_NAME-runtime}"
        say "container root pool '$container_pool' differs from profile root pool '$profile_pool'; using rootless runtime profile: $runtime_name"
        ensure_runtime_profile "$runtime_name"
        profile_to_add="$runtime_name"
    fi

    if container_has_profile "$profile_to_add"; then
        say "profile already applied: $profile_to_add"
    elif container_has_profile "$PROFILE_NAME" && [ "$profile_to_add" = "$PROFILE_NAME" ]; then
        say "profile already applied: $PROFILE_NAME"
    else
        run incus profile add "$NAME" "$profile_to_add"
    fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --name) NAME="${2:?missing --name value}"; shift 2 ;;
        --image) IMAGE="${2:?missing --image value}"; shift 2 ;;
        --profile-name) PROFILE_NAME="${2:?missing --profile-name value}"; shift 2 ;;
        --profile-file) PROFILE_FILE="${2:?missing --profile-file value}"; shift 2 ;;
        --backup-config) BACKUP_CONFIG_FILE="${2:?missing --backup-config value}"; APPLY_BACKUP_CONFIG=1; shift 2 ;;
        --no-backup-config) APPLY_BACKUP_CONFIG=0; shift ;;
        --runtime-profile-name) RUNTIME_PROFILE_NAME="${2:?missing --runtime-profile-name value}"; shift 2 ;;
        --storage-driver) STORAGE_POOL_DRIVER="${2:?missing --storage-driver value}"; shift 2 ;;
        --storage-pool) STORAGE_POOL_NAME="${2:?missing --storage-pool value}"; shift 2 ;;
        --storage-source) STORAGE_POOL_SOURCE="${2:?missing --storage-source value}"; shift 2 ;;
        --zfs-source) STORAGE_POOL_SOURCE="${2:?missing --zfs-source value}"; shift 2 ;;
        --version) VERSION="${2:?missing --version value}"; shift 2 ;;
        --local-binary) LOCAL_BINARY="${2:?missing --local-binary value}"; shift 2 ;;
        --skip-install) SKIP_INSTALL=1; shift ;;
        --dry-run|--print-only) DRY_RUN=1; shift ;;
        --tailscale-ssh) TAILSCALE_SSH=1; shift ;;
        --tailscale-hostname) TAILSCALE_HOSTNAME="${2:?missing --tailscale-hostname value}"; shift 2 ;;
        --allow-source-fallback) ALLOW_SOURCE_FALLBACK=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) fail "unknown argument: $1" ;;
    esac
done

case "$STORAGE_POOL_DRIVER" in
    zfs | btrfs | dir) ;;
    *) fail "--storage-driver must be 'zfs', 'btrfs', or 'dir', got: $STORAGE_POOL_DRIVER" ;;
esac
if [ -z "$STORAGE_POOL_NAME" ]; then
    case "$STORAGE_POOL_DRIVER" in
        zfs) STORAGE_POOL_NAME="labby-zfs" ;;
        btrfs) STORAGE_POOL_NAME="labby-btrfs" ;;
        dir) STORAGE_POOL_NAME="labby-dir" ;;
    esac
fi
if [ "$STORAGE_POOL_DRIVER" = "zfs" ] && [ -z "$STORAGE_POOL_SOURCE" ]; then
    STORAGE_POOL_SOURCE="rpool/labby-incus"
fi
if [ -z "$VERSION" ] && [ -z "$LOCAL_BINARY" ] && [ "$SKIP_INSTALL" -eq 0 ]; then
    fail "--version is required unless --local-binary or --skip-install is provided"
fi
if [ "$SKIP_INSTALL" -eq 1 ] && [ -n "$LOCAL_BINARY" ]; then
    fail "--skip-install cannot be combined with --local-binary"
fi
if [ -n "$LOCAL_BINARY" ] && [ "$DRY_RUN" -eq 0 ] && [ ! -f "$LOCAL_BINARY" ]; then
    fail "--local-binary path does not exist: $LOCAL_BINARY"
fi
if [ -z "$TAILSCALE_HOSTNAME" ]; then
    TAILSCALE_HOSTNAME="$NAME"
fi

INCUS_AVAILABLE=1
if ! command -v incus >/dev/null 2>&1; then
    INCUS_AVAILABLE=0
    cat >&2 <<'MISSING'
Incus is not installed or not on PATH.

Install and initialize it explicitly, then rerun this script. For Debian/Ubuntu:
  sudo apt install incus
  sudo incus admin init

This bootstrap does not install or initialize Incus automatically.
MISSING
    [ "$DRY_RUN" -eq 1 ] || exit 1
fi

if [ "$INCUS_AVAILABLE" -eq 1 ] && ! incus info >/dev/null 2>&1; then
    if [ "$DRY_RUN" -eq 1 ]; then
        err "Incus is present but not initialized or reachable; dry-run will still print the command plan."
        INCUS_AVAILABLE=0
    else
        fail "incus is present but not initialized or reachable; run 'incus admin init' explicitly"
    fi
fi

ensure_storage_pool
ensure_profile

if [ "$INCUS_AVAILABLE" -eq 0 ] && [ "$DRY_RUN" -eq 1 ]; then
    run incus launch "$IMAGE" "$NAME" --profile default --profile "$PROFILE_NAME"
elif ! incus list "$NAME" -c n --format csv 2>/dev/null | grep -qx "$NAME"; then
    run incus launch "$IMAGE" "$NAME" --profile default --profile "$PROFILE_NAME"
else
    say "container exists: $NAME"
    ensure_container_profile
    if ! incus list "$NAME" -c s --format csv 2>/dev/null | grep -qx RUNNING; then
        run incus start "$NAME"
    fi
fi

verify_container_substrate
ensure_tun_device
ensure_apparmor_signal_rule
ensure_container_networking
apply_backup_config
run incus exec "$NAME" -- hostnamectl set-hostname "$TAILSCALE_HOSTNAME"

if [ "$SKIP_INSTALL" -eq 1 ]; then
    run incus exec "$NAME" -- test -x /usr/local/bin/labby
elif [ -n "$LOCAL_BINARY" ]; then
    remote_tmp="/usr/local/bin/.labby-upload-$$"
    run incus exec "$NAME" -- mkdir -p /usr/local/bin
    run incus file push "$LOCAL_BINARY" "$NAME$remote_tmp"
    run incus exec "$NAME" -- chmod 0755 "$remote_tmp"
    run incus exec "$NAME" -- mv -f "$remote_tmp" /usr/local/bin/labby
else
    fallback="$ALLOW_SOURCE_FALLBACK"
    run incus file push scripts/install.sh "$NAME/tmp/labby-install.sh"
    run incus exec "$NAME" -- env \
        LAB_INSTALL_DIR=/usr/local/bin \
        LAB_INSTALL_REPO=jmagar/labby \
        LAB_INSTALL_VERSION="$VERSION" \
        LAB_REQUIRE_CHECKSUM=1 \
        LAB_ALLOW_SOURCE_FALLBACK="$fallback" \
        sh /tmp/labby-install.sh
    run incus exec "$NAME" -- rm -f /tmp/labby-install.sh
fi

run incus exec "$NAME" -- labby setup --provision --yes
verify_labby_ready

if [ -n "${TS_AUTHKEY:-}" ]; then
	if [ "$DRY_RUN" -eq 1 ]; then
		say "+ incus exec $(quote "$NAME") -- sh -c 'command -v tailscale >/dev/null || curl -fsSL https://tailscale.com/install.sh | sh'"
	elif ! incus exec "$NAME" -- sh -c "command -v tailscale >/dev/null 2>&1"; then
		run incus exec "$NAME" -- sh -c "curl -fsSL https://tailscale.com/install.sh | sh"
	fi
	ts_args="--auth-key=file:/run/labby-ts-authkey --hostname=$TAILSCALE_HOSTNAME"
	if [ "$TAILSCALE_SSH" -eq 1 ]; then
		ts_args="$ts_args --ssh"
	fi
	if [ "$DRY_RUN" -eq 1 ]; then
		say "+ incus exec $(quote "$NAME") -- tailscale up $ts_args"
	else
		trap cleanup_ts_authkey EXIT INT TERM
		printf '%s' "$TS_AUTHKEY" | incus exec "$NAME" -- sh -c "umask 077; cat > /run/labby-ts-authkey"
		set +e
		# shellcheck disable=SC2086
		incus exec "$NAME" -- tailscale up $ts_args
		ts_status=$?
		set -e
		cleanup_ts_authkey
		trap - EXIT INT TERM
		if [ "$ts_status" -ne 0 ]; then
			exit "$ts_status"
		fi
	fi
fi

cat <<DONE
Done. Manual steps remain:
  1. incus exec $NAME -- su - lab
  2. claude login && codex login && gemini
  3. verify service: incus exec $NAME -- systemctl status labby --no-pager
  4. verify readiness: incus exec $NAME -- curl -fsS http://127.0.0.1:8765/ready
  5. if Tailscale is enabled, verify: incus exec $NAME -- tailscale ip -4

Rollback:
  incus stop $NAME
  incus delete $NAME
DONE
