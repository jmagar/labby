# Agents and Agent Tasks

Labby owns durable Agent definitions and Agent Task lifecycle. These resources
are separate from Depot artifacts and ingestion jobs.

Every definition and task has exactly one `installation`, `team`, `project`, or
`personal` owner. The authenticated principal is resolved by the host; owner
and principal parameters never establish authority. List operations omit
resources the caller cannot read, while direct reads and mutations return the
same non-enumerating denial for absent and unauthorized identifiers.

Agent revisions pin content, repository, image, harness, loadout, credentials,
and catalog generations. Updating an Agent creates the next immutable revision.
Suspension or deletion blocks future runs. Runtime leases are checked at safe
boundaries so membership or policy revocation fences retained execution.

Agent Tasks capture an exact Agent revision, normalized input digest, catalog
generation, owner, creator, and authority fingerprint. Task idempotency keys are
scoped to the owner and bind the full immutable intent. Queue, cancellation,
execution, and settlement use fenced state transitions; terminal settlement is
exactly once. Current authority is required for every list, get, cancellation,
and result operation.

Authenticated HTTP exposes `POST /v1/agents` and `POST /v1/tasks` with the same
`action` plus `params` envelope used by MCP. MCP exposes the `agents` and
`tasks` caller-bound services only when the transport supplies a verified
identity. Context-free invocation fails closed. Local CLI invocation is not
offered because it has no equivalent authenticated identity binding.
