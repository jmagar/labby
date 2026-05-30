#!/usr/bin/env bash
# Smoke-test an MCP server's tools/resources via mcporter, with schema preflight
# and string-based response assertions.
#
# Modes:
#   ./smoke.sh <server>                  Run cases defined in CASES=() below.
#   ./smoke.sh --init <server>           Print a skeleton CASES=() block from the
#                                        server's inputSchema and exit.
#   ./smoke.sh --list-tools <server>     Print one tool name per line and exit.
#
# Case row format:
#   "label|args|assertion"
#     label     — tool name (e.g. search) or resource URI (e.g. ui://srv/x)
#     args      — appended to `mcporter call`; key=value or --args '{...}'
#     assertion — one of:
#                   (empty)            liveness only (call must not error)
#                   contains: TEXT     response text must include TEXT
#                   regex: PATTERN     bash ERE matched against response text
#                   jq: FILTER         response text parsed as JSON then jq -e
#                   error: KIND        expect a tool-error envelope with this kind
#
# Env:
#   TIMEOUT_MS  per-call timeout (default 15000)
#   VERBOSE=1   dump full response on any failure
#   NO_PREFLIGHT=1  skip input-schema preflight

set -euo pipefail

MODE="run"
case "${1:-}" in
  --init)        MODE="init";       shift ;;
  --list-tools)  MODE="list-tools"; shift ;;
  --help|-h)     sed -n '2,30p' "$0"; exit 0 ;;
esac

SERVER=${1:?usage: smoke.sh [--init|--list-tools] <server>}
TIMEOUT_MS=${TIMEOUT_MS:-15000}
VERBOSE=${VERBOSE:-0}
NO_PREFLIGHT=${NO_PREFLIGHT:-0}

# ---- cases ----------------------------------------------------------------
declare -a CASES=(
  # "tool_name|arg1=foo arg2=bar|contains: expected substring"
  # "search|--args {\"q\":\"hello\"}|regex: [0-9]+ result"
  # "ui://server/status||"
  # "bad_call|action=nope|error: invalid_param"
)
# ---------------------------------------------------------------------------

need() { command -v "$1" >/dev/null || { echo "missing dep: $1" >&2; exit 127; }; }
need mcporter; need jq

# Fetch schema JSON once (mcporter list <server> --schema --json) — cached for run.
SCHEMA_JSON=""
load_schema() {
  [[ -n "$SCHEMA_JSON" ]] && return 0
  SCHEMA_JSON=$(mcporter list "$SERVER" --schema --json 2>/dev/null) || {
    echo "warn: cannot load schema for $SERVER (server may be offline)" >&2
    SCHEMA_JSON="{}"; return 1
  }
}

# ----- init mode ------------------------------------------------------------
if [[ "$MODE" == "init" ]]; then
  load_schema
  echo "# Generated from $SERVER's inputSchema on $(date -Iseconds)"
  echo "# Required args are pre-filled with TODO placeholders. Edit before running."
  echo "declare -a CASES=("
  echo "$SCHEMA_JSON" | jq -r '
    .tools // [] | .[] |
    .name as $name |
    (.inputSchema.required // []) as $req |
    (.inputSchema.properties // {}) as $props |
    "  # " + ($name) + " — " + ((.description // "") | gsub("\n"; " ") | .[0:80]) + "\n" +
    "  \"" + $name + "|" +
      ($req | map(. + "=TODO") | join(" ")) +
    "|\""
  '
  echo ")"
  exit 0
fi

# ----- list-tools mode ------------------------------------------------------
if [[ "$MODE" == "list-tools" ]]; then
  load_schema
  echo "$SCHEMA_JSON" | jq -r '.tools // [] | .[].name'
  exit 0
fi

# ----- run mode -------------------------------------------------------------
# Preflight: server reachable / authenticated?
status=$(mcporter list --json 2>/dev/null \
  | jq -r --arg s "$SERVER" '.servers[] | select(.name==$s) | .status' || true)
if [[ -z "$status" ]]; then
  echo "note: $SERVER not in config, treating as ad-hoc"
elif [[ "$status" == "auth" ]]; then
  echo "$SERVER needs auth — run: mcporter auth $SERVER" >&2; exit 2
elif [[ "$status" == "offline" || "$status" == "error" ]]; then
  echo "$SERVER is $status" >&2; exit 2
fi

(( ${#CASES[@]} == 0 )) && { echo "no cases — edit CASES=() or run with --init" >&2; exit 2; }

[[ "$NO_PREFLIGHT" == "1" ]] || load_schema

pass=0; fail=0; failures=()

# Check that all required schema keys appear in the args string. Catches typos
# without needing a real JSON Schema validator.
preflight_args() {
  local tool="$1" args="$2"
  [[ "$NO_PREFLIGHT" == "1" ]] && return 0
  [[ "$tool" == *"://"* ]] && return 0   # resource URIs have no input schema
  [[ -z "$SCHEMA_JSON" || "$SCHEMA_JSON" == "{}" ]] && return 0

  local required
  required=$(echo "$SCHEMA_JSON" | jq -r --arg t "$tool" '
    .tools // [] | map(select(.name == $t)) | .[0] // empty
    | .inputSchema.required // [] | .[]
  ' 2>/dev/null)
  [[ -z "$required" ]] && return 0

  local missing=()
  while IFS= read -r key; do
    [[ -z "$key" ]] && continue
    # Match key= or "key": or `key:` (function-call form)
    if ! grep -qE "(^|[[:space:]\"'{,(])${key}([[:space:]]*[:=])" <<<"$args"; then
      missing+=("$key")
    fi
  done <<<"$required"

  if (( ${#missing[@]} > 0 )); then
    echo "${missing[*]}"; return 1
  fi
  return 0
}

run_one() {
  local label="$1" args="$2" assertion="$3"
  local selector="$SERVER.$label"
  printf "→ %-50s " "$selector"

  # Preflight
  local miss
  if miss=$(preflight_args "$label" "$args"); then :; else
    echo "FAIL  preflight: missing required args: $miss"
    failures+=("$selector [preflight]: missing $miss"); ((++fail)); return
  fi

  # Call (text output — --output json is broken in current mcporter)
  local out rc=0 raw
  out=$(eval "mcporter call '$selector' $args --output text --timeout $TIMEOUT_MS" 2>&1) || rc=$?

  # Detect errors. Three flavors:
  #   (a) transport failure   — mcporter exits non-zero
  #   (b) wrapper warning     — text starts with [mcporter] or contains "MCP error"
  #   (c) tool-level isError  — response has isError:true; text typically starts "Error:"
  # We always fetch --output raw to inspect the envelope (isError + kind extraction).
  local is_error=0 error_kind="" error_msg=""
  raw=$(eval "mcporter call '$selector' $args --output raw --timeout $TIMEOUT_MS" 2>&1 || true)
  if (( rc != 0 )) \
     || [[ "$out" == *"MCP error"* ]] \
     || [[ "$out" == *"forbidden:"* ]] \
     || [[ "$out" == *"Authorization required"* ]] \
     || [[ "$out" == "[mcporter]"* ]] \
     || [[ "$raw" == *"isError: true"* ]] \
     || [[ "$raw" == *"\"isError\": true"* ]]; then
    is_error=1
    error_msg=$(echo "$out" | head -c 200 | tr -d '\033' | sed 's/\[[0-9;]*m//g')
    # Try kind extraction across both raw formats (Node-inspect + real JSON).
    error_kind=$(echo "$raw" | grep -oE "kind: ?['\"][^'\"]+['\"]" | head -1 | sed -E "s/kind: ?['\"]//;s/['\"]$//" || true)
    if [[ -z "$error_kind" ]]; then
      error_kind=$(echo "$raw" | grep -oE 'MCP error -?[0-9]+: [a-z_]+' | head -1 | awk '{print $NF}' | tr -d ':' || true)
    fi
  fi

  # --- assertion dispatch ---
  if [[ -z "$assertion" ]]; then
    if (( is_error )); then
      echo "FAIL  (liveness) $error_msg"
      failures+=("$selector [liveness]: $error_msg"); ((++fail)); return
    fi
    echo "ok    [liveness]"; ((++pass)); return
  fi

  local kind="${assertion%%:*}" payload="${assertion#*:}"
  payload="${payload# }"   # strip one leading space

  case "$kind" in
    contains)
      if (( is_error )); then echo "FAIL  contains: got error: $error_msg"
        failures+=("$selector [contains]: error: $error_msg"); ((++fail))
      elif [[ "$out" == *"$payload"* ]]; then
        echo "ok    [contains: $payload]"; ((++pass))
      else
        echo "FAIL  contains: '$payload' not in output"
        failures+=("$selector [contains]: '$payload' missing"); ((++fail))
      fi
      ;;
    regex)
      if (( is_error )); then echo "FAIL  regex: got error: $error_msg"
        failures+=("$selector [regex]: error: $error_msg"); ((++fail))
      elif [[ "$out" =~ $payload ]]; then
        echo "ok    [regex: $payload]"; ((++pass))
      else
        echo "FAIL  regex: /$payload/ did not match"
        failures+=("$selector [regex]: /$payload/"); ((++fail))
      fi
      ;;
    jq)
      if (( is_error )); then echo "FAIL  jq: got error: $error_msg"
        failures+=("$selector [jq]: error: $error_msg"); ((++fail))
      elif echo "$out" | jq -e "$payload" >/dev/null 2>&1; then
        echo "ok    [jq: $payload]"; ((++pass))
      else
        echo "FAIL  jq: '$payload' falsy or response not JSON"
        failures+=("$selector [jq]: $payload"); ((++fail))
      fi
      ;;
    error)
      if (( ! is_error )); then
        echo "FAIL  error: expected '$payload', got success"
        failures+=("$selector [error]: expected $payload, got ok"); ((++fail))
      elif [[ -z "$error_kind" ]]; then
        # Couldn't extract kind; accept if error message mentions the payload word
        if [[ "$error_msg" == *"$payload"* ]]; then
          echo "ok    [error contains: $payload]"; ((++pass))
        else
          echo "FAIL  error: kind not extractable; msg: $error_msg"
          failures+=("$selector [error]: expected $payload, got $error_msg"); ((++fail))
        fi
      elif [[ "$error_kind" == "$payload" ]]; then
        echo "ok    [error.kind: $payload]"; ((++pass))
      else
        echo "FAIL  error: expected kind=$payload, got kind=$error_kind"
        failures+=("$selector [error]: kind $error_kind ≠ $payload"); ((++fail))
      fi
      ;;
    *)
      echo "FAIL  unknown assertion type: $kind (expected contains|regex|jq|error)"
      failures+=("$selector [bad-assertion]: $assertion"); ((++fail))
      ;;
  esac

  if (( fail > 0 )) && [[ "$VERBOSE" == "1" ]]; then
    echo "    --- raw output ---"
    echo "$out" | sed 's/^/    /'
  fi
}

for row in "${CASES[@]}"; do
  IFS='|' read -r label args assertion <<<"$row"
  run_one "$label" "$args" "$assertion"
done

echo "---"
printf "%d passed, %d failed\n" "$pass" "$fail"
if (( fail > 0 )); then
  printf '\nFailures:\n'; printf '  - %s\n' "${failures[@]}"
  exit 1
fi
