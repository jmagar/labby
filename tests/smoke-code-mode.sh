#!/usr/bin/env bash
# Smoke test for lab code mode: codemode discovery, callTool, and helpers.
# Usage: ./tests/smoke-code-mode.sh [server]   (default: lab)
#
# Prerequisites:
#   - mcporter configured with 'lab' server pointing to http://localhost:8765/mcp
#   - lab gateway running with code_mode.enabled=true

set -euo pipefail

MODE="run"
case "${1:-}" in
  --init)        MODE="init";       shift ;;
  --list-tools)  MODE="list-tools"; shift ;;
  --help|-h)     sed -n '2,10p' "$0"; exit 0 ;;
esac

SERVER=${1:-lab}
TIMEOUT_MS=${TIMEOUT_MS:-20000}
VERBOSE=${VERBOSE:-0}
NO_PREFLIGHT=${NO_PREFLIGHT:-1}   # codemode has complex nested schema; skip key preflight

# ---- cases -------------------------------------------------------------------
# Tests cover the { code } only contract — no action discriminator.
#   1. codemode(discovery) — in-sandbox codemode.search() returns compact hits
#   2. codemode(callTool) — direct callTool() broker call works end-to-end
#   3. codemode(helper) — snake_case namespace proxy: resolve_library_id is valid JS
#   4. codemode missing required field — MCP schema rejects empty object with invalid_param
declare -a CASES=(
  # Case 1: discovery is available through codemode.search().
  # Assert the compact result shape is present.
  "codemode|--args '{\"code\":\"async () => codemode.search({ query: \\\"context7\\\", limit: 3 })\"}' |contains: results"

  # Case 2: direct callTool() — verifies runner + broker loop.
  # Uses context7 resolve-library-id (available in the pool without OAuth).
  # The raw callTool path bypasses the snake_case preamble — tests broker wiring.
  # Assert 'context7.com' which appears in both success and quota-exceeded responses.
  "codemode|--args '{\"code\":\"async () => await callTool(\\\"context7::resolve-library-id\\\", {libraryName: \\\"react\\\", query: \\\"react hooks\\\"})\"}' |contains: context7.com"

  # Case 3: codemode proxy — validates the property-key quoting fix.
  # 'resolve-library-id' → resolve_library_id via tool_name_to_snake + serde_json::to_string.
  # Before the fix: QuickJS threw SyntaxError on the unquoted 'resolve-library-id:' key.
  # After the fix: the preamble uses \"resolve_library_id\": which is valid JS.
  # A response from context7 (even quota-exceeded) proves the snake_case proxy wired correctly.
  "codemode|--args '{\"code\":\"async () => await codemode.context7.resolve_library_id({libraryName: \\\"react\\\", query: \\\"react hooks\\\"})\"}' |contains: context7.com"

  # Case 4: missing required 'code' field → MCP schema validation error (invalid_param).
  # With the action discriminator removed, schema rejects {} as missing required field.
  "codemode|--args '{}' |error: invalid_param"
)
# ------------------------------------------------------------------------------

need() { command -v "$1" >/dev/null || { echo "missing dep: $1" >&2; exit 127; }; }
need jq

# Use mcporter via npx if not in PATH directly
if command -v mcporter >/dev/null 2>&1; then
  MCPORTER="mcporter"
else
  need npx
  MCPORTER="npx mcporter"
fi

SCHEMA_JSON=""
load_schema() {
  [[ -n "$SCHEMA_JSON" ]] && return 0
  SCHEMA_JSON=$($MCPORTER list "$SERVER" --schema --json 2>/dev/null) || {
    echo "warn: cannot load schema for $SERVER" >&2
    SCHEMA_JSON="{}"; return 1
  }
}

if [[ "$MODE" == "init" ]]; then
  load_schema
  echo "$SCHEMA_JSON" | jq -r '.tools // [] | .[].name'
  exit 0
fi

if [[ "$MODE" == "list-tools" ]]; then
  load_schema
  echo "$SCHEMA_JSON" | jq -r '.tools // [] | .[].name'
  exit 0
fi

# Server reachability check
status=$($MCPORTER list --json 2>/dev/null \
  | jq -r --arg s "$SERVER" '.servers[] | select(.name==$s) | .status' || true)
if [[ -z "$status" ]]; then
  echo "note: $SERVER not in mcporter config" >&2
elif [[ "$status" == "auth" ]]; then
  echo "$SERVER needs auth — run: $MCPORTER auth $SERVER" >&2; exit 2
elif [[ "$status" == "offline" || "$status" == "error" ]]; then
  echo "$SERVER is $status" >&2; exit 2
fi

(( ${#CASES[@]} == 0 )) && { echo "no cases defined" >&2; exit 2; }

pass=0; fail=0
failures=()

run_one() {
  local label="$1" args="$2" assertion="$3"
  local selector="$SERVER.$label"
  printf "→ %-55s " "$selector"

  local out rc=0 raw
  out=$(eval "$MCPORTER call '$selector' $args --output text --timeout $TIMEOUT_MS" 2>&1) || rc=$?

  local is_error=0 error_kind="" error_msg=""
  raw=$(eval "$MCPORTER call '$selector' $args --output raw --timeout $TIMEOUT_MS" 2>&1 || true)

  if (( rc != 0 )) \
     || [[ "$out" == *"MCP error"* ]] \
     || [[ "$out" == *"forbidden:"* ]] \
     || [[ "$out" == *"Authorization required"* ]] \
     || [[ "$out" == "[mcporter]"* ]] \
     || [[ "$raw" == *"isError: true"* ]] \
     || [[ "$raw" == *'"isError": true'* ]]; then
    is_error=1
    error_msg=$(echo "$out" | head -c 200 | tr -d '\033' | sed 's/\[[0-9;]*m//g')
    error_kind=$(echo "$raw" | grep -oE "kind: ?['\"][^'\"]+['\"]" | head -1 \
      | sed -E "s/kind: ?['\"]//;s/['\"]$//" || true)
    if [[ -z "$error_kind" ]]; then
      error_kind=$(echo "$out" | grep -oE '"kind"\s*:\s*"[^"]+"' | head -1 \
        | sed -E 's/"kind"\s*:\s*"//;s/"//' || true)
    fi
  fi

  if [[ -z "$assertion" ]]; then
    if (( is_error )); then
      echo "FAIL  (liveness) $error_msg"
      failures+=("$selector: $error_msg"); ((++fail)); return
    fi
    echo "ok    [liveness]"; ((++pass)); return
  fi

  local kind="${assertion%%:*}" payload="${assertion#*:}"
  payload="${payload# }"

  case "$kind" in
    contains)
      if (( is_error )); then
        echo "FAIL  contains: got error: $error_msg"
        failures+=("$selector [contains]: error: $error_msg"); ((++fail))
      elif [[ "$out" == *"$payload"* ]]; then
        echo "ok    [contains: ${payload:0:40}]"; ((++pass))
      else
        echo "FAIL  contains: '$payload' not found"
        [[ "$VERBOSE" == "1" ]] && echo "$out" | sed 's/^/    /'
        failures+=("$selector [contains]: '$payload' missing"); ((++fail))
      fi
      ;;
    regex)
      if (( is_error )); then
        echo "FAIL  regex: got error: $error_msg"
        failures+=("$selector [regex]: error: $error_msg"); ((++fail))
      elif [[ "$out" =~ $payload ]]; then
        echo "ok    [regex: $payload]"; ((++pass))
      else
        echo "FAIL  regex: /$payload/ no match"
        [[ "$VERBOSE" == "1" ]] && echo "$out" | sed 's/^/    /'
        failures+=("$selector [regex]: /$payload/"); ((++fail))
      fi
      ;;
    jq)
      if (( is_error )); then
        echo "FAIL  jq: got error: $error_msg"
        failures+=("$selector [jq]: error: $error_msg"); ((++fail))
      elif echo "$out" | jq -e "$payload" >/dev/null 2>&1; then
        echo "ok    [jq: $payload]"; ((++pass))
      else
        echo "FAIL  jq: '$payload' falsy"
        [[ "$VERBOSE" == "1" ]] && echo "$out" | sed 's/^/    /'
        failures+=("$selector [jq]: $payload"); ((++fail))
      fi
      ;;
    error)
      if (( ! is_error )); then
        echo "FAIL  error: expected '$payload', got success"
        failures+=("$selector [error]: expected $payload, got ok"); ((++fail))
      elif [[ -z "$error_kind" ]]; then
        if [[ "$error_msg" == *"$payload"* || "$out" == *"$payload"* ]]; then
          echo "ok    [error contains: $payload]"; ((++pass))
        else
          echo "FAIL  error: kind not extractable, msg: $error_msg"
          failures+=("$selector [error]: expected $payload, got: $error_msg"); ((++fail))
        fi
      elif [[ "$error_kind" == "$payload" ]]; then
        echo "ok    [error.kind: $payload]"; ((++pass))
      else
        echo "FAIL  error: expected kind=$payload, got kind=$error_kind"
        failures+=("$selector [error]: $error_kind ≠ $payload"); ((++fail))
      fi
      ;;
    *)
      echo "FAIL  unknown assertion: $kind"
      failures+=("$selector [bad-assertion]: $assertion"); ((++fail))
      ;;
  esac
}

echo "=== code mode smoke test ==="
echo "server: $SERVER"
echo ""

for row in "${CASES[@]}"; do
  IFS='|' read -r label args assertion <<<"$row"
  run_one "$label" "$args" "$assertion"
done

echo ""
echo "---"
printf "%d passed, %d failed\n" "$pass" "$fail"

if (( fail > 0 )); then
  echo ""
  echo "Failures:"
  printf '  - %s\n' "${failures[@]}"
  exit 1
fi
