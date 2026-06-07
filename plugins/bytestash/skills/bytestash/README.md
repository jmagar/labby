# ByteStash Skill

Manage code snippets in your self-hosted ByteStash instance through Claude Code.

## What It Does

- **List and Search**: Find snippets by title, category, or ID
- **Create Snippets**: Save single or multi-file code snippets
- **Update Snippets**: Modify titles, descriptions, and categories
- **Delete Snippets**: Remove snippets with confirmation
- **Share Management**: Create public, protected, or expiring share links
- **Organization**: Categorize and tag snippets for easy discovery

## Setup

### 1. Configure Login Credentials

ByteStash snippet writes require a JWT via the custom
`bytestashauth: bearer <token>` header on ByteStash <= 1.0.0. API keys are only
useful for public/read-only endpoints on that version. The durable setup is to
store username/password and let the wrapper log in for a fresh JWT each run.

### 2. Configure Credentials

Configure ByteStash credentials in plugin userConfig. The hook writes
`${XDG_CONFIG_HOME:-~/.config}/lab-bytestash/config.env` with mode `600`.
`~/.lab/.env` remains a fallback during migration:

```bash
# Optional local fallback:
nano ~/.lab/.env

BYTESTASH_URL="https://bytestash.example.com"
BYTESTASH_USERNAME="<your_username>"
BYTESTASH_PASSWORD="<your_password>"

# Optional fallback only:
BYTESTASH_TOKEN="<a_jwt>"
BYTESTASH_API_KEY="<api_key_for_public_reads_only>"

chmod 600 ~/.lab/.env
```

### 3. Verify Setup

Test the connection:

```bash
cd ~/workspace/lab/plugins/bytestash/skills/bytestash
./scripts/bytestash-api.sh list
```

You should see a JSON array of your snippets (or empty array `[]` if you have none).

## Usage Examples

### Basic Operations

```bash
# List all your snippets
./scripts/bytestash-api.sh list

# Search by title
./scripts/bytestash-api.sh search "docker"

# Search by category
./scripts/bytestash-api.sh search --category "python"

# Get specific snippet details
./scripts/bytestash-api.sh get 123
```

### Creating Snippets

**Single file snippet:**

```bash
./scripts/bytestash-api.sh create \
  --title "Docker Build Command" \
  --description "Standard Docker build with tags" \
  --categories "docker,devops" \
  --code "docker build -t myapp:latest ." \
  --language "bash" \
  --filename "build.sh"
```

**Multi-file snippet:**

```bash
# Create your files
echo 'FROM python:3.11-slim' > Dockerfile
echo 'fastapi==0.104.1' > requirements.txt

# Push as snippet
./scripts/bytestash-api.sh push \
  --title "Python API Starter" \
  --description "FastAPI project structure" \
  --categories "python,api,fastapi" \
  --files "Dockerfile,requirements.txt"

# Clean up
rm Dockerfile requirements.txt
```

### Updating Snippets

```bash
# Update title
./scripts/bytestash-api.sh update 123 --title "New Title"

# Update categories
./scripts/bytestash-api.sh update 123 --categories "docker,kubernetes"

# Update multiple fields
./scripts/bytestash-api.sh update 123 \
  --title "Updated Title" \
  --description "New description" \
  --categories "new,tags,here"
```

### Sharing Snippets

```bash
# Create public share link
./scripts/bytestash-api.sh share 123
# Returns share ID (e.g., "abc123")

# Create protected share (requires login to view)
./scripts/bytestash-api.sh share 123 --protected

# Create expiring share (auto-delete after 24 hours)
./scripts/bytestash-api.sh share 123 --expires 86400

# List all shares for a snippet
./scripts/bytestash-api.sh shares 123

# View shared snippet content
./scripts/bytestash-api.sh view-share abc123

# Delete share link
./scripts/bytestash-api.sh unshare abc123
```

Share URLs follow this format:
```
https://bytestash.example.com/s/{share-id}
```

### Deleting Snippets

```bash
# Delete with confirmation prompt
./scripts/bytestash-api.sh delete 123
# Prompts: "Are you sure you want to delete snippet 123? (y/N)"
```

## Workflow

### Typical Usage Pattern

1. **Save code while working:**
   ```bash
   # Quick save of current script
   ./scripts/bytestash-api.sh create \
     --title "Database Migration Script" \
     --categories "sql,postgres" \
     --code "$(cat migrate.sql)" \
     --language "sql" \
     --filename "migrate.sql"
   ```

2. **Find it later:**
   ```bash
   # Search by category
   ./scripts/bytestash-api.sh search --category "sql"

   # Get the full snippet
   ./scripts/bytestash-api.sh get 123 | jq -r '.fragments[0].code'
   ```

3. **Share with team:**
   ```bash
   # Create share link
   ./scripts/bytestash-api.sh share 123
   # Send link: https://bytestash.example.com/s/abc123
   ```

4. **Organize periodically:**
   ```bash
   # List uncategorized snippets
   ./scripts/bytestash-api.sh list | \
     jq '.[] | select(.categories | length == 0) | {id, title}'

   # Add categories
   ./scripts/bytestash-api.sh update 123 --categories "bash,utils"
   ```

## Advanced Usage

### Working with jq

All commands return JSON, making them easy to process with `jq`:

```bash
# Get snippet IDs and titles
./scripts/bytestash-api.sh list | jq '.[] | {id, title}'

# Extract code from specific snippet
./scripts/bytestash-api.sh get 123 | jq -r '.fragments[0].code' > output.py

# Count snippets by category
./scripts/bytestash-api.sh list | \
  jq -r '.[] | .categories[]?' | \
  sort | uniq -c | sort -rn

# Find snippets updated in last 7 days
./scripts/bytestash-api.sh list | \
  jq --arg date "$(date -d '7 days ago' --iso-8601)" \
     '.[] | select(.updated_at > $date)'
```

### Bulk Operations

```bash
# Add tag to all Docker snippets
./scripts/bytestash-api.sh list | \
  jq -r '.[] | select(.categories[]? == "docker") | .id' | \
  while read -r id; do
    ./scripts/bytestash-api.sh update "$id" --categories "docker,verified"
  done

# Export all Python snippets
./scripts/bytestash-api.sh search --category "python" > python-snippets.json
```

## Troubleshooting

### Authentication Issues

**Problem:** "401 Unauthorized" or "Authentication required"

**Solutions:**
1. Verify login credentials are present without printing secrets:
   `grep '^BYTESTASH_\\(URL\\|USERNAME\\|PASSWORD\\|TOKEN\\)=' ~/.lab/.env`
2. Prefer `BYTESTASH_USERNAME` + `BYTESTASH_PASSWORD`; the wrapper logs in for
   a fresh JWT.
3. If using `BYTESTASH_TOKEN`, make sure it is an unexpired JWT.
4. Do not use API keys for snippet CRUD on ByteStash <= 1.0.0.

### Connection Issues

**Problem:** "Connection refused" or timeout errors

**Solutions:**
1. Verify ByteStash is accessible: `curl https://bytestash.example.com`
2. Check URL in .env matches your instance
3. Verify network connectivity
4. Check if service is behind VPN/firewall

### Script Not Found

**Problem:** "command not found" when running scripts

**Solutions:**
1. Ensure you're in the right directory:
   `cd ~/workspace/lab/plugins/bytestash/skills/bytestash`
2. Make script executable: `chmod +x scripts/bytestash-api.sh`
3. Use relative path: `./scripts/bytestash-api.sh list`

### Invalid JSON Errors

**Problem:** "parse error" from jq or "invalid JSON" errors

**Solutions:**
1. Check API response: `./scripts/bytestash-api.sh list | cat`
2. Verify credentials are correct
3. Check for API errors: `./scripts/bytestash-api.sh list | jq -e '.error'`

## Notes

### Data Structure

Snippets support multiple code fragments (files):

```json
{
  "id": 123,
  "title": "Example",
  "description": "Multi-file example",
  "categories": ["python", "api"],
  "fragments": [
    {
      "id": 456,
      "file_name": "app.py",
      "code": "from fastapi import FastAPI...",
      "language": "python",
      "position": 0
    },
    {
      "id": 457,
      "file_name": "requirements.txt",
      "code": "fastapi==0.104.1\nuvicorn==0.24.0",
      "language": "text",
      "position": 1
    }
  ],
  "updated_at": "2024-01-01T00:00:00Z",
  "share_count": 2
}
```

### Security

- API keys are scoped to your user account only
- Share links can be:
  - **Public**: Anyone with link can view
  - **Protected**: Requires login to ByteStash
  - **Expiring**: Auto-delete after specified time
- Never commit `.env` file to version control
- Set strict permissions: `chmod 600 ~/.env`

### Limitations

- No bulk API operations (must loop for multiple snippets)
- Categories are flat tags (no hierarchy)
- Share links cannot be updated (must delete and recreate)
- Language detection based on file extension only

## Reference

- **API Documentation**: See `references/api-endpoints.md` for complete API reference
- **Quick Reference**: See `references/quick-reference.md` for command examples
- **Official Docs**: https://bytestash.example.com/api-docs/
- **Web Interface**: https://bytestash.example.com

## Getting Help

- Check the quick reference for common operations
- Use `--help` flag: `./scripts/bytestash-api.sh --help`
- Review the API documentation for endpoint details
- Verify setup with simple `list` command first
