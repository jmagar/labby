# rclone filter syntax

rclone has its own pattern language for include/exclude — it's not glob, not regex, and not gitignore exactly. Most surprises come from getting this wrong.

## Pattern primer

| Pattern | Meaning |
|---|---|
| `*.tmp` | Any single path component ending in `.tmp` |
| `**.tmp` | Any path containing a component ending in `.tmp` |
| `dir/**` | Everything under `dir/` at any depth |
| `dir/*` | Everything directly inside `dir/`, one level deep |
| `[abc]*.log` | Files starting with a/b/c and ending in .log |
| `{foo,bar}/**` | Everything under foo/ or bar/ |
| `/.git/**` | Anchored at root (leading `/`) |
| `**/.git/**` | `.git` anywhere in the tree |

**Trailing slash matters**: `dir/` matches the directory itself; `dir/**` matches its contents.

## Flag forms

| Form | Use when |
|---|---|
| `--include 'pattern'` / `-i` | Whitelist (only matches transfer) |
| `--exclude 'pattern'` | Blacklist (matches skip) |
| `--filter '+ pattern'` / `'- pattern'` | Mixed include/exclude with order |
| `--include-from FILE` / `--exclude-from FILE` / `--filter-from FILE` | Patterns one per line; `;` and `#` for comments |
| `--files-from FILE` | Exact path list — bypasses pattern matching entirely |

Filters are evaluated **in order**. First matching `+` or `-` wins. An implicit `- **` is appended unless any `--include` was used (which flips the default).

## Size / time / type filters

| Flag | Effect |
|---|---|
| `--min-size 1M` / `--max-size 100M` | Bound by size |
| `--min-age 7d` / `--max-age 30d` | Bound by modtime (d, h, m, s, w, M, y) |
| `--exclude-if-present .nobackup` | Skip a dir if it contains the named file |

## Examples

### "Copy only photos, skip metadata"

```bash
rclone copy ~/Pictures gdrive:Pictures \
  --include '*.{jpg,jpeg,png,heic,raw,nef,cr2}' \
  --exclude '.DS_Store' --exclude 'Thumbs.db' \
  --progress
```

### "Back up everything except node_modules and .git"

```bash
cat > /tmp/rclone-filters <<'F'
- node_modules/**
- **/.git/**
- **/.cache/**
- **/__pycache__/**
- **/dist/**
- **/build/**
+ **
F
rclone sync ~/code b2:code-backup --filter-from /tmp/rclone-filters --dry-run
```

### "Files modified in the last week, larger than 1MB"

```bash
rclone copy gdrive:Downloads ~/recent \
  --max-age 7d --min-size 1M \
  --progress
```

### "Just these exact paths"

```bash
cat > /tmp/files <<'F'
docs/spec-v2.md
src/main.rs
README.md
F
rclone copy ~/project gdrive:project-snapshot --files-from /tmp/files
```

`--files-from` paths are relative to the source root and bypass pattern filtering entirely. Useful when the user gives you a literal list.

## Debugging filters

```bash
rclone ls SRC --include 'pattern' --dry-run
rclone lsf SRC --filter-from rules.txt --recursive | head
```

`rclone lsf` is the listing-with-filters command — fast, scriptable, exits 0 even if no matches.

## Gotchas

- **No leading `./`** — patterns are relative to the source root by default. `./dir/file` and `dir/file` are NOT equivalent in rclone filters.
- **Anchor with leading `/`** to match only at root: `/.git/**` vs `**/.git/**`.
- **Order matters with `--filter`**: `+ *.log` followed by `- *` keeps all logs; `- *` followed by `+ *.log` excludes everything (including logs, because `- *` matched first).
- **`*` doesn't cross `/`** (single-component wildcard). Use `**` to cross directories.
- **Glob brace expansion is rclone's own**, not shell — quote the pattern to keep the shell from eating it.
