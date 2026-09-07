---
title: "Multi-user authority contract"
created: "2026-09-05"
updated: "2026-09-05"
status: "design"
---

# Multi-user authority contract

This document freezes the v1 authority model shared by Labby and Depot. The
older documents in this directory describe the larger workspace vision; where
they conflict for the multi-user release, this contract is authoritative.

## Domain boundary

One installation hosts one Organization. A Team is a flat
`Group(kind = team)` inside that Organization. A Principal can belong to zero,
one, or many Teams. A Project is a distinct runtime scope and may be assigned
to one Team. Team membership does not itself grant Project authority: a Team
assignment carries an explicit Project role. A Principal may also have a
direct Project membership. The effective Project role is the maximum of the
active direct and Team-derived roles.

Platform administration is orthogonal to Organization, Team, and Project
roles. OAuth scopes are transport ceilings only. Neither `lab:admin`, an
allowlisted email, a loopback connection, nor a caller-supplied actor field
creates a domain role.

## Entity relationship model

```text
Installation 1---1 Organization
     |                 |
     |                 +---* PrincipalLink *---1 Principal
     |                 |                         |
     |                 +---* Team 1---* TeamMembership *---+
     |                 |       |
     |                 |       +---* TeamProjectAssignment *---1 Project
     |                 |                                      |
     |                 +----------------* ProjectMembership *--+
     |
     +---* PlatformAdministrator *---1 Principal

OwnerScope = Installation
           | Team(team_id)
           | Project(project_id)
           | Personal(principal_id)

Every durable resource ---1 OwnerScope
```

`Organization` is a tenant/account envelope, not an assignable resource owner
in v1. `Public` is publication state, never an `OwnerScope`. Public discovery
does not grant mutation or create an anonymous Labby principal.

## Roles

Roles expand to registered capabilities. Policy code checks capabilities, not
role names.

- `PlatformAdmin` has installation-wide authority across Labby and Depot,
  including setup, host configuration, provider credentials, raw logs,
  backup/restore, global policy, pairing, and recovery. It is explicit,
  auditable, and rare.
- `TeamOwner` has every Team capability and alone may transfer ownership or
  complete Team deletion. Every active Team has at least one owner.
- `TeamAdmin` manages ordinary Team policy, members, Projects, and Team-owned
  resources, but cannot remove the final owner, transfer ownership, or use
  installation controls.
- `TeamMember` can discover, create, read, and operate resources allowed by
  Team policy. It cannot manage membership or Team policy.
- `PersonalUser` can manage resources owned by its own `Personal` scope and
  discover explicitly public resources. It has no Team visibility without an
  active membership.
- Project `Owner`, `Admin`, `Member`, and `Viewer` remain Project-local. They
  never imply Team or platform authority.

A Principal can hold different roles in different Teams. A request evaluates
only the explicit active context; roles never float into another Team.

## Owner scopes and resource families

| Resource | Allowed owner scopes | Administrative ceiling |
| --- | --- | --- |
| Installation policy, setup, host filesystem, raw logs, provider credentials, backup/recovery | Installation | PlatformAdmin |
| Library Artifact and Bundle | Team, Project, Personal | owner-scope capability plus Depot policy |
| Project and Project membership | Team | TeamOwner or TeamAdmin; final owner rules apply |
| Gateway policy, Loadout, upstream exposure, runtime binding | Team, Project, Personal where supported | scope administrator; host/process settings remain PlatformAdmin |
| Stash object | Team, Project, Personal | scope administrator; exact landed principal-Stash contract is the integration base |
| Agent definition and credential reference | Team, Project, Personal | scope administrator; secret bytes remain in the secret owner |
| Task and Job | Team, Project, Personal | owner-scope operator; creator alone is not authority |
| Dev Container definition, instance, and lease | Team, Project, Personal | owner-scope operator; privileged host capabilities default deny |

Ownership is typed and immutable during ordinary update. Transfer is a
separate compare-and-set operation with source and destination authorization,
referential checks, epoch changes, and an audit event. Team deletion is a
reconciled saga: `active -> deletion_pending -> deleted`. No Team becomes
invisible while usable resources still reference it.

## Capability families

The machine-readable registry is
[`authority-matrix-v1.json`](authority-matrix-v1.json). It defines the stable
capability families and maps every registered Labby service to exactly one
resource family. Implementations may add finer action capabilities, but must
not broaden the role templates in that registry silently.

The evaluation order is:

1. authenticate to a canonical `VerifiedIdentity` and active Principal;
2. require the transport scope for the surface;
3. resolve an explicit typed context (`Installation`, `Team`, `Project`, or
   `Personal`); only a caller's own Personal context may be implicit;
4. resolve current memberships, assignments, role templates, and resource
   policy at one authority revision;
5. require the action capability and matching `OwnerScope`;
6. intersect downstream/provider policy, license, publication, and runtime
   constraints; and
7. reauthorize at the final resource boundary immediately before use.

Failure at any step is deny. Missing and unauthorized identifiers produce the
same external response. Rich explanations require a scoped explain capability.

## Authority epochs and leases

An authorization snapshot carries the full epoch vector:

```text
authority_schema_generation
installation_epoch
organization_epoch
principal_epoch
team_membership_epoch[]
team_policy_epoch
project_membership_epoch
project_policy_epoch
resource_policy_epoch
gateway_catalog_generation
depot_projection_watermark
credential_generation
session_generation
```

Only components relevant to the decision need values, but none may be replaced
with a caller-controlled timestamp. A mutation changes its affected epochs in
the same database transaction as its audit event.

An `AuthorityLease` is a short-lived, non-serializable runtime handle binding
Principal, action, method, resource identity, `OwnerScope`, epoch vector, and
expiry. It is not transferable between actions or resources. Catalogs,
cursors, confirmations, uploads, jobs, agents, Tasks, Dev Containers, Stash
handles, credentials, and retained Code Mode results validate the lease at
their next safe boundary. Long operations define safe boundaries explicitly
and stop before acquiring new external effects after revocation.

## Labby and Depot authority

Labby AccessStore is the sole command authority for Principals, Teams,
memberships, Projects, role templates, and assignments in a managed pair.
Depot is authoritative for its durable Library/Artifact resource policy and
enforces every Depot operation locally. ETS or in-process caches are
projections, never policy truth.

Depot has two mutually exclusive modes:

- `standalone`: Depot owns a local platform administrator, local Team policy,
  and its own recovery ceremony. Labby assertions are rejected.
- `labby_managed`: local Team mutation endpoints are disabled. Depot accepts
  only a paired Labby issuer, consumes its ordered authority projection, and
  applies signed delegated assertions against that projection.

Pairing is an explicit, locally confirmed transition. There is never more than
one active command authority. Unpairing cannot silently restore stale local
administration; it enters recovery-required mode until an audited local
ceremony establishes a new generation.

## Ordered authority projection

Each Labby authority mutation writes an append-only outbox event in the same
transaction. Events contain a schema version, installation/Organization IDs,
monotonic sequence, entity version, operation ID, typed payload or tombstone,
and safe audit correlation. Labby delivers them at least once.

Depot durably records inbox operation IDs and one contiguous watermark before
acknowledging. Duplicate events are no-ops. A gap, invalid signature, unknown
required schema, tenant mismatch, or backward entity version blocks Team
operations and requests bounded resynchronization. Resync is a signed snapshot
at a declared sequence followed by ordered tail events. Tombstones are retained
long enough to prevent deleted memberships or Teams from reappearing.

Rolling compatibility follows expand/migrate/contract. Producers do not emit
new required fields until all consumers advertise support. Readers tolerate
known optional fields, reject unknown required semantics, and record the
capability-schema generation in authorization evidence.

## Delegated assertions

Labby-to-Depot requests use a signed, short-lived assertion bound to issuer,
audience, subject Principal, installation, Team/Project/Personal context,
method, normalized route/action, exact resource or creation intent, authority
epochs, issued-at, expiry, key ID, and unique assertion ID. Algorithm and key
type are pinned; `none`, symmetric downgrade, unknown critical headers, and
untrusted key URLs are rejected. Key rotation overlaps bounded old/new keys.

Reads may retry only while the authority vector remains current. Mutations use
a durable intent key. Depot atomically records consumption and result, returns
the prior result for an identical replay, and rejects a mismatched replay.
Ambiguous outcomes remain `indeterminate` until queried by the same intent.
General delegation chains are out of scope for v1.

## Audit contract

Every authorization-affecting mutation and security-sensitive decision emits a
bounded append-only audit record. A policy mutation and its record commit in
one local transaction. The record includes event/operation ID, timestamp,
correlation ID, actor Principal, authenticated issuer, action capability,
typed context and target fingerprints, decision/reason, before/after versions,
epoch vector, delegated assertion ID when present, and outcome state. It never
contains bearer tokens, assertions, secret values, raw request bodies, email as
an authority key, or hidden resource names in denial records.

Cross-system audit is correlated, not described as one atomic transaction.
Depot records receipt, policy decision, intent consumption, and outcome under
the Labby operation ID.

## Bootstrap, migration, rollback, and recovery

Upgrade is explicit and resumable:

1. back up AccessStore and Depot durable stores with schema fingerprints;
2. expand schemas without changing authority behavior;
3. create explicit Installation, Organization, bootstrap Principal, and
   PlatformAdministrator rows through a one-time local ceremony bound to a
   `VerifiedIdentity`;
4. classify every existing durable resource into Installation or that
   Principal's Personal scope; never infer Team sharing;
5. emit and verify a complete projection snapshot;
6. run shadow evaluation and compare decisions;
7. enable enforcement only when unclassified-resource count is zero; and
8. contract obsolete columns/endpoints only in a later release.

The restore set includes database, WAL-consistent state, signing keys, key
generations, projection sequence/watermark, schema fingerprints, and bootstrap
generation. Restoring only one member of the set enters recovery-required mode.
Rollback before enforcement restores the complete set. After managed authority
or new scoped writes begin, binaries may roll back only if the older version
advertises the schema/capability generations it can safely read; otherwise the
service fails closed. No rollback path re-enables implicit single-user admin.

Break-glass is a local-console, time-bounded PlatformAdmin grant requiring an
explicit reason and a healthy audit sink. It cannot be issued remotely or by
email comparison, is visible in health/status, increments the platform epoch,
and expires automatically.

## Performance and concurrency invariants

AccessStore never performs network, filesystem, process, or Depot work while a
write transaction or its serialization mutex is held. Authorization reads are
bounded and set-based. Catalogs use a shared base index plus owner-scope
indexes, not a complete per-Principal copy. Lists use keyset pagination with an
epoch-bound cursor; exact counts are optional and separately budgeted.

Admission limits are enforced per owner scope and Principal, with a platform
ceiling. Task/job schedulers are fair across owner scopes. Tests and release
evidence declare latency, query-count, memory/cardinality, queue fairness, and
revocation-to-safe-boundary budgets rather than relying on compilation alone.

## Deferred from v1

Cross-Organization membership/federation, nested Teams, arbitrary custom
roles/grants/denies, general ABAC, impersonation, Team-to-Team ownership
transfer, anonymous Labby serving, billing, SCIM/SAML, and general delegated
assertion chains are not part of this contract.
