# Fleet WebSocket Runtime Design

## Decision

Complete the remaining fleet WebSocket runtime work as one coherent design:

- make the device runtime WebSocket-first rather than HTTP-first
- require explicit master approval before a device token is accepted
- record unknown connection attempts as pending enrollments
- expose explicit master operator controls to list, approve, and deny
  enrollments over API, CLI, and MCP

This design builds directly on the current branch state, which already has:

- upstream WebSocket transport support for gateway connections
- a segmented device outbound queue
- a device WebSocket client that can connect and drain queued envelopes
- a master `/v1/fleet/ws` handler that accepts `initialize`,
  `fleet/metadata.push`, `fleet/status.push`, and `fleet/log.event`

What is still missing is a real session model, a real auth model, and an
operator approval path. Those pieces are intentionally designed together here
because they share the same lifecycle boundary: `initialize`.

## Why

The current branch proves protocol compatibility, but it still behaves like a
polling transport:

- the device connects
- sends `initialize`
- drains a finite batch
- closes the socket

That falls short of the product requirement. The fleet path should behave like
a device control plane:

- one long-lived session per active device
- explicit admission control
- durable pending enrollment state for unknown devices
- one canonical ingest path for metadata, status, and logs

The design also intentionally removes the remaining HTTP bootstrap dependency
for device runtime traffic. The user has accepted WS as the required path for
this branch, so keeping dual transport semantics would add complexity without
improving the rollout.

## Scope

### In scope

- persistent device WebSocket sessions instead of connect, drain, close
- master-side file-backed enrollment storage
- pending, approved, and denied enrollment states
- pending enrollment capture from rejected `initialize` attempts
- explicit approval and denial workflows on the master
- API routes to list, approve, and deny enrollments
- CLI commands to list, approve, and deny enrollments
- MCP actions to list, approve, and deny enrollments
- full migration of device startup metadata and steady-state status and log
  delivery onto WS
- removal of HTTP as the steady-state device runtime path for this feature

### Out of scope

- UI work for enrollment review or approval
- tailnet cryptographic attestation beyond recording presented identity data
- token rotation, token expiry, or multi-token support per device
- general device reprovisioning flows
- cross-master replication of enrollment state
- replacing the device fleet snapshot store with a different persistence model

## User Surface

### Device behavior

Non-master devices no longer treat HTTP as the primary master transport.

Startup flow:

1. ensure a persisted local device token exists
2. open `/v1/fleet/ws`
3. send `initialize` with `device_id`, token, client version, and tailnet
   identity
4. if approved, start the live session and send startup metadata over WS
5. if rejected as pending or denied, back off and retry later

Steady-state flow:

- periodic status updates use the live WS session
- queued metadata, status, and log envelopes drain over the same live WS
  session
- reconnect logic only runs after disconnect or initialize failure

### Master operator surface

The master gets explicit enrollment management surfaces.

API:

- `GET /v1/device/enrollments`
- `POST /v1/device/enrollments/{device_id}/approve`
- `POST /v1/device/enrollments/{device_id}/deny`

CLI:

```bash
lab device enrollments list
lab device enrollments approve <device_id>
lab device enrollments deny <device_id>
```

MCP:

- `device.enrollments.list`
- `device.enrollments.approve`
- `device.enrollments.deny`

The exact dispatch home should follow existing device-oriented service layout
rather than creating an unnecessary new tool.

## Enrollment Model

The enrollment subsystem is separate from the in-memory fleet snapshot store.
The fleet store remains the transient view of connected devices, metadata,
status, and logs. Enrollment is the durable admission-control layer.

### Persistent states

`pending`

- device presented a token that is not approved
- record is operator-reviewable
- future unknown reconnects for the same device update the existing record

`approved`

- device is admitted
- the presented token is pinned to that `device_id`
- successful `initialize` requires an exact match

`denied`

- device has been explicitly denied
- initialize attempts fail immediately

### Stored fields

Pending record:

- `device_id`
- presented token or token fingerprint
- presented tailnet identity
- first seen timestamp
- last seen timestamp
- client version
- optional latest metadata
- optional notes or reason fields if the operator acts later

Approved record:

- `device_id`
- pinned token
- approved timestamp
- approval source
- optional operator note

Denied record:

- `device_id`
- presented or last-known token
- denied timestamp
- denial reason

### Persistence

Use a file-backed store under `~/.labby/` on the master with atomic write
semantics. The format can be JSON or TOML, but it must support:

- load on startup
- full overwrite with atomic rename
- deterministic serialization for review and testing
- simple transition operations between pending, approved, and denied

The store should be designed as a dedicated module so later migration to SQLite
or another backend does not leak across the API, CLI, and WS handlers.

## WebSocket Protocol

### `initialize`

`initialize` becomes the authoritative admission point.

Accepted:

- approved device ID with exact token match

Rejected:

- unknown token or unknown device ID:
  create or update pending enrollment and return `enrollment_required`
- approved device with wrong token:
  return `auth_failed`
- denied device:
  return `access_denied`

Successful initialize returns:

- protocol version
- server info
- minimal session metadata, including confirmed `device_id`

### Live session behavior

After initialize succeeds, the session remains open until network failure,
shutdown, or server-side termination.

During the session:

- `fleet/metadata.push` updates the fleet store and may also refresh the
  pending enrollment preview if needed for future review
- `fleet/status.push` updates connected state and live device status
- `fleet/log.event` records log events into the fleet store

On disconnect:

- the fleet snapshot store marks the device disconnected
- the client reconnect loop starts with backoff and jitter

## Component Design

### Master-side components

`device/enrollment_store.rs` or equivalent:

- load and save durable enrollment state
- transition pending to approved
- transition pending or approved to denied
- query by device ID and token
- expose list views for operator surfaces

`api/device/fleet.rs`

- keep the WS session handler here or factor its protocol logic into a helper
  module if the file grows too large
- `initialize` delegates to the enrollment store
- session state holds the admitted `device_id`

`api/device/` enrollment routes

- list enrollment records
- approve a pending device
- deny a pending or approved device

`cli/device/...`

- add thin commands that call shared dispatch logic

`dispatch/device/...` or existing device dispatch layer

- own action metadata, param parsing, and shared execution for API, CLI, and
  MCP enrollment actions

### Device-side components

`device/ws_client.rs`

- evolve from batch-drain helper into long-lived session manager
- own reconnect loop, initialize handshake, heartbeat handling if needed, and
  outbound message delivery over one socket

`device/runtime.rs`

- stop using HTTP startup calls for metadata and status flow
- queue metadata just like logs and status
- delegate all master communication to the WS client lifecycle

`device/token.rs`

- remains the local token authority on the device
- no longer implies trust on the master until approval happens

## Data Flow

### Unknown device

1. device opens WS and sends `initialize`
2. master finds no approved entry
3. master records or updates a pending enrollment
4. master returns `enrollment_required`
5. device logs and backs off
6. operator reviews and approves or denies
7. next reconnect either succeeds or is denied

### Approved device

1. device opens WS and sends `initialize`
2. master validates exact token match
3. session becomes active
4. device sends metadata if needed
5. device sends periodic status
6. queue drain sends metadata, status, and logs over the same socket
7. disconnect marks the device offline and triggers reconnect

## Error Handling

Stable error kinds for initialize failures should align with the existing
structured error vocabulary:

- `enrollment_required`
- `auth_failed`
- `access_denied`
- `validation_failed`

The WS handler should return JSON-RPC error objects that include a stable kind
in either the message or structured data payload, depending on what fits the
existing envelope rules without inventing a divergent protocol.

Operator surfaces should preserve stable errors as well:

- approving unknown device ID returns `not_found`
- approving denied device is either `validation_failed` or explicit no-op,
  depending on chosen semantics
- denying unknown device ID returns `not_found`

For v1, approval is idempotent and token rotation is out of scope. Re-approving
an already approved device should not silently replace the token.

## Observability

All new paths must follow `docs/OBSERVABILITY.md`.

Required additions:

- WS initialize success and failure events with device ID when safe to log
- enrollment-record-created and enrollment-record-updated events
- enrollment approve and deny operator events
- session connected and disconnected events
- queue drain success and failure over WS

Forbidden:

- logging raw device tokens

If a token needs operator correlation, log only a fingerprint or omit it from
normal traces entirely.

## Testing

### Unit tests

- enrollment store load and save
- atomic transition rules between pending, approved, and denied
- initialize auth decision matrix
- idempotent approval and denial behavior

### Integration-style tests

- unknown device creates pending enrollment and gets rejected
- approved device connects and stays admitted
- denied device is rejected
- approved device with wrong token is rejected
- device reconnect after approval succeeds without changing local token
- live session disconnect marks device offline

### Surface tests

- API list, approve, and deny
- CLI list, approve, and deny
- MCP list, approve, and deny

### Regression tests

- device WS client still drains queue correctly
- metadata, status, and log flows all operate on the real axum WS handler

## Implementation Order

1. add the file-backed enrollment store and tests
2. wire `initialize` to the enrollment store and return pending or approved
   outcomes correctly
3. add master API, CLI, and MCP enrollment actions
4. convert the device WS client into a long-lived session manager
5. move metadata and status fully onto the live WS path
6. remove the remaining HTTP bootstrap dependency from device runtime flows
7. run targeted verification, then all-features verification

## Risks

- file-backed enrollment state must be written atomically or operator actions
  become unsafe
- long-lived WS sessions can expose shutdown and reconnect edge cases that the
  current batch client avoided
- removing HTTP fallback means rollout coordination matters; devices against an
  older master will not connect until both sides support the approved protocol

Those risks are acceptable for this branch because the user explicitly accepted
WS as the required path and requested the approval model in the same slice.
