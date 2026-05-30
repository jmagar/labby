# rclone backends — common configs

`rclone config` walks you through each backend interactively. This file shows the underlying `rclone.conf` shape and the `rclone config create` scriptable form for the most common backends.

Configs live at `~/.config/rclone/rclone.conf` by default (override with `$RCLONE_CONFIG`).

## Google Drive

```ini
[gdrive]
type = drive
client_id =                       # optional; use your own for higher quotas
client_secret =
scope = drive                     # drive | drive.readonly | drive.file (app-folder only)
token = {"access_token":"..."}    # populated by OAuth flow
team_drive =                      # for shared drives
```

OAuth flow is interactive — use `rclone config` to populate the `token` field. After that, no interactive step is needed.

```bash
# Listing what's at the root of My Drive
rclone lsd gdrive:

# A shared drive (Team Drive)
rclone lsd gdrive,team_drive=0AKxxxxxxxxx:
```

## Amazon S3 (and S3-compatible)

```ini
[s3-mine]
type = s3
provider = AWS                    # also: Wasabi, Backblaze, MinIO, Cloudflare, DigitalOcean, etc.
env_auth = true                   # use ~/.aws/credentials or env
region = us-east-1
storage_class = STANDARD
```

Scriptable create with explicit creds:

```bash
rclone config create s3-mine s3 \
  provider=AWS access_key_id=AKIA... secret_access_key=... region=us-east-1
```

For MinIO / Cloudflare R2 / DigitalOcean Spaces, set `provider=Minio|Cloudflare|DigitalOcean` and `endpoint=https://...`.

## Backblaze B2

```ini
[b2]
type = b2
account = 0012abcd...
key = K001...
hard_delete = false               # true = actually purge old versions on delete
```

```bash
rclone config create b2 b2 account=0012abcd key=K001...
```

## OneDrive (personal / business)

```ini
[onedrive]
type = onedrive
token = {"access_token":"..."}
drive_id = b!xxx
drive_type = personal             # personal | business | documentLibrary
```

OAuth interactive setup required. For SharePoint document libraries, use `drive_type=documentLibrary` + the library's `drive_id`.

## Dropbox

```ini
[dropbox]
type = dropbox
token = {"access_token":"..."}
```

Interactive OAuth via `rclone config`.

## SFTP

```ini
[mybox]
type = sftp
host = mybox.lan
user = jmagar
port = 22
key_file = ~/.ssh/id_ed25519
shell_type = unix                 # or none / cygwin / powershell
md5sum_command = none             # set "md5sum" if available for faster integrity
```

Scriptable:

```bash
rclone config create mybox sftp \
  host=mybox.lan user=jmagar key_file=~/.ssh/id_ed25519
```

`rclone` can use SFTP remotes for anything other backends do — including `mount`, `serve`, and crypt overlays.

## WebDAV (Nextcloud, ownCloud, Apache)

```ini
[nextcloud]
type = webdav
url = https://cloud.example.com/remote.php/dav/files/jmagar/
vendor = nextcloud                # nextcloud | owncloud | sharepoint | other
user = jmagar
pass = <obscured>                 # set via: rclone obscure 'plaintext'
```

```bash
rclone config create nextcloud webdav \
  url=https://cloud.example.com/remote.php/dav/files/jmagar/ \
  vendor=nextcloud user=jmagar pass="$(rclone obscure 'plaintext')"
```

## HTTP (read-only static sites)

```ini
[mirror]
type = http
url = https://mirror.example.com/pub/
```

Read-only — use for crawling published file trees.

## Crypt (encrypted overlay)

A crypt remote wraps another remote and transparently encrypts filenames + contents. The underlying remote sees only gibberish.

```ini
[b2-encrypted]
type = crypt
remote = b2:my-bucket/encrypted          # underlying remote + path
password = <obscured>                    # rclone obscure 'master-password'
password2 = <obscured>                   # optional salt
filename_encryption = standard           # off | standard | obfuscate
directory_name_encryption = true
```

```bash
rclone config create b2-encrypted crypt \
  remote=b2:my-bucket/encrypted \
  password="$(rclone obscure 'master-pw')" \
  filename_encryption=standard
```

**The crypt password is the only key.** Losing it = losing the data. Always back it up out-of-band before putting anything important in a crypt remote.

## Union (stack multiple remotes as one)

```ini
[combined]
type = union
upstreams = local:/data:ro gdrive::nc b2-encrypted::nc
# action_policy and create_policy control where writes go
action_policy = epff             # existing path, first found
create_policy = ff               # first found
```

Use cases: hot/cold tiering, presenting multiple cloud accounts as one tree, fallback reads.

## Useful misc commands

```bash
rclone obscure 'plaintext-password'                   # produce obscured form
rclone config password <remote> <field>=<plaintext>   # update a single field
rclone config redacted [<remote>]                     # dump conf without secrets
rclone config disconnect <remote>                     # invalidate OAuth token
rclone config reconnect <remote>                      # refresh OAuth token
```

## Where to find backend-specific docs

`rclone help backends` → list. `rclone help backend <name>` → detailed flags. The official docs at `https://rclone.org/<backend>/` have setup walkthroughs for every supported provider.
