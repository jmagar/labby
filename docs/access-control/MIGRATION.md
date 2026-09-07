---
title: "Multi-user ownership migration and recovery"
created: "2026-09-05"
updated: "2026-09-05"
status: "design"
---

# Multi-user ownership migration and recovery

This runbook freezes the rehearsal and activation contract for moving an
existing Labby AccessStore v5 installation to multi-user ownership. It does not
introduce schema v6. The v5 rehearsal proves that the input can be inventoried,
backed up, reopened, restored, and deterministically classified before the
ownership migration is allowed to exist.

## Activation boundary

The point of no return is the first successful scoped write after multi-user
enforcement is enabled. Before that boundary, rollback restores the complete
pre-migration checkpoint. After it, rollback means forward repair or restoring
the complete checkpoint and losing all later writes; an old binary must never
open the new store and reinterpret scoped rows as globally owned.

Ordinary startup does not cross the v5/v6-to-v7 schema boundary implicitly.
After completing the pre-activation evidence below, the operator must set
`LABBY_ACCESS_MIGRATION_EVIDENCE` to the owner-controlled approval document
described in [ENV.md](../runtime/ENV.md). Labby binds that document to an
independent checkpoint, the exact source/target schema pair, a durable operation
ID, and an explicit activation decision before opening an exclusive migration
transaction. A prepared sidecar marker makes an interrupted attempt replay only
with the same evidence; successful commit publishes a complete marker.

The migration is split into separately observable phases:

1. `inventory`: open v5 read-only, run quick/FK/schema/bootstrap validation,
   and produce the signed inventory report;
2. `checkpoint`: stop writers, checkpoint WAL, create an SQLite-consistent
   backup, copy required sidecars/keys/configuration, and hash the restore set;
3. `expand`: create only the next-version structures in one exclusive
   transaction;
4. `classify`: attach exactly one typed owner to every durable resource;
5. `verify`: compare counts, stable IDs, logical content digests, references,
   quarantine counts, and audit/outbox facts;
6. `reopen`: close and reopen twice using the new binary, repeating integrity
   and inventory verification each time;
7. `shadow`: compare old and new authorization decisions without enforcing;
8. `activate`: persist the enforcement generation and emit its audit/outbox
   event in one transaction; and
9. `contract`: remove obsolete compatibility fields only in a later release.

Every phase has a durable operation ID and completion marker. Repeating a
completed phase verifies its recorded input/output digests and returns the
prior result. A changed replay fails closed.

## Production-shaped v5 inventory

The rehearsal fixture is non-empty and includes:

- the canonical bootstrap Organization, Principal, identity link, default
  Project, owner membership, and audit record when bootstrap generation is one;
- multiple active and inactive Principals with unique verified identity links;
- multiple Projects, all four Project roles, optional Loadout mappings, and
  audit rows;
- current credential/proof, tombstone, policy-publication, admission, and
  security tables, including empty-table counts where emptiness is meaningful;
- WAL mode with committed rows still represented through the WAL checkpoint
  path; and
- IDs and text containing realistic maximum-safe lengths and Unicode.

The report records the exact v5 application ID, user version, schema
fingerprint, bootstrap generation, global revision, table/index manifest
digest, table row counts, primary-key set digest per table, logical content
digest per table, `quick_check`, `foreign_key_check`, and source file/sidecar
digests. File-byte equality is not expected after a valid SQLite migration;
logical digests are canonical encodings ordered by primary key.

The current checked rehearsal validates only the schema and internal
consistency of a supplied evidence manifest. It does not execute a v4-to-v5 or
v5-to-v7 migration, reopen SQLite, or restore a checkpoint. Executable
migration and restore evidence remains an activation prerequisite.

## Ownership classification

The only automatic owner seed is the canonical verified bootstrap Principal.
Existing private user material becomes that Principal's Personal scope.
Installation configuration, host filesystem operations, raw logs, recovery,
and provider credentials become Installation-owned. No Team is invented, and
email, display name, namespace, directory name, creator string, or OAuth scope
is never used to infer a Team or PlatformAdmin.

Rows whose owner cannot be proven, whose identifiers collide after canonical
normalization, or whose references disagree are copied without mutation to a
PlatformAdmin-only quarantine ledger. The ledger records source store/table,
stable source key, safe digest, reason code, discovery phase, and resolution
state. It does not contain secret material. Quarantine is never included in
ordinary listing or compatibility-owner fallback.

Activation requires:

- every source row accounted for as classified or quarantined;
- zero duplicate target owner rows;
- zero dangling references or unclassified usable resources;
- unchanged stable IDs and logical content digests for non-policy payloads;
- exactly one PlatformAdministrator derived from the canonical bootstrap
  Principal; and
- independently reproducible before/after reports.

## Failure injection

Before production activation, a separate executable rehearsal must interrupt
every phase and inject the following failures. This proof is not provided by
`validate-multi-user-migration-rehearsal.py`:

- busy/locked, read-only, disk-full, I/O, corrupt/not-a-database, and truncated
  SQLite inputs;
- invalid application ID, user version, schema fingerprint/manifest,
  bootstrap shape, and foreign keys;
- process termination with non-empty WAL/SHM;
- duplicate IDs, canonical-name collisions, dangling owner candidates, and
  invalid UTF-8 at external inventory boundaries;
- missing/truncated backup members and mismatched restore-set generations; and
- failure to append audit/outbox/quarantine records.

Before activation, each failure leaves the source v5 store byte/logically
recoverable and the enforcement generation unchanged. Transactional failures
must leave the schema version and logical inventory unchanged. After restart,
the reconciler either resumes the exact operation or rejects changed inputs; it
does not skip ahead.

## Backup and restore

The operator blocks writers and records a maintenance lease before backup.
SQLite backup uses its online backup API or `VACUUM INTO` from a validated
connection after an explicit WAL checkpoint. Copying `access.db` alone while a
WAL may contain committed state is invalid.

The restore set contains:

- AccessStore database and required WAL/SHM state or a verified consolidated
  SQLite backup;
- schema and capability registry generations;
- signing private/public keys and active/overlap key generations;
- authority outbox head, Depot acknowledged watermark, and snapshot digest;
- bootstrap and enforcement generations; and
- configuration that selects standalone/managed authority mode.

Restore occurs into a new owner-only directory. The operator verifies every
manifest digest before atomically selecting it, starts without traffic, opens
and validates twice, verifies counts/digests/quarantine, and only then removes
the maintenance lease. A partial or cross-generation restore enters
recovery-required mode.

## Old-binary proof

The checkpoint retains an exact v5 database that the current v5 reader opens
and validates after restore. The future ownership database advertises a newer
user version and fingerprint; v5 must return `UnsupportedSchema` without
mutation. This is the rollback proof: restore the v5 checkpoint first, then run
the old binary. Never point the old binary at a post-activation database.

## Evidence retained

Release evidence includes source and target binary commits/digests, fixture
seed, phase operation IDs, start/end times, pre/post reports, checkpoint and
restore-set digests, injected-failure matrix, reopen results, quarantine
inventory, shadow-decision differences, and the activation audit/outbox event.
Secrets and raw identity assertions are excluded.
