# ByteStash Troubleshooting

Common issues and fixes for `skills/bytestash/scripts/bytestash-api.sh`.

## 401 "Authentication required" with a VALID API key (ByteStash ≤ 1.0.0)

### Symptom
`x-api-key` requests to `/api/snippets` return `401 {"error":"Authentication required"}`,
while a *garbage* key returns `401 {"error":"Invalid API key"}` (proving the key is
actually valid — it's just not being honored).

### Cause
ByteStash v1.0.0's route chain is `authenticateApiKey → authenticateToken`. A valid
API key sets `req.apiKey`, but v1.0.0's `authenticateToken` **does not** check
`req.apiKey` and still demands a JWT, so it falls through to "Authentication required".
(`main` adds `if (req.apiKey) return next()`, which fixes this — but it isn't in the
v1.0.0 release image.)

### Fix
Authenticate with a **JWT** via the `bytestashauth: bearer <token>` header (this is what
the wrapper now does). Provide it in `~/.lab/.env` as either:
- `BYTESTASH_USERNAME` + `BYTESTASH_PASSWORD` (wrapper calls `POST /api/auth/login` → fresh 24h JWT), or
- `BYTESTASH_TOKEN` (a pre-minted JWT; expires).

API keys still work on the **read-only** public endpoints (`GET /api/public/snippets`).

### Endpoint gotcha
The authed snippet routes are under `/api/snippets` (not `/api/v1/snippets`). `GET
/api/snippets` returns `{data:[...], pagination}`; the wrapper unwraps `.data`.

## DNS/Connectivity Errors

### Symptom
`curl: (6) Could not resolve host: bytestash.example.com`

### Checks
```bash
nslookup bytestash.example.com
curl -I https://bytestash.example.com
```

### Fixes
- Verify DNS/VPN/Tailscale connectivity to the host.
- Confirm `BYTESTASH_URL` in `.env` points to a reachable address.
- If using an internal domain, test with the service IP directly.

## Authentication Failures

### Symptom
`HTTP 401` or `HTTP 403`

### Checks
```bash
grep '^BYTESTASH_' ~/.lab/.env
```

### Fixes
- Regenerate API key in ByteStash Settings -> API Keys.
- Replace `BYTESTASH_API_KEY` in `.env`.
- Ensure no extra spaces or quote mismatches in `.env`.

## Empty or Invalid JSON Output

### Symptom
- `jq` parse errors
- blank output for list/get/search

### Checks
```bash
./scripts/bytestash-api.sh list
echo $?
```

### Fixes
- Run command without `jq` first to inspect raw API output.
- Check upstream reverse proxy/auth middleware response pages.
- Validate ByteStash service health in browser.

## Share API Issues

### Symptom
`share`, `shares`, `unshare`, or `view-share` fails with auth errors.

### Cause
Some ByteStash deployments require JWT auth for share endpoints instead of API keys.

### Fixes
- Confirm your instance share endpoint auth mode in `/api-docs`.
- If JWT-only, use web UI for share management or extend the script with login/JWT flow.

## Permission and Script Execution Errors

### Symptom
`Permission denied` when running script.

### Fix
```bash
chmod +x skills/bytestash/scripts/bytestash-api.sh
```
