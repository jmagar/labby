# Dev Containers

Dev Containers are owner-scoped, quota-bounded development environments. This
document freezes the contract and persistence boundary; Labby does not yet
register a Dev Container service or execute a container runtime.

Every instance has exactly one installation, Team, Project, or Personal owner.
Its durable record pins an administrator-approved template and an immutable
`sha256:<64 lowercase hex>` image digest. Tags and caller-supplied image names
are never launch authority.

## Admission

A launch is admitted only when all of these remain true at the final execution
boundary:

- the caller currently has the required capability over the exact owner scope;
- the template is still approved and its pinned image digest is unchanged;
- the owner's active-instance quota has capacity;
- requested CPU, memory, disk, and lifetime are non-zero and do not exceed the
  template ceiling; and
- every requested host capability is explicitly approved by the template.

Host access is default-denied. Privileged execution, the host filesystem,
container-runtime sockets, host networking, host devices, and kernel
administration are separate capabilities. Approval of one never implies
another.

Secrets are stored as opaque secret references. Secret values, environment
material, credentials, and decrypted content do not belong in the instance
ledger, audit records, API payloads, or logs.

## Durable lifecycle

The ledger stores the typed owner, instance ID, template ID, image digest,
lifecycle nonce, desired state, observed state, quota reservation, secret
references, authority epoch/fingerprint, revision, and timestamps. Desired
states are `running`, `stopped`, and `deleted`; observed states are `pending`,
`starting`, `running`, `stopping`, `stopped`, `failed`, and `deleted`.

Every create or recreate receives a new unpredictable lifecycle nonce. Runtime
observations and cleanup receipts must carry that exact nonce, so a late event
from an earlier instance cannot mutate or delete a replacement that reused the
same external name. Desired/observed transitions use compare-and-swap over the
ledger revision.

Deletion is terminal for a lifecycle nonce. Durable state is retained long
enough to reconcile cleanup and prove quota release; callers cannot restore a
deleted nonce. A new instance is a new lifecycle.

## Revocation and failure

Membership, policy, template, credential, or owner changes invalidate retained
authority at the next safe boundary. Admission, runtime start, credential
checkout, external effects, observation commit, stop, deletion, and retained
resume reauthorize current state. Missing, corrupt, stale, or mismatched state
fails closed.

An indeterminate runtime outcome remains reconciliable state; it is not reported
as success and its quota reservation is not silently released. Cleanup acts
only on resources proven to carry the ledger's instance ID and lifecycle nonce.

This contract does not authorize direct access to a container engine and does
not define HTTP, MCP, CLI, or web surfaces. Those adapters may be added only
after the durable ledger and runtime reconciliation implementation exist.
