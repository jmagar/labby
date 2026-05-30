# rclone

Drive [rclone](https://rclone.org/) — the "rsync for cloud storage" — from any Claude session. One CLI verb (`copy`, `sync`, `move`, `mount`, `serve`, …) against 50+ backends (Google Drive, S3, B2, Dropbox, OneDrive, SFTP, WebDAV, Mega, …), all reading from a single `rclone.conf`.

Unlike the `nircmd` / `sysinternals` skills, this one runs **locally on the Linux host where Claude lives** — no SSH wrapper. rclone is a portable Go binary.

## What it does

| Capability | One-line example |
|---|---|
| List configured remotes | `rclone listremotes` |
| Show what's on a remote | `rclone tree gdrive:` / `rclone size b2:bucket` |
| Stream a remote file to stdout | `rclone cat gdrive:notes.txt` |
| Copy local → cloud (additive) | `rclone copy ~/photos gdrive:photos -P` |
| Sync (destructive — deletes extras in dest) | `rclone sync ~/photos gdrive:photos --dry-run` then for-real |
| Move (copy + delete from src) | `rclone move gdrive:old/ b2:archive/` |
| Verify two trees match | `rclone check ~/photos gdrive:photos` |
| Mount cloud as local FS | `rclone mount gdrive: ~/mnt/gdrive --read-only --daemon` |
| Serve a remote over HTTP/WebDAV/SFTP | `rclone serve webdav gdrive: --addr :8080` |
| Encrypted overlay (crypt remote) | `rclone sync ~/Docs b2-encrypted:docs` |
| Dedupe Google Drive name collisions | `rclone dedupe newest gdrive:folder` |

## Install

```bash
# Debian/Ubuntu
sudo apt install rclone

# Anywhere
curl https://rclone.org/install.sh | sudo bash
```

Verify with `rclone version`. Config lives at `~/.config/rclone/rclone.conf` (override with `$RCLONE_CONFIG`).

## Setting up a remote

Run **`rclone config`** in a terminal (not via Claude — it's interactive). The wizard handles OAuth flows for cloud backends and writes the encrypted credentials into `rclone.conf` for you.

To script a remote add (e.g. for SFTP/HTTP backends without OAuth):

```bash
rclone config create mybox sftp host=mybox.lan user=jmagar key_file=~/.ssh/id_ed25519
```

## Safety model

Three tiers, enforced by the skill:

- **Auto-allowed** (read-only): every `ls*`, `tree`, `size`, `about`, `cat`, `check`, `cryptcheck`, `config show`, `ncdu`. Also `copy` (purely additive — never deletes).
- **Ask first**: `sync` (deletes extras in dest), `move`, `bisync`, every `delete`/`purge` verb, `mount`, `serve *`, `config create/update/password/delete`, `dedupe`.
- **Refuse without explicit instruction**: `sync` with explicit delete-mode flags against an unnamed remote, `purge` at remote root, `serve` bound to `0.0.0.0` without auth, writing credentials outside the standard config path.

The skill also nudges toward `--dry-run` on any destructive verb before doing it for real — sync-in-the-wrong-direction is the #1 way people lose data with rclone.

## Sibling skills

| Skill | When to prefer |
|---|---|
| `gh` CLI | GitHub-specific (releases, issues, PRs) |
| `scp` / `rsync` directly | Host-to-host over SSH only |
| **rclone (this)** | Cloud backends, multi-backend transfers, mount/serve, crypt |

## Files

```
rclone/
├── SKILL.md                              Agent instructions + verb catalog
├── README.md                             This file
└── references/
    ├── backends.md                       Common backend configs + scriptable creates
    └── filters.md                        --filter / --include / --exclude syntax
```
