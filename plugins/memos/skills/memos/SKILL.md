---
name: memos
description: "This skill should be used when the user asks to save a note, create a memo, search memos, find notes about something, add a note, capture a thought, save something to their note hub, or mentions the Memos service. Does not apply when the user says 'remember this' without specifying Memos — that may route to the mnem memory system instead."
---

# Memos Skill

## Purpose

This skill provides **read-write** access to a self-hosted Memos instance for quick note capture, search, and organization. Memos is a privacy-focused, self-hosted note-taking service with Markdown support, tagging, and file attachments.

**Core capabilities:**
- Create, read, update, and delete memos (notes)
- Search memos by content, tags, or metadata
- Upload and manage file attachments
- Organize memos with tags
- Archive and visibility controls
- Link related memos together

**Primary use case:** Quick capture of important information from Claude conversations into a personal knowledge base.

## Setup

### Prerequisites
- Memos instance running and accessible
- API access token generated from Memos UI
- `curl` and `jq` installed

### Credential Configuration

Configure these values in plugin userConfig. The hook writes
`${XDG_CONFIG_HOME:-~/.config}/lab-memos/config.env` with mode `600`.
`~/.lab/.env` remains a fallback during migration:

```bash
# Memos - Self-hosted note-taking service
MEMOS_URL="https://memos.example.com"
MEMOS_API_TOKEN="<your_api_token>"
```

**To generate an API token:**
1. Log into your Memos instance
2. Go to Settings → Access Tokens
3. Click "Create" and copy the generated token
4. Add the token to plugin userConfig, or to `.env` as a local fallback

**Security:**
- Generated config and `.env` files are local-only (never commit)
- Set permissions: `chmod 600 ~/.lab/.env`
- Token has same permissions as your user account

## Commands

All commands return JSON output for LLM parsing. Scripts source credentials from
the generated plugin config automatically.

### Memo Operations

**Create a memo:**
```bash
bash scripts/memo-api.sh create "Your memo content here"
bash scripts/memo-api.sh create "Memo with tags" --tags "work,project"
bash scripts/memo-api.sh create "Private memo" --visibility PRIVATE
```

**List memos:**
```bash
bash scripts/memo-api.sh list
bash scripts/memo-api.sh list --limit 10
bash scripts/memo-api.sh list --filter 'tag == "work"'
```

**Get specific memo:**
```bash
bash scripts/memo-api.sh get <memo-id>
```

**Update memo:**
```bash
bash scripts/memo-api.sh update <memo-id> "Updated content"
bash scripts/memo-api.sh update <memo-id> --add-tags "urgent"
```

**Delete memo:**
```bash
bash scripts/memo-api.sh delete <memo-id>
```

**Archive memo:**
```bash
bash scripts/memo-api.sh archive <memo-id>
```

### Search Operations

**Search by content:**
```bash
bash scripts/search-api.sh "search query"
bash scripts/search-api.sh "docker kubernetes" --tags "devops"
bash scripts/search-api.sh "meeting notes" --from "2024-01-01"
```

**Search by tag:**
```bash
bash scripts/tag-api.sh list                    # List all tags
bash scripts/tag-api.sh search "project-x"      # Find memos with tag
```

### Resource (Attachment) Operations

**Upload file:**
```bash
bash scripts/resource-api.sh upload /path/to/file.pdf
bash scripts/resource-api.sh upload image.png --memo-id <id>
```

**List attachments:**
```bash
bash scripts/resource-api.sh list
bash scripts/resource-api.sh list --memo-id <id>
```

**Delete attachment:**
```bash
bash scripts/resource-api.sh delete <attachment-name>
```

### User Operations

**Get current user:**
```bash
bash scripts/user-api.sh whoami
```

**List access tokens:**
```bash
bash scripts/user-api.sh tokens
```

## Workflow

When the user asks about memos:

1. **"Save this to my memos"** → Extract key content, create memo with appropriate tags
2. **"What did I write about X?"** → Search memos by content/tags, present results
3. **"Find my notes on project Y"** → Use tag search or content filter
4. **"Update my memo about Z"** → Search for memo, get ID, update content
5. **"Delete that memo"** → Confirm with user, then delete by ID

## Notes

### API Details

- **Authentication:** Bearer token in `Authorization` header
- **Base URL:** `/api/v1` endpoint
- **Rate limits:** No documented limits (self-hosted)
- **Pagination:** Uses `pageSize` and `pageToken` parameters
- **Filtering:** Google AIP-160 standard (e.g., `tag == "work"`)

### Memo Format

Memos support full Markdown syntax:
- Headers, lists, code blocks
- Links and images
- Task lists (- [ ] and - [x])
- Tables

### Visibility Options

- `PRIVATE` - Only you can see
- `PROTECTED` - Authenticated users can see
- `PUBLIC` - Anyone can see (RSS feed)

### Best Practices

1. **Use descriptive content:** First line is preview in UI
2. **Tag consistently:** Use lowercase, hyphens for multi-word (e.g., "project-alpha")
3. **Archive old memos:** Keep workspace clean
4. **Link related memos:** Use memo relations for context

### Common Errors

For error diagnosis, see `references/troubleshooting.md`.

## Reference

Bundled references (load as needed):
- `references/api-endpoints.md` — API endpoint details
- `references/quick-reference.md` — command examples
- `references/troubleshooting.md` — common errors and fixes
- `examples/quick-capture.md`, `examples/tagging-workflow.md`, `examples/search-patterns.md` — worked examples

External:
- Official Docs: https://usememos.com/docs
- API Reference: https://usememos.com/docs/api

## Agent Tool Usage

Run this skill's scripts with the Bash tool directly:

```bash
./skills/memos/scripts/memo-api.sh [args]
```
