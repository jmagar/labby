---
title: "Durable-State Disaster Recovery"
created: "2026-09-04"
updated: "2026-09-04"
---

# Durable-State Disaster Recovery

Labby owns an offline disaster-recovery workflow for the complete selected
installation root and configured external authentication files. Stop the Labby
daemon before using it; each operation acquires the same exclusive
`lifecycle.lock` as the daemon and fails if the daemon is still running.

```bash
labby state export --output /secure/staging/labby-state-2026-09-04
labby state verify --bundle /secure/staging/labby-state-2026-09-04
labby state restore --bundle /secure/staging/labby-state-2026-09-04
```

All three commands require `LABBY_RECOVERY_KEY_PATH` to name a separately
stored, owner-only file containing at least 32 random bytes. Keep that key
outside both `LABBY_HOME` and every backup bundle. Export authenticates the
exact manifest with HMAC-SHA256; verify and restore fail closed if the key is
missing, insecure, wrong, or the manifest was modified. Losing the key makes
the bundle unverifiable and unrestorable. Rotate it by retaining the old key
for its existing retention window and using the new key for new exports; never
overwrite the only key that authenticates retained backups.

The backup bundle itself must also be stored outside `LABBY_HOME`. Export,
verify, and restore resolve existing path ancestors and reject both direct and
symbolic-link aliases into the installation root. Use a separate local volume
or secured staging directory before copying the bundle to off-host storage.

An export is a versioned directory containing `manifest.json` and private
payload files. Manifest version 1 records the producing Labby version, exact
installation root, original absolute path, file mode, size, and SHA-256 digest
for every file. Export walks the entire installation root, including config,
dotenv secrets, access/auth databases, snippets, gateway state, and other
future state without relying on a maintained filename allowlist. It excludes
only `lifecycle.lock`. Absolute `LABBY_AUTH_SQLITE_PATH` and
`LABBY_AUTH_KEY_PATH` files selected by config, dotenv, or process environment
are included when they live outside the installation root.

Export and restore reject symbolic-link path components, hard-linked or
non-regular files, unsafe manifest paths, destination topology collisions,
unconfigured external destinations, group/world-writable source parts, digest
or size drift, an unsupported manifest schema, a different installation root,
or a bundle from a newer or incompatible Labby version. Bundle directories are
created owner-only and payload/manifest files are created mode `0600`. Restore
verifies the complete bundle before changing live state, stages replacement
files beside their destinations, atomically renames each file, removes
post-backup installation-root files, and retains adjacent rollback copies until
the transaction completes. A failure restores already-changed paths in reverse
order before the lifecycle lock is released.

The bundle contains plaintext credentials and signing material. The built-in
workflow provides authenticated integrity and filesystem permissions, not encryption. Encrypt
the verified bundle before it leaves the host using the organization's managed
backup encryption system; keep encryption keys outside the backup and test key
recovery separately. Maintain at least one encrypted, access-controlled copy in
a different failure domain. Apply retention appropriate to credential rotation
and legal requirements, and securely expire obsolete copies.

On supported platforms, an export includes both File Stash metadata and blobs
when its root remains beneath `LABBY_HOME`. Export rejects an externally
configured File Stash root because copying only the database or only the blob
tree would not be a consistent backup. After restore, prove runtime readiness
and byte integrity through an authenticated metadata lookup plus HTTP download;
successful archive extraction alone is not File Stash recovery proof.

RPO and RTO are operator policies, not implicit product promises. Choose an
export schedule whose maximum interval meets the required recovery point
objective; also export after credential, access-policy, gateway, or snippet
changes that cannot wait for the next schedule. Measure the recovery time
objective with periodic offline drills: stop the service, verify and restore a
copy on an isolated host with the same absolute installation root, start Labby,
and prove readiness plus an authenticated durable-state action. Record bundle
identity, verification result, elapsed restore time, and application-level
proof without recording secrets.

Durable-state recovery currently fails closed on Windows because owner-only ACL
creation and verification are not yet implemented for this workflow. Use the
supported Unix host/container recovery path; do not copy a plaintext bundle to
Windows as a workaround.
