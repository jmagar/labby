# Serialization

This document is the canonical serialization contract for `lab`.

It defines:

- which layer owns wire types and presentation
- the stable envelope shapes for MCP and HTTP
- output-format responsibilities
- naming and serde expectations

## Goal

Serialization rules should keep the project consistent across:

- upstream API request and response bodies
- MCP tool input and output
- API input and output
- CLI human and machine output

The main boundary is simple:

- `lab-apis` owns typed service data and wire-level serde models
- `lab` owns product-surface envelopes and presentation formats

## Ownership

### `lab-apis`

`lab-apis` owns:

- request and response structs used against upstream services
- serde derives and field mappings needed to talk to those services
- typed SDK data returned to the binary

`lab-apis` does not own:

- table rendering
- CLI presentation wrappers
- MCP envelope construction
- HTTP error envelope construction

### `labby`

`lab` owns:

- MCP success and error envelopes
- HTTP request/response envelope shaping for the product API
- CLI human and JSON output behavior

## SDK Type Rules

For SDK types in `lab-apis`:

- use `serde` derives on wire-facing types
- model the upstream API as faithfully as practical
- keep presentation concerns out of SDK structs
- prefer explicit field mappings when upstream naming is non-idiomatic

If naming transformations are needed, use serde attributes intentionally rather than relying on hidden conventions.

## Surface Envelope Rules

### MCP Success Envelope

Canonical success shape:

```json
{
  "ok": true,
  "service": "marketplace",
  "action": "mcp.list",
  "data": []
}
```

Rules:

- `ok`, `service`, `action`, and `data` are required
- optional `meta` may be added when needed
- the envelope should remain stable across services

### MCP Error Envelope

Canonical error shape:

```json
{
  "ok": false,
  "service": "marketplace",
  "action": "mcp.install",
  "error": {
    "kind": "missing_param",
    "message": "missing parameter: name"
  }
}
```

The error payload follows [ERRORS.md](../ERRORS.md).

### HTTP Product API

HTTP handlers should mirror the MCP dispatch input model:

- one action-based request shape
- one structured JSON error shape
- shared semantic behavior across transports

MCP wraps successful action data in the MCP success envelope. HTTP action
routes return the action data directly on success and use the shared structured
error shape on failure. When HTTP and MCP both expose the same action surface,
they should not drift in input fields or error semantics.

## CLI Output Rules

The CLI supports:

- human-readable terminal rendering
- JSON

Rules:

- human-readable rendering is built in `lab`, not SDK types
- machine-readable output serializes the underlying SDK data or surface envelope shape
- `lab-apis` types must remain presentation-free

If a command needs special display rows, define them in `lab`, not in the SDK.

## Naming Rules

Field naming should be explicit.

Rules:

- use serde attributes when upstream APIs require a specific field name or case
- do not rely on accidental naming matches when the upstream contract is unstable or unclear
- keep external wire names and internal Rust names distinct when that improves clarity

For product-surface envelopes, keep the established field names stable:

- `ok`
- `service`
- `action`
- `data`
- `error`
- `kind`
- `message`

## Null And Optional Data

Rules:

- use `Option<T>` when the upstream field is genuinely optional
- do not serialize placeholder empty strings or sentinel values to fake optionality
- treat missing and null according to the upstream contract rather than preference

## Product Input Shapes

For MCP and HTTP dispatch:

- `action` is the stable operation selector
- `params` is the action-specific object

Do not invent service-specific top-level request shapes that bypass the shared dispatcher model.

## Redaction And Safety

Serialization must respect the security rules:

- do not serialize secrets into logs
- do not serialize secret env values into prompts or doctor output
- do not accidentally expose auth headers, cookies, or tokens in debug output

Observability-specific logging rules live in [OBSERVABILITY.md](../OBSERVABILITY.md).

## Verification Requirements

At minimum, verify:

1. upstream request/response types deserialize and serialize correctly
2. MCP success and error envelopes match the documented shape
3. HTTP JSON error shape matches the documented shape
4. CLI JSON output serializes the intended data rather than presentation wrappers unless that wrapper is the contract

## Related Docs

- [ERRORS.md](../ERRORS.md)
- [OBSERVABILITY.md](../OBSERVABILITY.md)
- [MCP.md](../MCP.md)
- [CLI.md](../CLI.md)
- [CONVENTIONS.md](../CONVENTIONS.md)
