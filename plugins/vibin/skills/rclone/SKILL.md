---
name: rclone
description: Move, sync, mount, encrypt, and inspect files across cloud storage and remote hosts using the rclone CLI. Supports 50+ backends (Google Drive, S3, B2, Dropbox, OneDrive, SFTP, WebDAV, Mega, etc.) plus crypt overlays and HTTP/Web serving. Use whenever the user wants to copy files to/from a cloud remote, sync a local folder up to / down from cloud, list/move/delete files on a remote, mount a remote as a local filesystem, dedupe or check files, serve files over HTTP/WebDAV, browse what's configured (`rclone listremotes`), or run any rclone subcommand. Trigger phrases include "rclone", "sync to gdrive / s3 / b2 / onedrive", "copy this to my cloud", "mount the remote", "what's on my drive", "back up to S3", "list my buckets", "rclone config". Reads remotes from `~/.config/rclone/rclone.conf` by default. Destructive verbs (sync, delete, purge, move) require user confirmation per the safety boundaries below.
allowed-tools: Read, Bash
argument-hint: [remote:path or subcommand]
---

## Context

- Argument: $ARGUMENTS
- rclone: !`rclone version 2>/dev/null | head -1 || echo "not installed (apt install rclone or curl https://rclone.org/install.sh | bash)"`
- Configured remotes: !`rclone listremotes 2>/dev/null | tr '\n' ' ' || echo "(none — run rclone config)"`
- Config path: !`echo "${RCLONE_CONFIG:-$HOME/.config/rclone/rclone.conf}"`

# rclone

Cloud storage swiss army knife. Single Go binary, no daemon, config in one `rclone.conf` file. Same verbs across every backend — `rclone copy gdrive:foo s3:bucket/foo` works because both sides speak the same internal interface.

## The mental model

```
[local fs]  ⇄  rclone  ⇄  [any of 50+ backends]
                │
                ├─ Google Drive, S3, B2, Dropbox, OneDrive, Mega, …
                ├─ SFTP, WebDAV, FTP, HTTP
                ├─ Crypt (encrypted overlay on any other remote)
                ├─ Union (stack multiple remotes as one)
                └─ Combine (mount-point routing across remotes)
```

A "remote" is a named backend in `rclone.conf`. Paths are `remote:path/inside/remote`. Three special cases:

- `:local:` or just a local path → local filesystem (no remote prefix)
- `remote:` → the remote's root
- `remote:bucket-or-folder/...` → a path inside the remote

## Universal invocation

```bash
rclone <verb> <source> <dest> [flags]
```

All verbs take one or two paths. Flags are global (`--dry-run`, `--progress`, `--transfers N`, `--checkers N`, `--bwlimit 10M`) or verb-specific.

## Most-used verbs

### Inspection (auto-allowed, read-only)

| Verb | What it does |
|---|---|
| `rclone listremotes` | Names of every configured remote |
| `rclone lsd remote:` | Top-level directories |
| `rclone ls remote:path` | Recursive file list (size + path) |
| `rclone lsl remote:path` | With modtime |
| `rclone tree remote:path` | Tree view |
| `rclone size remote:path` | Total bytes + object count |
| `rclone about remote:` | Free/used quota (where the backend supports it) |
| `rclone hashsum md5 remote:path` | Per-file hashes (MD5/SHA1/SHA256/QuickXorHash) |
| `rclone cat remote:path/file` | Stream a remote file to stdout |
| `rclone version --check` | Local + latest published version |

### Transfer (copy = safe-additive; sync = destructive)

| Verb | What it does | Safety |
|---|---|---|
| `rclone copy SRC DST` | Copy new + changed. Never deletes. | **auto** |
| `rclone copyto SRC DST` | Copy a single file to a single file (no dir semantics) | **auto** |
| `rclone sync SRC DST` | Make DST match SRC. **Deletes extras in DST.** | **ask first** |
| `rclone bisync DIR1 DIR2` | Two-way sync with change-detection state file | **ask first** |
| `rclone move SRC DST` | Copy then delete from SRC | **ask first** |
| `rclone moveto SRC DST` | Single-file move | **ask first** |

**Always run a destructive verb with `--dry-run` first** and show the user the diff before re-running without it. The first 50 actions are usually enough to see whether the direction is right.

### Mutation (destructive)

| Verb | Effect | Safety |
|---|---|---|
| `rclone delete remote:path` | Delete every file matching filters under path | **ask first** |
| `rclone purge remote:path` | Delete dir + contents (faster than delete + rmdir) | **ask first** |
| `rclone deletefile remote:file` | One file | **ask first** |
| `rclone rmdirs remote:path` | Empty dirs only | **ask first** |
| `rclone cleanup remote:` | Purge trash on backends that support it (gdrive, b2) | **ask first** — irreversible |
| `rclone dedupe MODE remote:path` | Find duplicate-name files (gdrive can have these); MODE = `interactive\|skip\|first\|newest\|oldest\|largest\|smallest\|rename` | **ask first** |

### Mount / serve (state-changing on the local system)

| Verb | Effect | Safety |
|---|---|---|
| `rclone mount remote:path /mnt/x --daemon` | FUSE-mount the remote | **ask first** — pass `--read-only` unless the user wants writes |
| `rclone serve http remote:path --addr :8080` | Expose over HTTP | **ask first** — confirm bind addr + auth |
| `rclone serve webdav` / `sftp` / `nfs` / `restic` / `s3` | Serve a remote in another protocol | **ask first** |
| `rclone ncdu remote:path` | Interactive disk-usage TUI (read-only) | **auto** |

### Integrity

```bash
rclone check SRC DST                     # report differing files, no transfer
rclone check SRC DST --download          # rehash both sides (slower, definitive)
rclone cryptcheck CRYPT_REMOTE PLAIN     # validate a crypt remote
```

`check` is safe (read-only). It exits non-zero on mismatch — useful in scripts.

### Config

```bash
rclone config                            # interactive — usually run by the user
rclone config show [remote]              # dump (auto)
rclone config redacted [remote]          # dump with creds redacted (safer for logs)
rclone config dump                       # all remotes as JSON
rclone config create <name> <type> ...   # scriptable add — credentials in argv
rclone config password <name> field=value  # set/refresh an obscured field
```

Don't run interactive `rclone config` from inside Claude — it expects a TTY. If the user wants to add a remote, walk them through it or give them the `rclone config create` invocation to paste.

## Useful global flags

| Flag | When |
|---|---|
| `--dry-run` / `-n` | Always for the first run of any destructive verb |
| `--progress` / `-P` | Live transfer stats (don't pipe to grep — use it in foreground) |
| `--transfers N` | Parallel transfers (default 4; bump for many small files on fast links) |
| `--checkers N` | Parallel hash/listing checks (default 8) |
| `--bwlimit 10M` | Cap bandwidth (10M = 10 MB/s; supports schedule `8:00,512 12:00,off`) |
| `--tpslimit 4` | Cap API transactions per second (for rate-limited APIs like gdrive) |
| `--retries N` | Default 3; bump for flaky links |
| `--exclude-from FILE` / `--filter-from FILE` | Pattern-based filtering |
| `--max-age 7d` / `--min-age 30d` | Modtime windows |
| `--size-only` | Don't compare modtime/hash, just size (faster + risky) |
| `--immutable` | Refuse to modify existing dst files (defense against bad sync direction) |
| `--track-renames` | Detect rename + copy locally instead of re-uploading |
| `--log-file /tmp/r.log --log-level INFO` | Persistent log; `DEBUG` for support tickets |

## Patterns

### "Did this sync actually do the right thing?"

```bash
# 1. Dry run, capture
rclone sync ~/photos gdrive:photos --dry-run --log-file /tmp/dry.log --log-level INFO
grep -E 'NOTICE|ERROR|Would' /tmp/dry.log | head -50

# 2. If it looks right, re-run for real with --progress, and keep the log
rclone sync ~/photos gdrive:photos --progress --log-file /tmp/sync.log --log-level INFO

# 3. Verify
rclone check ~/photos gdrive:photos
```

### "What's on this remote?"

```bash
rclone tree remote:                                    # quick structure
rclone size remote:bucket                              # total
rclone ncdu remote:bucket                              # interactive (TUI)
rclone lsjson -R remote:bucket | jq '.[] | .Path'      # scriptable
```

### "Encrypted off-site backup"

```bash
# Assuming you've set up a crypt remote that wraps another:
# [b2-encrypted]
# type = crypt
# remote = b2-raw:my-bucket
# password = ...  filename_encryption = standard

rclone sync ~/Documents b2-encrypted:docs --progress --log-file /tmp/backup.log
```

Crypt remotes encrypt filenames + contents transparently — the underlying remote sees gibberish names. **Losing the crypt password means losing the data.** Confirm the password is backed up before relying on this.

### "Mount a remote read-only for browsing"

```bash
mkdir -p ~/mnt/gdrive
rclone mount gdrive: ~/mnt/gdrive --read-only --daemon --vfs-cache-mode minimal
# When done:
fusermount -u ~/mnt/gdrive
```

### "Move a file via remote without local disk"

```bash
rclone copyto src-remote:foo dest-remote:bar
```

Streams between remotes directly (server-side where supported, e.g. S3 → S3 in the same region).

## Safety boundaries

| Tier | Verbs |
|---|---|
| **Auto-allowed** | `listremotes`, `ls*`, `tree`, `size`, `about`, `cat`, `hashsum`, `check`, `cryptcheck`, `config show/redacted/dump`, `ncdu`, `copy` (additive only), `copyto`, `version` |
| **Ask first** | `sync`, `move`, `moveto`, `bisync`, `delete`, `deletefile`, `purge`, `rmdirs`, `cleanup`, `dedupe`, `mount`, `serve *`, `config create/update/password/delete` |
| **Refuse without extremely explicit instruction** | `sync` *with `--delete-during` or `--delete-before` against a remote the user hasn't named in the current ask*, `purge` on a top-level remote root, `config create` writing secrets into a file path other than the standard rclone.conf, `serve` bound to `0.0.0.0` without auth |

When the user says "back up X to Y", default to `rclone copy --dry-run` first, show them what would move, then ask before running for real. **Sync vs copy is the most common direction-of-arrow disaster** — confirm which side is the source.

## When NOT to use rclone

- **Single small file, local → local**: `cp` is simpler.
- **Tar/zip archives across hosts**: `scp`, `rsync`, or `tar | ssh` is more idiomatic.
- **One-shot HTTP downloads**: `curl -O`.
- **Sharing a public read-only URL**: backend-native tooling (`aws s3 presign`, `gsutil signurl`) is more capable.

rclone shines when (a) the source or dest is a cloud backend, (b) you want one verb that works across N backends, or (c) you need crypt/union/dedupe/mount features no single backend provides.

## References

- `references/backends.md` — common backend configs (gdrive, s3, b2, sftp, crypt) with example `rclone config create` invocations
- `references/filters.md` — `--filter` / `--include` / `--exclude` pattern syntax with worked examples
