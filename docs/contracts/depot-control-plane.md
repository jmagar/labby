---
title: Depot control-plane compatibility contract
status: active
created: 2026-09-03
updated: 2026-09-06
---

# Depot control-plane compatibility contract

`apps/gateway-admin` is the only Labby and Depot frontend. The browser calls
relative Labby URLs; only Labby holds a Depot credential. Depot remains the
authority for Artifact visibility, mutation policy, immutable revisions, and
audit truth.

Multi-user authority is governed by
[`../access-control/MULTI_USER_AUTHORITY.md`](../access-control/MULTI_USER_AUTHORITY.md).
Labby is the command authority for identity, Teams, memberships, Projects, and
role templates in a managed pair; Depot remains the enforcing authority for
its resource rows. This split never permits one side to trust caller-supplied
scope fields.

## Authority modes

Depot advertises exactly one mode. In `standalone`, it owns local principals,
Teams, policy, and recovery and rejects Labby delegated assertions. In
`labby_managed`, local Team mutations are disabled and Depot accepts only its
explicitly paired Labby issuer. Pair, unpair, and local recovery are physical
or loopback operator ceremonies with generation changes and audit evidence;
they do not create two simultaneous command authorities.

Managed Depot consumes a signed ordered projection using a transactional Labby
outbox and durable Depot inbox. It acknowledges only a contiguous sequence.
Duplicates are idempotent; gaps, tombstone loss, schema incompatibility,
signature failure, and tenant mismatch block Team operations until bounded
snapshot-plus-tail resynchronization succeeds.

The release denominator is the joint pair of checked manifests:
[`compatibility-v1.json`](fixtures/depot-control-plane/compatibility-v1.json)
defines the authenticated exact-import and Administration contract, while
[`compatibility-v2.json`](fixtures/depot-control-plane/compatibility-v2.json)
defines federated discovery. Both must pass `just docs-check`. A UI action is available only when its required
operation and contract fingerprint are present. Administration renders Depot's
published `labby.depot-operation-schema/v1` subset as typed controls. The subset,
cardinality limits, authority states, fingerprint binding, and fail-closed
`incompatible` behavior are machine-readable in compatibility-v1. Missing, oversized, or unknown required contracts
render `incompatible`; Labby never invents an unadvertised operation.

## Actor and mount policy

- OAuth browser identity plus a durable Labby principal is required for Depot
  mutation and import routes.
- `none`, `web_ui_auth_disabled`, synthetic development identity, and the
  static-bearer browser shell do not establish a Depot actor.
- A shared Depot service credential may mutate only when the browser principal
  currently holds `lab:admin`, the request carries valid session CSRF, and the
  credential itself carries Depot's required write authority. Depot remains the
  final scope and resource-policy authority.
- Effective permission is the intersection of current Labby permission,
  configured connection ACL, Depot delegated scope, and Depot resource policy.

## Authority epoch and lease

Labby issues an opaque epoch covering its browser-session generation, the
configured Depot connection generation, Depot deployment/account/tenant/team
and principal, the Depot operation fingerprint, and the selected local
destination generation. Cursors, jobs, uploads, intents, confirmations, cache
entries, and receipts are invalid outside that epoch.

The opaque value represents the relevant vector rather than one wall-clock
timestamp: authority schema, installation, Organization, Principal, Team and
Project membership/policy, resource policy, catalog, projection watermark,
credential, connection, destination, and session generations. Execution uses
a short-lived action/method/resource/intent-bound AuthorityLease and
reauthorizes at the final Depot resource boundary.

## Delegated request profile

The delegated assertion is signed and pins issuer, audience, subject Principal,
typed owner context, method, normalized operation, exact resource or creation
intent, authority vector, issue/expiry times, key ID, and unique assertion ID.
Depot pins its algorithm/key profile, rejects key-location indirection and
unknown required semantics, and supports bounded overlapping key rotation.

Mutation assertions carry a durable intent key. Depot atomically records
consumption and the result, returns the same result for an identical retry, and
rejects a changed replay. An ambiguous result remains `indeterminate` until
reconciled by intent. Read retry is allowed only while the entire authority
vector remains current.

## Operational surface

The Administration surface consumes Depot's authorization-filtered canonical
operation catalog. It covers Artifact and Skill lifecycle, sources, ingestion,
uploads, bundles, token administration, and privileged maintenance. Labby keeps
provider connection management beside those operations while Depot remains the
authority for the operation schemas, visibility, revisions, and execution.

Discovery's **Send to Labby** action resolves the selected provider to an
Artifact acquisition connection with the same ID, requests the exact selected
Artifact revision through Depot's `/api/artifacts/exact` contract, verifies its
components, and commits the result through Labby's `artifacts.import` action.
It fails closed when the matching acquisition connection or exact revision is
missing; it never substitutes another configured Depot.

## Bounded transport

- Artifact pages contain at most 200 summaries. Continuation cursors are opaque,
  visibility-bound, and listing-generation-bound.
- Detail contains one current revision and a revision count; unbounded history
  is never returned in the detail envelope.
- Exact export returns `dinglebear.artifact-interchange/v1` and relative
  same-origin component locators. Components are authenticated independently,
  digest verified, and subject to Labby's existing file/package limits.
- Authority responses use `Cache-Control: private, no-store`. Redirects,
  alternate origins, HTML fallthrough, and unbounded decompression are errors.

Federated discovery uses provider-qualified identities and a random 256-bit
Labby cursor. It fairly merges at most one bounded page from each provider,
keeps upstream continuations server-side, reports pending and failed coverage
separately, and expires backscroll after two replayable transitions. Artifact
detail always requires the exact pair of provider ID and raw artifact ID.

## Retry and result truth

Reads may retry after a successful session refresh if the authority epoch is
unchanged. Mutations never replay blindly. A supported mutation carries one
server-bound intent key; an ambiguous response remains `indeterminate` until
the same intent is reconciled. Browser disconnect is not proof of remote
cancellation.

## Compatibility evidence

Release evidence records the Labby commit and binary digest, frontend export
manifest digest, Depot commit and signed image digest, this manifest digest,
operation fingerprint, auth/actor mode, and durable schema generation. Source
checkout combinations are not authoritative release evidence.

## Compatibility rollout and rollback

Depot has exactly one authority mode. `standalone` retains explicit local
platform authority and cannot accept Labby Team delegation. `labby_managed`
disables ordinary local Team mutation and requires a healthy signed projection
plus delegated assertion protocol v1. Missing readiness, a stale watermark, an
unknown protocol version, or the managed-authority kill switch makes Team
mutation unavailable; none of these conditions falls back to standalone.

Rollout order is Depot accept-capable, Labby producer, verified watermark,
then managed enforcement. Mixed versions remain read-compatible, but Team
writes remain disabled until both peers advertise protocol v1 readiness.
Rollback before ownership transfer disables the producer and returns Depot to
explicit standalone mode. After audited ownership transfer, rollback first
enables the kill switch, drains/reconciles outstanding intents, exports the
authority snapshot, and performs an audited transfer back to local platform
authority. Operators must never toggle directly from a degraded managed state
to standalone while Labby remains a command authority.
