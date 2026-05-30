#!/usr/bin/env bash
# Pure-bash emit fallback used until bin/broadcastr-emit is compiled.
# Slower than the Rust binary; produces non-ULID-monotonic IDs. Routes all
# JSON construction through jq so caller-supplied summaries / paths / data
# cannot inject malformed JSON regardless of what they contain.
set -euo pipefail

[ "${BROADCASTR_DISABLED:-}" = "1" ] && exit 0

if ! command -v jq >/dev/null 2>&1; then
  echo "broadcastr-fallback: jq required but not installed; event dropped" >&2
  exit 0
fi

CATEGORY="" TIER="" SUMMARY="" SOURCE=cli DATA="{}" BRANCH=""
while [ $# -gt 0 ]; do
  case "$1" in
    --category) CATEGORY="$2"; shift 2;;
    --tier) TIER="$2"; shift 2;;
    --summary) SUMMARY="$2"; shift 2;;
    --source) SOURCE="$2"; shift 2;;
    --data) DATA="$2"; shift 2;;
    --branch) BRANCH="$2"; shift 2;;
    --) shift; break;;
    *)
      echo "broadcastr-fallback: unknown arg: $1" >&2
      shift
      ;;
  esac
done

REPO="${CLAUDE_PROJECT_DIR:-$PWD}"
PER_REPO_BUS="$REPO/.broadcastr/events.jsonl"
GLOBAL_HOME="${BROADCASTR_HOME:-$HOME/.claude/broadcastr}"
GLOBAL_BUS="$GLOBAL_HOME/events.jsonl"

TS="$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)"

# Generate a 26-char Crockford-base32-ish suffix in ONE shot. Read enough
# bytes that tr's per-class retention (~12.5%) yields >=26 chars even in the
# worst case (256 bytes * 0.125 = 32 expected chars). Trim to 26.
RAND="$(head -c 256 /dev/urandom | LC_ALL=C tr -dc '0-9A-HJKMNP-TV-Z' | head -c 26)"
if [ "${#RAND}" -lt 26 ]; then
  RAND="$(printf '%s%s' "$RAND" "$(head -c 256 /dev/urandom | LC_ALL=C tr -dc '0-9A-HJKMNP-TV-Z' | head -c 26)" | head -c 26)"
fi
ID="evt_${RAND}"

SESSION_ID="${CLAUDE_SESSION_ID:-}"
AGENT="user"
[ -n "$SESSION_ID" ] && AGENT="claude-code"
HOST="${HOSTNAME:-$(hostname)}"
USER_NAME="${USER:-$(id -un)}"

# Validate DATA is JSON. If not, replace with an object that preserves the
# raw string + a parse-failure marker so the bug is visible downstream.
if ! printf '%s' "$DATA" | jq -e . >/dev/null 2>&1; then
  DATA="$(jq -n --arg raw "$DATA" '{_parse_error: "invalid JSON in --data", _raw: $raw}')"
fi

# Build the entire event through jq so every string is properly escaped and
# the output is guaranteed valid JSON regardless of input contents.
LINE="$(jq -nc \
  --arg ts "$TS" \
  --arg id "$ID" \
  --arg tier "$TIER" \
  --arg category "$CATEGORY" \
  --arg source "$SOURCE" \
  --arg session_id "$SESSION_ID" \
  --arg agent "$AGENT" \
  --arg host "$HOST" \
  --arg user "$USER_NAME" \
  --arg repo "$REPO" \
  --arg summary "$SUMMARY" \
  --arg branch "$BRANCH" \
  --argjson data "$DATA" \
  '{
    ts: $ts,
    id: $id,
    tier: $tier,
    category: $category,
    source: $source,
    emitter: {
      session_id: (if $session_id == "" then null else $session_id end),
      agent: $agent,
      host: $host,
      user: $user
    },
    repo: $repo,
    summary: $summary,
    data: $data
  } + (if $branch == "" then {} else {branch: $branch} end)')"

append_with_rotate() {
  local bus="$1"
  mkdir -p "$(dirname "$bus")"
  local max="${BROADCASTR_BUS_MAX_BYTES:-5242880}"
  local retain="${BROADCASTR_BUS_RETAIN:-3}"
  [ "$retain" -lt 1 ] && retain=1

  if [ -f "$bus" ]; then
    local size
    size=$(stat -c %s "$bus" 2>/dev/null || stat -f %z "$bus" 2>/dev/null || echo 0)
    if [ "$size" -ge "$max" ]; then
      local lock="${bus}.rotate.lock"
      (
        flock -n 9 || exit 0
        local s2
        s2=$(stat -c %s "$bus" 2>/dev/null || stat -f %z "$bus" 2>/dev/null || echo 0)
        if [ "$s2" -ge "$max" ]; then
          local i=$((retain - 1))
          while [ "$i" -ge 1 ]; do
            [ -f "${bus}.${i}" ] && mv "${bus}.${i}" "${bus}.$((i+1))"
            i=$((i-1))
          done
          mv "$bus" "${bus}.1"
          : > "$bus"
        fi
      ) 9>"$lock"
    fi
  fi
  printf '%s\n' "$LINE" >> "$bus"
}

append_with_rotate "$PER_REPO_BUS"

if [ "${BROADCASTR_GLOBAL_FEED:-1}" != "0" ]; then
  append_with_rotate "$GLOBAL_BUS"
fi
