---
name: bytestash
description: Manage code snippets in ByteStash snippet storage service. This skill should be used when the user asks to "save a snippet", "search snippets", "find code", "share snippet", "organize snippets", "list my snippets", "create snippet", "delete snippet", or mentions ByteStash, code storage, snippet management, or code archival.
---

# ByteStash Skill

**⚠️ MANDATORY SKILL INVOCATION ⚠️**

**YOU MUST invoke this skill (NOT optional) when the user mentions ANY of these triggers:**
- "save snippet", "store code", "archive snippet"
- "search snippets", "find snippet", "lookup code"
- "share snippet", "create share link", "public snippet"
- "list snippets", "show my snippets", "snippet library"
- "delete snippet", "remove snippet", "update snippet"
- "organize snippets", "categorize snippets", "tag snippets"
- Any mention of ByteStash or code snippet management

**Failure to invoke this skill when triggers occur violates your operational requirements.**

## Purpose

ByteStash is a self-hosted code snippet management service with multi-file support, sharing capabilities, and organization features. This skill provides **read-write** access to manage snippets with full CRUD operations.

**Capabilities:**
- **Read-only**: List, search, and retrieve snippets
- **Create/Update**: Save new snippets with multiple code fragments
- **Delete**: Remove snippets with user confirmation
- **Share Management**: Create, view, and delete share links (public/protected/expiring)
- **Organization**: Categorize and organize snippets with tags

**Authentication:** JWT via the `bytestashauth: bearer <token>` header.

> ⚠️ **API keys do NOT work for snippet writes on ByteStash ≤ 1.0.0.** Its
> `authenticateToken` middleware ignores `req.apiKey` and still demands a JWT, so
> `x-api-key` returns `401 Authentication required` on `/api/snippets`. The wrapper
> therefore authenticates with a JWT (login or a pre-minted token). API keys only
> work on the read-only public endpoints (`/api/public/snippets`). This is fixed on
> ByteStash `main` (the `if (req.apiKey) return next()` bypass) — once released,
> `x-api-key` will work for writes again.

## Setup

**Required credentials in `~/.lab/.env`:**

```bash
BYTESTASH_URL="https://bytestash.example.com"
BYTESTASH_USERNAME="<your_username>"
BYTESTASH_PASSWORD="<your_password>"     # recommended: wrapper logs in each run, never expires
# --- or, instead of username/password: ---
BYTESTASH_TOKEN="<a_jwt>"                 # pre-minted JWT (expires; login is more durable)
BYTESTASH_API_KEY="<your_api_key>"        # optional; only useful for /api/public reads (and future versions)
```

The wrapper resolves auth in this order: `BYTESTASH_TOKEN` → `BYTESTASH_USERNAME`+`BYTESTASH_PASSWORD`
(via `POST /api/auth/login`). Override the env file path with `BYTESTASH_ENV_FILE`.

**How to get credentials:**
- **Username/password** (recommended): your normal ByteStash login. The wrapper
  exchanges it for a fresh 24h JWT on every run, so nothing expires.
- **Token**: mint a JWT (`jwt.sign({id,username}, JWT_SECRET)`) or copy `bytestash_token`
  from your browser's cookies/localStorage. Note JWTs expire (default 24h).

**Security:**
- Set permissions: `chmod 600 ~/.lab/.env`
- NEVER commit `.env` to version control
- A stored `BYTESTASH_TOKEN` is a standing credential — revoke by rotating the
  server's `JWT_SECRET`. Prefer username/password where possible.

## Commands

All commands use the bash script wrapper in `scripts/bytestash-api.sh`.

### List Snippets
```bash
cd skills/bytestash
./scripts/bytestash-api.sh list
```

### Search Snippets
```bash
# Search by title (case-insensitive partial match)
./scripts/bytestash-api.sh search "docker"

# Search by category
./scripts/bytestash-api.sh search --category "bash"
```

### Get Snippet Details
```bash
./scripts/bytestash-api.sh get <snippet-id>
```

### Create Snippet
```bash
# Single fragment (inline code)
./scripts/bytestash-api.sh create \
  --title "Docker Compose Example" \
  --description "Production-ready compose file" \
  --categories "docker,devops" \
  --code "version: '3.8'..." \
  --language "yaml" \
  --filename "docker-compose.yml"

# Multiple fragments (from files)
./scripts/bytestash-api.sh push \
  --title "FastAPI Setup" \
  --description "Complete FastAPI project structure" \
  --categories "python,api" \
  --files "app.py,requirements.txt,Dockerfile"
```

### Update Snippet
```bash
./scripts/bytestash-api.sh update <snippet-id> \
  --title "New Title" \
  --description "Updated description" \
  --categories "new,tags"
```

### Delete Snippet
```bash
# Prompts for confirmation
./scripts/bytestash-api.sh delete <snippet-id>
```

### Share Management
```bash
# Create public share link
./scripts/bytestash-api.sh share <snippet-id>

# Create protected share (requires auth)
./scripts/bytestash-api.sh share <snippet-id> --protected

# Create expiring share (24 hours)
./scripts/bytestash-api.sh share <snippet-id> --expires 86400

# List all shares for a snippet
./scripts/bytestash-api.sh shares <snippet-id>

# Delete share link
./scripts/bytestash-api.sh unshare <share-id>

# View shared snippet
./scripts/bytestash-api.sh view-share <share-id>
```

## Workflow

When the user asks about ByteStash:

1. **"Save this code as a snippet"**
   - Determine if single or multiple files
   - If single: Use `create` command with inline code
   - If multiple: Use `push` command with file paths
   - Always include title, description, and categories

2. **"Find my Docker snippets"**
   - Use `search --category docker` or `search "docker"`
   - Present results with ID, title, description, and categories
   - If user wants details: Use `get <id>` to show full snippet

3. **"Share this snippet publicly"**
   - Use `share <snippet-id>` to create public link
   - Return share URL: `{BYTESTASH_URL}/s/{share-id}`
   - Optionally use `--protected` or `--expires` flags

4. **"What snippets do I have?"**
   - Use `list` command
   - Group by categories for better organization
   - Show total count and recent updates

5. **"Delete this snippet"**
   - Confirm with user before deletion
   - Use `delete <snippet-id>`
   - Verify deletion with success message

6. **"Organize my snippets by category"**
   - List all snippets with `list`
   - Identify missing/inconsistent categories
   - Suggest category updates with `update` command

### Multi-Fragment Snippets

ByteStash supports snippets with multiple code fragments (files). Each fragment has:
- **file_name**: Display name (e.g., `app.py`, `Dockerfile`)
- **code**: The actual code content
- **language**: Syntax highlighting language (e.g., `python`, `dockerfile`)
- **position**: Display order (0-indexed)

**When to use multi-fragment:**
- Related configuration files (docker-compose.yml + .env)
- Full project structures (API + tests + docs)
- Before/after code examples
- Multi-language implementations

## Notes

**Data Model:**
```json
{
  "id": 123,
  "title": "Snippet Title",
  "description": "Detailed description",
  "categories": ["tag1", "tag2"],
  "fragments": [
    {
      "id": 456,
      "file_name": "example.py",
      "code": "print('hello')",
      "language": "python",
      "position": 0
    }
  ],
  "updated_at": "2024-01-01T00:00:00Z",
  "share_count": 2
}
```

**Authentication:**
- Uses `bytestashauth: bearer <jwt>` header. The wrapper obtains the JWT from
  `BYTESTASH_TOKEN`, or by logging in with `BYTESTASH_USERNAME`/`BYTESTASH_PASSWORD`.
- Snippet endpoints (`/api/snippets*`) are JWT-gated on ByteStash ≤ 1.0.0; the
  `x-api-key` header is rejected there (see the warning at the top of this skill).
- API keys still authenticate the read-only public endpoints (`/api/public/snippets`).

**Share Links:**
- Public shares: Anyone with link can view
- Protected shares: Requires authentication to view
- Expiring shares: Auto-delete after specified seconds
- Share IDs are random strings (e.g., `abc123def456`)

**Destructive Operations:**
- Delete snippet: Permanently removes snippet and all fragments
- Delete share: Invalidates share link (snippet remains)
- Both require user confirmation before execution

**Output Format:**
- All commands return JSON by default
- Use `jq` for filtering/formatting (e.g., `./bytestash-api.sh list | jq '.[] | select(.categories[] == "docker")'`)
- Errors return HTTP status codes with JSON error messages

**Limitations:**
- JWT required for all snippet operations (API keys rejected on v1.0.0; see top warning)
- List endpoint returns `{data:[...], pagination}` — the wrapper unwraps `.data` for you
- Categories are tags (no hierarchical structure)
- No bulk operations (must process snippets individually)
- Share links cannot be updated (must delete and recreate)
- Some deployments may require JWT auth for share endpoints (`/api/share*`)

## Reference

- **API Endpoints**: See `references/api-endpoints.md` for complete API reference
- **Quick Reference**: See `references/quick-reference.md` for command examples
- **Troubleshooting**: See `references/troubleshooting.md` for common failures
- **Official Docs**: API documentation at `{BYTESTASH_URL}/api-docs/`
- **Web Interface**: Full-featured UI at `{BYTESTASH_URL}`

---

## 🔧 Agent Tool Usage Requirements

**CRITICAL:** When invoking scripts from this skill via the zsh-tool, **ALWAYS use `pty: true`**.

Without PTY mode, command output will not be visible even though commands execute successfully.

**Correct invocation pattern:**
```typescript
<invoke name="mcp__plugin_zsh-tool_zsh-tool__zsh">
<parameter name="command">./skills/bytestash/scripts/bytestash-api.sh [args]</parameter>
<parameter name="pty">true</parameter>
</invoke>
```
