#!/usr/bin/env bash
# check-oauth.sh — verify OAuth/auth configuration and endpoint security
#
# Usage:
#   ./scripts/check-oauth.sh [BASE_URL]
#
# Environment variables (override auto-detection from ~/.lab/.env):
#   LAB_BASE_URL          — server base URL (default: http://localhost:8080)
#   LAB_MCP_HTTP_TOKEN    — static bearer token (tested when present)
#   LAB_PUBLIC_URL        — expected public URL for OAuth issuer/audience
#   LAB_GOOGLE_CLIENT_ID  — required when running in OAuth mode
#
# Exit codes:
#   0  all checks passed
#   1  one or more checks failed
#   2  prerequisite missing (curl not found)

set -euo pipefail

BASE_URL="${1:-${LAB_BASE_URL:-http://localhost:8080}}"
BASE_URL="${BASE_URL%/}"

PASS=0
FAIL=0
WARN=0
SKIP=0

# ── colours (only when stdout is a TTY) ───────────────────────────────────────
if [ -t 1 ]; then
    GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'
    CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'
else
    GREEN=''; RED=''; YELLOW=''; CYAN=''; BOLD=''; RESET=''
fi

# ── helpers ───────────────────────────────────────────────────────────────────
header() { printf "\n${BOLD}${CYAN}▶ %s${RESET}\n" "$*"; }

pass()   { printf "  ${GREEN}✓${RESET} %s\n" "$*"; PASS=$((PASS+1)); }
fail()   { printf "  ${RED}✗${RESET} %s\n"   "$*"; FAIL=$((FAIL+1)); }
warn()   { printf "  ${YELLOW}!${RESET} %s\n" "$*"; WARN=$((WARN+1)); }
skip()   { printf "  ${YELLOW}·${RESET} %s (skipped)\n" "$*"; SKIP=$((SKIP+1)); }

# curl wrapper — returns HTTP status code, writes body to $BODY_FILE
BODY_FILE="$(mktemp)"
trap 'rm -f "$BODY_FILE"' EXIT

http() {
    local method="$1" url="$2"; shift 2
    curl -s -o "$BODY_FILE" -w '%{http_code}' -X "$method" \
        --max-time 5 --connect-timeout 3 \
        "$@" "$url" 2>/dev/null || echo "000"
}

body() { cat "$BODY_FILE"; }

json_field() {
    local field="$1"
    # simple jq-free extraction: works for top-level string/number fields
    grep -o "\"${field}\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" "$BODY_FILE" \
        | sed 's/.*: *"\([^"]*\)"/\1/' | head -1
}

# ── prerequisite ──────────────────────────────────────────────────────────────
if ! command -v curl &>/dev/null; then
    echo "ERROR: curl is required" >&2; exit 2
fi

printf "${BOLD}lab auth/OAuth verification${RESET}  %s\n" "$BASE_URL"

# ── 1. Config checks ──────────────────────────────────────────────────────────
header "Configuration"

# Load ~/.lab/.env if it exists and vars aren't already set
LAB_ENV="${HOME}/.lab/.env"
if [ -f "$LAB_ENV" ]; then
    # shellcheck disable=SC1090
    set -a; source "$LAB_ENV" 2>/dev/null || true; set +a
fi

HAS_STATIC_TOKEN=false
HAS_OAUTH=false

if [ -n "${LAB_MCP_HTTP_TOKEN:-}" ]; then
    pass "LAB_MCP_HTTP_TOKEN is set"
    HAS_STATIC_TOKEN=true
else
    warn "LAB_MCP_HTTP_TOKEN is not set — static bearer auth will not be tested"
fi

if [ -n "${LAB_PUBLIC_URL:-}" ]; then
    pass "LAB_PUBLIC_URL is set (${LAB_PUBLIC_URL})"
else
    warn "LAB_PUBLIC_URL is not set — JWT issuer validation and OAuth mode cannot be tested"
fi

if [ -n "${LAB_GOOGLE_CLIENT_ID:-}" ] && [ -n "${LAB_GOOGLE_CLIENT_SECRET:-}" ]; then
    pass "LAB_GOOGLE_CLIENT_ID + LAB_GOOGLE_CLIENT_SECRET are set"
    HAS_OAUTH=true
else
    warn "LAB_GOOGLE_CLIENT_ID / LAB_GOOGLE_CLIENT_SECRET not set — OAuth mode not configured"
fi

if [ -n "${LAB_WEB_UI_AUTH_DISABLED:-}" ] && [ "${LAB_WEB_UI_AUTH_DISABLED}" = "true" ]; then
    fail "LAB_WEB_UI_AUTH_DISABLED=true — /v1/* routes are UNPROTECTED (intentional only for static-asset dev)"
else
    pass "LAB_WEB_UI_AUTH_DISABLED is not set (protected mode)"
fi

# ── 2. Server reachability ────────────────────────────────────────────────────
header "Server reachability"

STATUS=$(http GET "$BASE_URL/health")
if [ "$STATUS" = "000" ]; then
    fail "Server is not reachable at $BASE_URL (connection refused or timeout)"
    printf "\n${RED}Cannot continue — server is not running or not reachable.${RESET}\n"
    exit 1
elif [ "$STATUS" = "200" ]; then
    pass "GET /health → 200 (no auth required)"
else
    fail "GET /health → $STATUS (expected 200)"
fi

STATUS=$(http GET "$BASE_URL/ready")
if [ "$STATUS" = "200" ]; then
    pass "GET /ready → 200 (no auth required)"
else
    fail "GET /ready → $STATUS (expected 200)"
fi

# ── 3. Protected endpoints reject unauthenticated requests ────────────────────
header "Protected endpoints — unauthenticated must get 401"

for path in \
    "/v1/extract/actions" \
    "/mcp" \
    "/v1/openapi.json" \
    "/v1/doctor"
do
    STATUS=$(http GET "$BASE_URL$path")
    if [ "$STATUS" = "401" ]; then
        KIND=$(json_field "kind")
        if [ "$KIND" = "auth_failed" ]; then
            pass "GET $path → 401 {kind:auth_failed}"
        else
            warn "GET $path → 401 but missing kind:auth_failed in body (got: $KIND)"
        fi
    elif [ "$STATUS" = "000" ]; then
        warn "GET $path → connection failed (route may not be mounted)"
    else
        fail "GET $path → $STATUS (expected 401 — route may be unprotected)"
    fi
done

# /v0.1/servers (mcpregistry feature, may not be compiled in)
STATUS=$(http GET "$BASE_URL/v0.1/servers")
if [ "$STATUS" = "401" ]; then
    pass "GET /v0.1/servers → 401 (mcpregistry endpoint is protected)"
elif [ "$STATUS" = "404" ]; then
    skip "GET /v0.1/servers → 404 (mcpregistry feature not compiled)"
else
    fail "GET /v0.1/servers → $STATUS (expected 401 or 404)"
fi

# ── 4. Static bearer token ────────────────────────────────────────────────────
if $HAS_STATIC_TOKEN; then
    header "Static bearer token authentication"

    STATUS=$(http GET "$BASE_URL/v1/extract/actions" \
        -H "Authorization: Bearer ${LAB_MCP_HTTP_TOKEN}")
    if [ "$STATUS" = "200" ]; then
        pass "GET /v1/extract/actions with Bearer → 200"
    else
        fail "GET /v1/extract/actions with Bearer → $STATUS (expected 200)"
    fi

    STATUS=$(http GET "$BASE_URL/v1/openapi.json" \
        -H "Authorization: Bearer ${LAB_MCP_HTTP_TOKEN}")
    if [ "$STATUS" = "200" ]; then
        pass "GET /v1/openapi.json with Bearer → 200"
    else
        fail "GET /v1/openapi.json with Bearer → $STATUS (expected 200)"
    fi

    # Wrong token must still get 401
    STATUS=$(http GET "$BASE_URL/v1/extract/actions" \
        -H "Authorization: Bearer wrong-token-intentionally-bad")
    if [ "$STATUS" = "401" ]; then
        pass "GET /v1/extract/actions with wrong Bearer → 401"
    else
        fail "GET /v1/extract/actions with wrong Bearer → $STATUS (expected 401)"
    fi

    # Token with extra whitespace should be rejected
    STATUS=$(http GET "$BASE_URL/v1/extract/actions" \
        -H "Authorization: Bearer ${LAB_MCP_HTTP_TOKEN} extra")
    if [ "$STATUS" = "401" ]; then
        pass "Malformed bearer (extra token) → 401"
    else
        fail "Malformed bearer (extra token) → $STATUS (expected 401)"
    fi
else
    skip "Static bearer token tests (LAB_MCP_HTTP_TOKEN not set)"
fi

# ── 5. MCP endpoint is bearer-only (no session cookies) ──────────────────────
header "MCP endpoint — bearer-only, no session cookies"

# A fake session cookie must not grant access
STATUS=$(http GET "$BASE_URL/mcp" \
    -H "Cookie: lab_session=fake-session-id-should-not-work")
if [ "$STATUS" = "401" ]; then
    pass "GET /mcp with fake session cookie → 401 (bearer-only enforced)"
else
    fail "GET /mcp with fake session cookie → $STATUS (expected 401 — session cookies must not work on /mcp)"
fi

if $HAS_STATIC_TOKEN; then
    STATUS=$(http GET "$BASE_URL/mcp" \
        -H "Authorization: Bearer ${LAB_MCP_HTTP_TOKEN}" \
        -H "Accept: text/event-stream,application/json")
    if [ "$STATUS" = "200" ] || [ "$STATUS" = "405" ]; then
        pass "GET /mcp with Bearer → $STATUS (MCP endpoint reachable)"
    elif [ "$STATUS" = "401" ]; then
        fail "GET /mcp with valid Bearer → 401 (MCP auth middleware broken)"
    else
        warn "GET /mcp with Bearer → $STATUS (non-standard response — check MCP transport)"
    fi
fi

# ── 6. OAuth public discovery endpoints ──────────────────────────────────────
header "OAuth public discovery endpoints"

if $HAS_OAUTH || [ -n "${LAB_PUBLIC_URL:-}" ]; then
    STATUS=$(http GET "$BASE_URL/.well-known/oauth-authorization-server")
    if [ "$STATUS" = "200" ]; then
        ISSUER=$(json_field "issuer")
        TOKEN_ENDPOINT=$(json_field "token_endpoint")
        if [ -n "$ISSUER" ] && [ -n "$TOKEN_ENDPOINT" ]; then
            pass "GET /.well-known/oauth-authorization-server → 200 (issuer: $ISSUER)"
        else
            warn "GET /.well-known/oauth-authorization-server → 200 but missing issuer/token_endpoint"
        fi
        # Validate issuer matches LAB_PUBLIC_URL
        if [ -n "${LAB_PUBLIC_URL:-}" ]; then
            EXPECTED_ISSUER="${LAB_PUBLIC_URL%/}"
            if [ "$ISSUER" = "$EXPECTED_ISSUER" ]; then
                pass "Issuer matches LAB_PUBLIC_URL"
            else
                fail "Issuer mismatch: got '$ISSUER' expected '$EXPECTED_ISSUER'"
            fi
        fi
    elif [ "$STATUS" = "404" ]; then
        warn "GET /.well-known/oauth-authorization-server → 404 (OAuth mode may not be active)"
    else
        fail "GET /.well-known/oauth-authorization-server → $STATUS (expected 200)"
    fi

    STATUS=$(http GET "$BASE_URL/.well-known/oauth-protected-resource")
    if [ "$STATUS" = "200" ]; then
        RESOURCE=$(json_field "resource")
        pass "GET /.well-known/oauth-protected-resource → 200 (resource: $RESOURCE)"
    elif [ "$STATUS" = "404" ]; then
        warn "GET /.well-known/oauth-protected-resource → 404 (OAuth mode may not be active)"
    else
        fail "GET /.well-known/oauth-protected-resource → $STATUS (expected 200)"
    fi

    STATUS=$(http GET "$BASE_URL/jwks")
    if [ "$STATUS" = "200" ]; then
        # JWKS must have a keys array
        if grep -q '"keys"' "$BODY_FILE"; then
            pass "GET /jwks → 200 (keys array present)"
        else
            fail "GET /jwks → 200 but no 'keys' array in response"
        fi
    elif [ "$STATUS" = "404" ]; then
        warn "GET /jwks → 404 (OAuth mode may not be active)"
    else
        fail "GET /jwks → $STATUS (expected 200)"
    fi

    # OAuth auth endpoint — must be reachable (will redirect/error without params, not 401/404)
    STATUS=$(http GET "$BASE_URL/authorize")
    if [ "$STATUS" = "200" ] || [ "$STATUS" = "302" ] || [ "$STATUS" = "400" ]; then
        pass "GET /authorize → $STATUS (OAuth authorization endpoint reachable)"
    elif [ "$STATUS" = "404" ]; then
        warn "GET /authorize → 404 (OAuth mode may not be active)"
    elif [ "$STATUS" = "401" ]; then
        fail "GET /authorize → 401 (OAuth endpoints must NOT require auth)"
    else
        fail "GET /authorize → $STATUS (unexpected)"
    fi

    STATUS=$(http GET "$BASE_URL/auth/login")
    if [ "$STATUS" = "200" ] || [ "$STATUS" = "302" ] || [ "$STATUS" = "400" ]; then
        pass "GET /auth/login → $STATUS (browser login endpoint reachable)"
    elif [ "$STATUS" = "404" ]; then
        warn "GET /auth/login → 404 (OAuth mode may not be active)"
    elif [ "$STATUS" = "401" ]; then
        fail "GET /auth/login → 401 (browser login must NOT require auth)"
    else
        fail "GET /auth/login → $STATUS (unexpected)"
    fi

    # Token endpoint — POST without a body should get 4xx, not 401/404
    STATUS=$(http POST "$BASE_URL/token" -H "Content-Type: application/x-www-form-urlencoded")
    if [ "$STATUS" = "400" ] || [ "$STATUS" = "422" ]; then
        pass "POST /token (no body) → $STATUS (token endpoint reachable, rejects bad request)"
    elif [ "$STATUS" = "404" ]; then
        warn "POST /token → 404 (OAuth mode may not be active)"
    elif [ "$STATUS" = "401" ]; then
        fail "POST /token → 401 (token endpoint must NOT require auth)"
    else
        fail "POST /token → $STATUS (unexpected)"
    fi
else
    skip "OAuth discovery endpoints (OAuth not configured — set LAB_PUBLIC_URL + Google credentials)"
fi

# ── 7. WWW-Authenticate header in OAuth mode ──────────────────────────────────
header "WWW-Authenticate header (OAuth mode)"

if $HAS_OAUTH || [ -n "${LAB_PUBLIC_URL:-}" ]; then
    # Unauthenticated request to a protected route in OAuth mode must include
    # WWW-Authenticate: Bearer resource_metadata=... so clients can discover the server
    STATUS=$(http GET "$BASE_URL/v1/extract/actions")
    if [ "$STATUS" = "401" ]; then
        WWW_AUTH=$(grep -i 'resource_metadata' "$BODY_FILE" 2>/dev/null || \
            curl -s -I --max-time 5 "$BASE_URL/v1/extract/actions" 2>/dev/null \
            | grep -i 'www-authenticate' || echo "")
        # Re-fetch to capture headers
        HEADERS=$(curl -s -I --max-time 5 "$BASE_URL/v1/extract/actions" 2>/dev/null)
        if echo "$HEADERS" | grep -qi 'www-authenticate.*resource_metadata'; then
            pass "401 response includes WWW-Authenticate: Bearer resource_metadata=... (RFC 9728)"
        else
            warn "401 response missing WWW-Authenticate header with resource_metadata (OAuth clients need this)"
        fi
    fi
else
    skip "WWW-Authenticate header check (OAuth not configured)"
fi

# ── 8. Dev marketplace endpoint ───────────────────────────────────────────────
header "Dev marketplace endpoint"

# Must be public
STATUS=$(http POST "$BASE_URL/dev/api/marketplace" \
    -H "Content-Type: application/json" \
    -d '{"action":"help"}')
if [ "$STATUS" = "200" ] || [ "$STATUS" = "400" ] || [ "$STATUS" = "422" ]; then
    pass "POST /dev/api/marketplace (read action, no auth) → $STATUS (not auth-blocked)"
elif [ "$STATUS" = "401" ]; then
    fail "POST /dev/api/marketplace → 401 (dev route must be unauthenticated)"
fi

# Must block mutating actions
STATUS=$(http POST "$BASE_URL/dev/api/marketplace" \
    -H "Content-Type: application/json" \
    -d '{"action":"plugin.install"}')
if [ "$STATUS" = "403" ]; then
    KIND=$(json_field "kind")
    if [ "$KIND" = "dev_preview_read_only" ]; then
        pass "POST /dev/api/marketplace (mutating action) → 403 {kind:dev_preview_read_only}"
    else
        warn "POST /dev/api/marketplace (mutating action) → 403 but unexpected kind: $KIND"
    fi
elif [ "$STATUS" = "401" ]; then
    fail "POST /dev/api/marketplace (mutating) → 401 (should be 403 — reached auth layer instead of dev guard)"
else
    fail "POST /dev/api/marketplace (mutating action) → $STATUS (expected 403)"
fi

# ── 9. Node self-registration endpoints (public) ──────────────────────────────
header "Node self-registration endpoints (public)"

for path in "/v1/nodes/hello" "/v1/fleet/hello"; do
    STATUS=$(http POST "$BASE_URL$path" \
        -H "Content-Type: application/json" \
        -d '{"hostname":"test-node","role":"worker"}')
    if [ "$STATUS" = "200" ] || [ "$STATUS" = "400" ] || [ "$STATUS" = "422" ]; then
        pass "POST $path (no auth) → $STATUS (not auth-blocked — public self-registration)"
    elif [ "$STATUS" = "401" ]; then
        fail "POST $path → 401 (node hello must be public)"
    elif [ "$STATUS" = "404" ]; then
        warn "POST $path → 404 (fleet feature may not be mounted)"
    else
        warn "POST $path → $STATUS"
    fi
done

# WebSocket upgrade endpoint — must be reachable (will fail on HTTP without Upgrade header)
for path in "/v1/nodes/ws" "/v1/fleet/ws"; do
    STATUS=$(http GET "$BASE_URL$path" -H "Accept: */*")
    if [ "$STATUS" = "426" ] || [ "$STATUS" = "400" ] || [ "$STATUS" = "101" ]; then
        pass "GET $path (no auth) → $STATUS (WebSocket endpoint reachable without auth)"
    elif [ "$STATUS" = "401" ]; then
        fail "GET $path → 401 (WebSocket upgrade endpoint must be public — internal auth happens after upgrade)"
    elif [ "$STATUS" = "404" ]; then
        warn "GET $path → 404 (fleet feature may not be mounted)"
    else
        warn "GET $path → $STATUS"
    fi
done

# ── 10. Upstream OAuth callback (public) ─────────────────────────────────────
header "Upstream OAuth browser callback (public)"

# Probe without required query params — must get 400/422 (missing params), not 401.
# Do NOT send a real state token; the handler validates it against the OAuth state
# store and returns auth_failed (which looks like 401) if it's not found.
STATUS=$(http GET "$BASE_URL/auth/upstream/callback")
if [ "$STATUS" = "400" ] || [ "$STATUS" = "422" ]; then
    pass "GET /auth/upstream/callback (no params) → $STATUS (route is public — rejected bad request, not auth)"
elif [ "$STATUS" = "404" ]; then
    warn "GET /auth/upstream/callback → 404 (route may not be mounted — gateway_manager may not be configured)"
elif [ "$STATUS" = "401" ]; then
    fail "GET /auth/upstream/callback → 401 (route is behind bearer auth — must be public for browser OAuth redirect)"
elif [ "$STATUS" = "000" ]; then
    warn "GET /auth/upstream/callback → connection failed"
else
    # Any non-401 means the route is reachable without auth (could be 500 if sqlite not configured, etc.)
    pass "GET /auth/upstream/callback (no params) → $STATUS (not auth-blocked)"
fi

# ── Summary ───────────────────────────────────────────────────────────────────
printf "\n${BOLD}────────────────────────────────────────────${RESET}\n"
printf "  ${GREEN}Passed: %d${RESET}  ${RED}Failed: %d${RESET}  ${YELLOW}Warned: %d${RESET}  Skipped: %d\n" \
    "$PASS" "$FAIL" "$WARN" "$SKIP"
printf "${BOLD}────────────────────────────────────────────${RESET}\n"

if [ "$FAIL" -gt 0 ]; then
    printf "\n${RED}${BOLD}FAIL${RESET} — %d check(s) failed. Review output above.\n" "$FAIL"
    exit 1
elif [ "$WARN" -gt 0 ]; then
    printf "\n${YELLOW}${BOLD}WARN${RESET} — all checks passed with %d warning(s).\n" "$WARN"
    exit 0
else
    printf "\n${GREEN}${BOLD}PASS${RESET} — all checks passed.\n"
    exit 0
fi
