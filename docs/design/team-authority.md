---
title: "Team authority architecture"
created: "2026-09-05"
updated: "2026-09-05"
status: "design"
---

# Team authority architecture

The normative domain and protocol rules are in
[`../access-control/MULTI_USER_AUTHORITY.md`](../access-control/MULTI_USER_AUTHORITY.md).
This document records the implementation boundaries that keep those rules
consistent across Labby, Depot, and every Labby surface.

## Command and enforcement boundaries

Labby AccessStore owns identity, PlatformAdministrator, Team, membership,
Project assignment, and role-template commands. Shared dispatch receives a
typed authority context and performs policy evaluation. CLI, MCP, HTTP, Code
Mode, and web UI are adapters; none may infer a Team, translate `lab:admin`
into a role, or bypass final-boundary authorization.

Depot owns Library/Artifact rows and their resource policy. In managed mode it
uses its durable projection of Labby authority plus a per-request delegated
assertion. It must not call Labby synchronously in the authorization hot path.
Projection lag or gaps fail closed for Team operations while bounded Personal
or public operations may proceed only if their own complete policy facts do
not depend on the missing sequence.

## Data flow

```text
VerifiedIdentity
      |
      v
Labby AccessStore --transactional outbox--> signed ordered projection
      |                                      |
      v                                      v
AuthoritySnapshot                         Depot inbox/policy store
      |                                      ^
      +--> shared dispatch --> assertion ----+
              |                              |
              +--> local resource check      +--> Depot resource check
```

An AccessStore transaction ends before projection delivery, Depot calls,
filesystem work, process launch, or any other external effect begins.

## Change protocol

Cross-repository releases use compatibility-ordered changes:

1. consumers accept the next optional schema and advertise support;
2. producers populate it without relying on it;
3. migration/backfill reaches a verified watermark;
4. enforcement switches to the new required semantics; and
5. obsolete fields and endpoints are removed in a later release.

Labby and Depot releases remain independently deployable. A mismatch is a
typed incompatible/blocked state, never permissive fallback.

## Runtime objects

`AuthoritySnapshot` is immutable evidence for discovery and response shaping.
`AuthorityLease` is the short-lived execution guard. A snapshot is not an
execution credential. Retained runtime objects store identifiers and epochs,
not cloned membership rows or reusable delegated assertions.

Caches are keyed by typed owner context, Principal, relevant epoch vector, and
catalog generation. Browser cache keys use the same typed authority identity.
Changing Team/context advances a client generation; late responses from an old
generation are discarded. Global request abort registries are unnecessary:
each request owns its cancellation handle and generation check.

## Resource lifecycle integration

Library, Project, Gateway, Stash, Agent, Task, and Dev Container implementations
all implement the same sequence: create with one typed owner; check owner and
capability on list/get/use/mutate; increment the correct epoch on policy or
ownership changes; audit; and revalidate retained work at safe boundaries.

Each family additionally defines its own lifecycle. In particular, Tasks and
jobs have idempotent settlement, Agents have versioned definitions separate
from executions, and Dev Containers have durable operation ledgers plus
instance nonces so a stale stop/delete cannot target a replacement instance.

## Operations

Health exposes authority mode, schema generation, projection watermark/lag,
blocked gap, active signing-key generation, and break-glass expiry without
exposing principals or resource names. Backup/restore treats policy stores,
keys, sequences, and bootstrap generation as one restore set.

Load tests cover high-cardinality memberships, mixed Team/Personal catalogs,
revocation during long work, per-owner admission, fair queueing, projection
catch-up, and bounded resync. Release evidence records both repository commits,
schema/capability generations, migration watermark, and enforcement mode.
