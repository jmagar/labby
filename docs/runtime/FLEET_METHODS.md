# Fleet WebSocket Methods

This document covers all JSON-RPC 2.0 methods available on the fleet WebSocket endpoint at `GET /v1/nodes/ws`.

## Auth Model

The endpoint is intentionally placed **outside bearer-auth middleware**. Until a session is established by a successful `initialize` call, the only methods the server will execute are `initialize` itself and the MCP demux methods that are allowlisted by name (see below) for discovery. All node methods (`nodes/*`) require an active authenticated session established by a prior `initialize` call.

## General Envelope

All messages follow JSON-RPC 2.0:

```jsonc
// Request (client → server)
{ "jsonrpc": "2.0", "id": <id>, "method": "<method>", "params": { ... } }

// Success response (server → client)
{ "jsonrpc": "2.0", "id": <id>, "result": { ... } }

// Error response (server → client)
{ "jsonrpc": "2.0", "id": <id>, "error": { "code": <int>, "message": "<msg>", "data": { "kind": "<kind>" } } }
```

Error `code` conventions:
- `-32602` — invalid / missing params
- `-32601` — method not found / not permitted
- `-32001` — auth error (auth_failed, access_denied, enrollment_required)
- `-32000` — general server error

---

## Methods

### `initialize`

**Direction:** client → server  
**Auth required:** no (this establishes auth)  
**Phase:** stable  

Performs enrollment validation and establishes the node session. The server closes the connection if no first WebSocket message arrives within 10 seconds.

**Params:**
```jsonc
{
  "protocolVersion": "2024-11-05",
  "capabilities": {},
  "clientInfo": { "name": "lab-node", "version": "<semver>" },
  "_meta": {
    "lab.node_id": "<node_id>",
    "lab.device_token": "<token>",
    "lab.tailnet_identity": {
      "node_key": "<tailscale_node_key>",
      "login_name": "<user@example.com>",
      "hostname": "<device_hostname>"
    }
  }
}
```

**Result:**
```jsonc
{
  "protocolVersion": "2024-11-05",
  "serverInfo": { "name": "lab-nodes", "version": "<server_semver>" },
  "_meta": { "lab.node_id": "<node_id>" }
}
```

**Error kinds:**
| Kind | Meaning |
|------|---------|
| `enrollment_required` | Node is pending approval or was debounced (retry after 30 s) |
| `enrollment_cap_exceeded` | More than 1000 nodes are already in the pending queue |
| `access_denied` | Node has been explicitly denied |
| `auth_failed` | Device token does not match the approved record |

---

### `nodes/ping`

**Direction:** client → server  
**Auth required:** yes (session must be initialized)  
**Phase:** stable  

Liveness check. Returns an empty result.

**Params:** `{}` or omitted  
**Result:** `{}`

**Error kinds:**
| Kind | Meaning |
|------|---------|
| `auth_failed` | Session not yet initialized |

---

### `nodes/status.push`

**Direction:** client → server  
**Auth required:** yes  
**Phase:** stable  

Reports node telemetry (CPU, memory, storage, network).

**Params:**
```jsonc
{
  "node_id": "<node_id>",
  "connected": true,
  "cpu_percent": 12.5,
  "memory_used_bytes": 1073741824,
  "storage_used_bytes": 21474836480,
  "os": "linux",
  "ips": ["100.64.0.1"]
}
```

**Result:** `{}`

**Error kinds:**
| Kind | Meaning |
|------|---------|
| `auth_failed` | Session not initialized |
| `invalid_param` | `node_id` does not match session node or params are malformed |

---

### `nodes/metadata.push`

**Direction:** client → server  
**Auth required:** yes  
**Phase:** stable  

Reports discovered service configuration on the node.

**Params:**
```jsonc
{
  "node_id": "<node_id>",
  "discovered_configs": [
    { "service": "gateway", "url": "http://localhost:8765/mcp", ... }
  ]
}
```

**Result:** `{}`

**Error kinds:**
| Kind | Meaning |
|------|---------|
| `auth_failed` | Session not initialized |
| `invalid_param` | `node_id` mismatch or malformed params |

---

### `nodes/log.event`

**Direction:** client → server  
**Auth required:** yes  
**Phase:** stable  

Batch-uploads structured log events from the node.

**Params:**
```jsonc
{
  "node_id": "<node_id>",
  "events": [
    {
      "node_id": "<node_id>",
      "source": "syslog",
      "timestamp_unix_ms": 1700000000000,
      "level": "info",
      "message": "kernel: started",
      "fields": {}
    }
  ]
}
```

**Result:** `{}`

**Error kinds:**
| Kind | Meaning |
|------|---------|
| `auth_failed` | Session not initialized |
| `invalid_param` | `node_id` mismatch, malformed events, or per-event validation error |

---

### `nodes/device.enroll`

**Direction:** client → server  
**Auth required:** yes (session must be initialized)  
**Phase:** beta  

Registers or re-confirms a node identity in the node store. Idempotent if role matches; returns `enroll_rejected` on role mismatch.

**Params:**
```jsonc
{
  "node_id": "<node_id>",
  "role": "node",        // accepted: "node" | "master"
  "version": "<semver>"
}
```

**Result:**
```jsonc
{ "enrolled": true, "node_id": "<node_id>" }
```

**Error kinds:**
| Kind | Meaning |
|------|---------|
| `auth_failed` | Session not initialized |
| `invalid_param` | Missing or empty `node_id`, `role`, or `version`; or unknown `role` |
| `enroll_conflict` | Node already registered with a different role |

---

### `nodes/command.invoke`

**Direction:** client → server (client triggers execution on itself)  
**Auth required:** yes  
**Phase:** beta  

Requests the server to record a new command execution context and push the invocation back to the node. The server assigns a `command_id` (UUID), stores a command state entry, and immediately pushes a `nodes/command.invoke` server-push frame to the node (on the same channel). Output and result are reported back via `nodes/command.output` and `nodes/command.result`.

Command state entries have a 5-minute TTL and are swept every 60 seconds.

**Params:**
```jsonc
{ "command": "<shell command string>" }
```

**Result:**
```jsonc
{ "command_id": "<uuid>" }
```

**Server-push frame (server → node, same connection):**
```jsonc
{
  "jsonrpc": "2.0",
  "id": "<command_id>",
  "method": "nodes/command.invoke",
  "params": { "command_id": "<uuid>", "command": "<shell command string>" }
}
```

**Error kinds:**
| Kind | Meaning |
|------|---------|
| `auth_failed` | Session not initialized |

---

### `nodes/command.output`

**Direction:** client → server (node reports stdout/stderr chunk)  
**Auth required:** yes  
**Phase:** beta  

Streams a chunk of command output to the server. The server forwards each chunk to the output mpsc channel (capacity 512) for the command identified by `command_id`. Ignored if the `command_id` is unknown (e.g. already expired).

**Params:**
```jsonc
{ "command_id": "<uuid>", "chunk": "<stdout/stderr text>" }
```

**Result:** `{}`

**Error kinds:**
| Kind | Meaning |
|------|---------|
| `auth_failed` | Session not initialized |
| `invalid_param` | Missing or malformed `command_id` |

---

### `nodes/command.result`

**Direction:** client → server (node reports final exit status)  
**Auth required:** yes  
**Phase:** beta  

Reports the final result of a command execution. Removes the command from the active state map.

**Params:**
```jsonc
{ "command_id": "<uuid>", "exit_code": 0, "success": true }
```

**Result:**
```jsonc
{ "command_id": "<uuid>", "exit_code": 0, "success": true }
```

**Error kinds:**
| Kind | Meaning |
|------|---------|
| `auth_failed` | Session not initialized |
| `invalid_param` | Missing or malformed `command_id` |

---

### `nodes/peer.invoke`

**Direction:** client → server  
**Auth required:** yes  
**Phase:** not implemented  

Reserved for peer-to-peer RPC between fleet nodes. Currently returns `-32601 not_implemented`.

**Params:** any  
**Result:** (none — always errors)

**Error kinds:**
| Kind | Meaning |
|------|---------|
| `not_implemented` | Peer RPC is not yet implemented |

---

## MCP Demux

Methods not in the `nodes/` namespace are demuxed through the MCP registry with an allowlist gate. Only the following methods are permitted:

```
lab://catalog
<service>.help
<service>.schema
```

Any other non-`nodes/` method returns:

```jsonc
{ "error": { "code": -32601, "message": "method not permitted over fleet WS", "data": { "kind": "not_permitted" } } }
```

Allowlisted demux calls have a 30-second timeout. A timeout yields:

```jsonc
{ "error": { "code": -32001, "message": "upstream timeout", "data": { "kind": "upstream_timeout" } } }
```

Demux calls are forwarded by splitting the method name on the first `.` to extract `(service, action)`, then dispatching to the matching registry service. This is identical to the MCP action dispatch shape.

---

## Limits and Hardening

| Limit | Value |
|-------|-------|
| Initialize timeout | 10 seconds |
| Enrollment debounce (PendingRequired path only) | 30 seconds per `node_id` |
| Max pending enrollments | 1000 |
| Command output channel capacity | 512 frames |
| Command TTL | 5 minutes |
| Command sweep interval | 60 seconds |
| Max message size | 10 MiB |
