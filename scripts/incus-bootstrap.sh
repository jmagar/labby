#!/bin/sh
# Bootstrap Labby into an Incus Ubuntu 24.04 system container.

set -eu

NAME="labby"
IMAGE="images:ubuntu/24.04"
VERSION="${LAB_INSTALL_VERSION:-}"
LOCAL_BINARY=""
DRY_RUN=0
TAILSCALE_SSH=0
ALLOW_SOURCE_FALLBACK=0

say() { printf '%s\n' "$*"; }
err() { printf '%s\n' "$*" >&2; }
fail() { err "incus-bootstrap.sh: $*"; exit 1; }

usage() {
    cat <<'USAGE'
Usage: scripts/incus-bootstrap.sh --version vX.Y.Z [options]

Options:
  --name NAME                 Container name (default: labby)
  --image IMAGE               Incus image alias (default: images:ubuntu/24.04)
  --version TAG               Labby release tag to install, e.g. v0.22.2
  --local-binary PATH          Push a locally built labby binary instead of downloading a release
  --dry-run                   Print commands only
  --tailscale-ssh             Run tailscale up with --ssh when TS_AUTHKEY is set
  --allow-source-fallback     Allow install.sh cargo fallback if release asset is unavailable
  -h, --help                  Show this help

Environment:
  TS_AUTHKEY                  Optional Tailscale auth key for in-container join
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

while [ "$#" -gt 0 ]; do
    case "$1" in
        --name) NAME="${2:?missing --name value}"; shift 2 ;;
        --image) IMAGE="${2:?missing --image value}"; shift 2 ;;
        --version) VERSION="${2:?missing --version value}"; shift 2 ;;
        --local-binary) LOCAL_BINARY="${2:?missing --local-binary value}"; shift 2 ;;
        --dry-run|--print-only) DRY_RUN=1; shift ;;
        --tailscale-ssh) TAILSCALE_SSH=1; shift ;;
        --allow-source-fallback) ALLOW_SOURCE_FALLBACK=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) fail "unknown argument: $1" ;;
    esac
done

if [ -z "$VERSION" ] && [ -z "$LOCAL_BINARY" ]; then
    fail "--version is required unless --local-binary is provided"
fi
if [ -n "$LOCAL_BINARY" ] && [ "$DRY_RUN" -eq 0 ] && [ ! -f "$LOCAL_BINARY" ]; then
    fail "--local-binary path does not exist: $LOCAL_BINARY"
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

if [ "$INCUS_AVAILABLE" -eq 0 ] && [ "$DRY_RUN" -eq 1 ]; then
    run incus launch "$IMAGE" "$NAME" -c security.privileged=true -c security.nesting=false
elif ! incus list "$NAME" -c n --format csv 2>/dev/null | grep -qx "$NAME"; then
    run incus launch "$IMAGE" "$NAME" -c security.privileged=true -c security.nesting=false
else
    say "container exists: $NAME"
    run incus config set "$NAME" security.privileged true
    run incus config set "$NAME" security.nesting false
    if ! incus list "$NAME" -c s --format csv 2>/dev/null | grep -qx RUNNING; then
        run incus start "$NAME"
    fi
fi

if [ "$INCUS_AVAILABLE" -eq 0 ] && [ "$DRY_RUN" -eq 1 ]; then
    run incus config device add "$NAME" tun unix-char path=/dev/net/tun
elif [ "$DRY_RUN" -eq 0 ] && incus exec "$NAME" -- test -c /dev/net/tun 2>/dev/null; then
    say "TUN device already present in container"
elif ! incus config device show "$NAME" | grep -q '^tun:'; then
    run incus config device add "$NAME" tun unix-char path=/dev/net/tun
else
    say "TUN passthrough already configured"
fi

if [ "$DRY_RUN" -eq 0 ]; then
    incus exec "$NAME" -- test -c /dev/net/tun || fail "$NAME is missing /dev/net/tun"
fi

if [ -n "$LOCAL_BINARY" ]; then
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
        LAB_INSTALL_REPO=jmagar/lab \
        LAB_INSTALL_VERSION="$VERSION" \
        LAB_REQUIRE_CHECKSUM=1 \
        LAB_ALLOW_SOURCE_FALLBACK="$fallback" \
        sh /tmp/labby-install.sh
    run incus exec "$NAME" -- rm -f /tmp/labby-install.sh
fi

run incus exec "$NAME" -- labby setup --provision --yes

if [ -n "${TS_AUTHKEY:-}" ]; then
	run incus exec "$NAME" -- sh -c "curl -fsSL https://tailscale.com/install.sh | sh"
	ts_args="--auth-key=file:/run/labby-ts-authkey"
	if [ "$TAILSCALE_SSH" -eq 1 ]; then
		ts_args="$ts_args --ssh"
	fi
	if [ "$DRY_RUN" -eq 1 ]; then
		say "+ incus exec $(quote "$NAME") -- tailscale up $ts_args"
	else
		incus exec "$NAME" -- sh -c "umask 077; cat > /run/labby-ts-authkey" <<EOF
$TS_AUTHKEY
EOF
		trap 'incus exec "$NAME" -- rm -f /run/labby-ts-authkey >/dev/null 2>&1 || true' EXIT INT TERM
		set +e
		# shellcheck disable=SC2086
		incus exec "$NAME" -- tailscale up $ts_args
		ts_status=$?
		set -e
		incus exec "$NAME" -- rm -f /run/labby-ts-authkey
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
