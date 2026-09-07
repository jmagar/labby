# File Stash

File Stash is Labby's built-in service for personal- or Team-scoped arbitrary
files owned by an authenticated principal or one explicitly selected Team. Labby owns the local metadata and
blob lifecycle, so this capability meets the built-in-service exception. Depot
is not a dependency and an explicitly configured remote target never falls back
to File Stash.

This document is the normative v1 contract. File Stash is registered on Linux
and Android and is available through authenticated HTTP, generic service
dispatch, MCP resources, and the web UI. Unsupported platforms omit the service
rather than advertising handlers that cannot honor its filesystem contract.

## Boundary and non-goals

File Stash stores flat, principal-owned files. It is not the retired Agent
Artifact Manager. V1 has no components, revisions, workspaces, component kinds,
providers, push/pull, deployment targets, Marketplace forks, drift detection,
directory import/export, or implicit synchronization. The archived Stash docs
are historical evidence only and must not be used as an implementation pattern.

## Identity and isolation

Every operation consumes the middleware-derived canonical `VerifiedIdentity`
and asks the existing AccessStore to resolve its `PrincipalLink` to the durable
AccessStore `PrincipalId`. File Stash does not derive, hash, cache, or migrate a
parallel principal namespace. Browser sessions and propagated in-process or
remote MCP authorization use that same resolution. MCP must preserve caller
authorization through `resolve_caller_authorization`; it must never fall back to
trusted-local identity. Static bearer, Unix-peer, and trusted-local stdio
credentials work only when their stable local `PrincipalLink` is explicitly
mapped to an active service/bootstrap Principal in AccessStore. Missing,
ambiguous, inactive, or unavailable resolution fails closed before any filename,
object, grant, quota, or recipient lookup. Observability actor keys are not
authorization identities.

Personal is the compatibility default and retains the historical durable
principal key. Team operations require explicit `owner_kind=team` plus an
opaque Team ID (HTTP uses the corresponding `X-Labby-Owner-*` headers; browser
downloads use same-origin query parameters because links cannot attach
headers). Labby resolves current Team capability before lookup, and mutations
and opened downloads re-observe authority at their final boundary. Team members
may read Team files; Team administrators may manage them according to the fixed
role templates. Removing membership blocks new opens without affecting the
caller's personal files. Durable Team keys are type-prefixed, so no principal
or Team identifier can alias another owner's quota or objects.

Grant recipients are selected by a validated opaque AccessStore `PrincipalId`
from an authoritative, non-enumerating identity-selection surface, never by
email or display name. The owner and grantee IDs are in the same AccessStore
namespace. V1 rejects self-grants and duplicate active grants. Credential
rotation that keeps the same resolved Principal preserves access; a link change,
issuer change, or deleted/deactivated recipient denies access and never silently
retargets a grant.

## Object and filename contract

Each upload receives a random opaque file ID. The canonical reference is:

```text
stash://me/files/{opaque_file_id}
```

`me` is a fixed authorization-context authority label, not an ownership
namespace embedded in the URI. The opaque ID selects one object; each read then
authorizes the resolved caller as its owner or an active grantee. The exact same
URI therefore works for owner and grantee. It stays bound to that one object
until deletion; rename, deletion, and a later upload never reuse or retarget it.
Owner and shared files with equal display names remain unambiguous.

MCP clients select a Team-owned resource view per request with the
`ai.dinglebear.labby/stashOwner` request `_meta` object, for example
`{"kind":"team","id":"team-id"}`. Omitting it (or selecting
`{"kind":"personal"}`) uses the caller's Personal Stash. This is only a
scope selector: Labby resolves the verified identity and rechecks current Team
membership and read capability before listing or opening any resource.

The client filename is display metadata only and never becomes a storage path.
For multipart input, take only the final path component after splitting the raw
filename on both `/` and `\\`, in that order before Unicode normalization; this
turns browser values such as `C:\\fakepath\\report.pdf` into `report.pdf`.
Normalize that component to Unicode NFC, then reject an empty value, `.`/`..`,
controls, remaining separators, NUL, invalid UTF-8, or more than 255 UTF-8 bytes.
A principal cannot have two live owned files with the same normalized name.
Uploading a collision returns `conflict`; v1 never overwrites.

## Operations and metadata

V1 supports upload, list/search, metadata read, download, delete, grant create,
grant list, and grant revoke. Lists are cursor-paginated in stable
`created_at DESC, file_id DESC` order. Search is a bounded case-insensitive
substring match over normalized display names only. The stats response defines:

- `owned_file_count`: committed, non-deleted files owned by the caller;
- `owned_shared_file_count`: those owned files with at least one currently
  active, non-expired grant (one file counts once regardless of grant count);
- `owned_committed_bytes`: persisted bytes of committed, non-deleted owned files;
- `owned_reserved_bytes`: declared bytes held by the caller's pending uploads.

The mock's **Shared** summary card is `owned_shared_file_count`, not grant count
or files shared with the caller. All stats come from one authoritative snapshot,
not UI aggregation.

Delete is destructive and marks the object deleted while revoking every grant in
one metadata transaction. Delete and revoke prevent all new opens immediately.
Authorization is snapshot-on-open: a stream whose authorized regular-file handle
was already opened may finish, while later opens fail. Physical reclamation may
be asynchronous, but a deleted object never becomes newly readable. Upload and
grant creation mutate state but are not destructive. Shared action metadata is the only source for
`requires_admin` and `destructive`; surface adapters must not reclassify them.

Grants are explicit, read-only, and bind one file to one grantee AccessStore
`PrincipalId`. Revocation is effective for the next open, including an already
discovered URI; it does not abort a stream opened under an earlier valid
snapshot. V1 does not expire grants, mint bearer share links, or permit
re-sharing.

## Limits and backpressure

Defaults are intentionally conservative for a local operator service:

| Limit | Default | Configurable maximum |
| --- | ---: | ---: |
| file bytes | 100 MiB | 1 GiB |
| principal committed plus reserved bytes | 1 GiB | 100 GiB |
| instance committed plus reserved bytes | 10 GiB | 1 TiB |
| live files per principal | 1,000 | 100,000 |
| list page | 50 | 200 |
| search query | 128 UTF-8 bytes | 1,024 bytes |
| request-header bytes | 16 KiB | 64 KiB |
| grant recipients returned per page | 50 | 200 |
| MCP resource read | 10 MiB | 25 MiB |

The runtime also bounds concurrent uploads per principal (2) and instance (8),
instance downloads/disk reads (16), MCP resource reads (4), and database blocking
work (one 64-entry queue). Defaults may be lowered. Implementations must provide
bounded maxima for every exposed override and reject invalid startup config.

Uploads require exactly one valid `Content-Length`, reject any unsupported
`Transfer-Encoding`, and require absent or `identity` `Content-Encoding`.
Absent, malformed, duplicate, understated, overstated, or body-mismatched lengths
are rejected. The runtime reserves exactly the declared bytes transactionally,
checks the incremental byte count while reading, and compares the final persisted
byte count exactly to `Content-Length` before publication. Upload idle and total deadlines default to
30 seconds and 10 minutes. Pending reservations expire after 30 minutes. A
bounded janitor processes at most 100 expired items per pass with exponential
backoff capped at five minutes. Permit or queue saturation returns a retryable
`busy` error; quota exhaustion returns `quota_exceeded` and is not retried until
state or limits change.

## Storage, durability, and content safety

Metadata is durable SQLite state under Labby's configured state root. Blobs use
opaque storage names beneath a dedicated root. Startup validates the root and
every existing ancestor without following links, rejects symlinks/reparse points
and insecure ownership or permissions, and creates missing descendants with
owner-only permissions. All create/read/delete/reconcile operations are
descriptor-relative beneath the validated root and no-follow. They accept only
regular-file handles, create temporary and final blob names exclusively, and
reject links, reparse points, devices, sockets, FIFOs, or other special files at
every lifecycle stage.

SQLite and the filesystem cannot commit atomically. Upload therefore uses an
explicit crash-consistent state machine: (1) commit `pending` metadata and quota
reservation; (2) stream to an exclusively created temp file, verify the exact
length, fsync the file, exclusively publish the opaque blob, then fsync the blob
directory; (3) commit metadata as `committed`; and (4) release the reservation.
Only `committed` rows with a verified regular blob are readable. Restart recovery
reconciles each state idempotently: remove unreferenced temp/blob files, complete
or roll back expired pending rows, release orphaned reservations, and fail closed
on ambiguous or mismatched state. A database/blob mismatch is `integrity_error`,
never an empty or missing file.

Startup opens and validates the root and database, then performs crash-critical
pending-upload reconciliation in a background lifecycle task. Readiness and
every Stash operation fail closed with a recovering/unavailable state until
that pass finishes. Its cursor is durably checkpointed after each bounded batch,
so an interrupted process resumes without replaying completed entries. Once the
runtime becomes ready, committed-blob verification and orphan hygiene continue
as non-critical background work; opened blobs still verify their expected size.
Synchronous descriptor-relative filesystem batches run on the blocking pool.
A recovery error changes the runtime to a blocked state and prevents the janitor
from starting.

SQLite mutations retain one serialized transactional writer. Query-only work is
distributed across a bounded four-connection read pool, with shared queue and
deadline admission, so unrelated reads can overlap without weakening write
ordering or overload behavior.

File content is untrusted and plaintext at rest in v1. Operators must include
both database and blob directories in a consistent backup and restrict host
access; restore must preserve their shared generation. Encryption at rest,
malware scanning, content preview/rendering, deduplication, version history, and
remote replication are deferred. HTTP downloads use attachment disposition with
an escaped ASCII-safe quoted `filename` fallback plus an RFC 5987 `filename*`,
`X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, a Content Security
Policy of `default-src 'none'; sandbox`, private no-store caching, and a generic
binary media type. Header construction replaces control characters, quotes, and
path separators that cannot be represented safely. The web UI never executes,
frames, or previews uploaded content inline.

## Surfaces and errors

File Stash is default `gateway-host` functionality on supported platforms with
authenticated HTTP, generic service action metadata/tool dispatch, MCP resource
reads, and the web UI. It has no bespoke clap tree. Feature slices agree: a
surface is absent when its owning runtime is not compiled or available, rather
than advertising a handler that fails later.

The v1 durable runtime is available only where Labby can anchor SQLite and blob
operations to verified directory handles. Linux and Android satisfy that
contract. macOS, Windows, and other unsupported targets fail initialization
closed and must not register or advertise File Stash until a sanctioned
handle-relative implementation exists; the rest of `gateway-host` remains
available.

Every `/v1/stash/*` route remains behind the ordinary `/v1` authentication,
host/origin, and authorization middleware. Cookie-authenticated mutations also
require the shared CSRF validation. No loopback, multipart, download, or MCP
adapter bypass is permitted. HTTP streams large bodies without JSON/base64 wrapping. MCP resource reads apply
the lower MCP ceiling and return `quota_exceeded` when the object is too large;
clients use HTTP download for larger files. Stable agent error kinds are
`invalid_param`, `not_found`, `conflict`, `quota_exceeded`, `busy`,
`service_unavailable`, and `integrity_error`. Authorization failures and probes
of another principal's object or grant use the same non-enumerating `not_found`
shape. Authentication failures occur before service dispatch. Every error keeps
the shared agent error envelope and HTTP mapping; raw paths, identity material,
filenames, authorization values, and file content are excluded from logs.
