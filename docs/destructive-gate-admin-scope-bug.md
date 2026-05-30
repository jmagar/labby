# Bug: Code Mode destructive-action gate ignores `lab:admin` scope

> Diagnosed 2026-05-29. The admin email account correctly receives `lab:admin`, but that scope
> grants nothing at the destructive-action gate — which checks only a `confirm` flag, not scopes.
> This is the recurring "why doesn't the admin account already have these permissions" issue.

## Symptom
Through the MCP Code Mode surface (`mcp__plugin_lab_lab__execute`), read-only upstream tools work
(`Screenshot`, `Snapshot`) but every destructive tool (`PowerShell`, `App`, `Click`, `Process`,
`Type`, …) returns:

```
confirmation_required: "Tool `agent-os_windows-mcp::PowerShell` has destructive=true.
Set allow_destructive_actions=true in the Code Mode surface to proceed."
```

…even when the caller's JWT carries `lab:admin`.

## Root cause — two decoupled axes

### Axis 1: scopes (works correctly)
`crates/lab-auth/src/authorize.rs:354` — after `check_email_allowlist` passes, the OAuth callback
calls `elevate_scope_for_allowed_user(&request.scope, &state.config.default_scope)`
(authorize.rs:464). Being on the allowlist IS the admin gate, so the issued token gets
`lab:admin` injected regardless of what the client requested. **The admin email DOES get
`lab:admin`.** Confirmed.

The Code Mode caller then checks scopes correctly:
- `CodeModeCaller::can_read()`  → `lab:read | lab | lab:admin` (code_mode.rs:308)
- `CodeModeCaller::can_execute()` → `lab | lab:admin` (code_mode.rs:317)

An admin token passes both.

### Axis 2: destructive gate (ignores scopes — THE BUG)
`crates/lab/src/dispatch/gateway/code_mode.rs:957`:

```rust
if upstream_tool.destructive && !surface.allow_destructive_actions() {
    return Err(ToolError::Sdk { sdk_kind: "confirmation_required", ... });
}
```

The gate consults **only `surface.allow_destructive_actions()`** — never `caller`/scopes. And the
MCP surface's flag is set in exactly one place, `crates/lab/src/mcp/server.rs:1480`:

```rust
let allow_destructive_actions =
    args.get("confirm").and_then(Value::as_bool) == Some(true);
```

So destructive permission is driven **purely by a `confirm: true` argument on the `execute` tool
call** — fully decoupled from authentication. `CodeModeSurface::Cli` always returns `true`
(operator-driven); `CodeModeSurface::Mcp { allow_destructive_actions }` echoes the `confirm` flag.

### Why the caller is available but unused
`code_mode.rs:909 call_tool_id` has `caller: CodeModeCaller` in scope (line 913) and uses it for
`runtime_owner`/`oauth_subject` (lines 925–926), but passes **only `surface`** into
`call_upstream_tool` (line 934). The `caller` — and its scopes — is dropped before the gate.

## Why the client can't work around it
`confirm: true` must be a **top-level arg to the `lab__execute` tool**, beside `code`. The Claude
Code harness wrapper for `mcp__plugin_lab_lab__execute` exposes only `code`, so a client cannot set
`confirm`. Net: the only surface that currently permits destructive actions is the **CLI**. An
authenticated admin over MCP is hard-blocked — contradicting the stated design intent at
authorize.rs:348-353 ("operators … get admin … so MCP clients can call destructive gateway/setup
actions without a separate flow").

## The fix (surgical)
Make the destructive gate honor an admin/execute-scoped caller — an admin's surface should permit
destructive actions, exactly as the authorize.rs comment intends.

Option A (preferred — gate honors caller scope): thread `caller` into `call_upstream_tool` and
allow when the caller `can_execute()` (i.e. holds `lab` or `lab:admin`):

```rust
// code_mode.rs call_tool_id — pass caller through:
self.call_upstream_tool(manager, &upstream, &tool, params, &owner, oauth_subject, surface, &caller).await

// call_upstream_tool signature gains `caller: &CodeModeCaller`, gate becomes:
let permitted = surface.allow_destructive_actions() || caller.can_execute();
if upstream_tool.destructive && !permitted {
    return Err(... confirmation_required ...);
}
```

Rationale: `can_execute()` already means `lab | lab:admin` — the same bar the authorize flow
auto-grants allowlisted operators. This realigns the two axes so admin scope actually means admin.

Option B (surface-level): when constructing `CodeModeSurface::Mcp` in `server.rs:1479`, OR the
`confirm` flag with whether the authenticated caller holds `lab:admin`:
`allow_destructive_actions = confirm || auth.scopes.contains("lab:admin")`. Keeps the gate untouched
but needs the auth scopes at that construction site.

Either way the principle: **`lab:admin` (or `lab`) must satisfy the destructive gate.** Today it
does not, which is why the admin email "doesn't already have these scopes" in practice — it has the
scope, the scope is just ignored at the one gate that matters.

## Tests to add
- `code_mode`: destructive upstream tool + `CodeModeCaller::Scoped{ scopes:["lab:admin"] }` +
  `Mcp{ allow_destructive_actions:false }` ⇒ **permitted** (currently denied).
- destructive + `Scoped{ ["lab:read"] }` + `Mcp{false}` ⇒ denied (unchanged).
- destructive + `Cli` ⇒ permitted (unchanged).
