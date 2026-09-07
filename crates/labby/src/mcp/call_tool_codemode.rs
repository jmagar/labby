//! Code Mode gateway tool branch of `call_tool`.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.5`) as inherent
//! `impl LabMcpServer` helpers. Each helper is reached only after the
//! service-name match in `call_tool_impl` and self-`return`s its result.
//! Owns the single definition of the Code Mode tool description renderer, plus
//! `string_array_arg`.
//!
//! This branch logs via `tracing` directly (not `emit_dispatch_notification`)
//! and fires lightweight catalog-change detection around the broker call.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use labby_codemode::error::ToolError as CodeModeToolError;
use labby_codemode::{
    CodeModeExecutedCall, CodeModeExecutionError, CodeModeExecutionResponse, MAX_SOURCE_BYTES,
    SERVICE as CODE_MODE_SERVICE,
};
use labby_runtime::catalog_notify::SOURCE_MCP_CALL_CODEMODE;
use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, MetaObject};
use rmcp::service::RequestContext;
use serde_json::Value;
use tokio::sync::Notify;

use crate::dispatch::error::ToolError as DispatchToolError;
use crate::dispatch::gateway::code_mode::{
    CodeModeBroker, CodeModeCaller, CodeModeCallerCapabilities, CodeModeExecutionSource,
    CodeModeHistoryEntry, CodeModeHistoryKind, JournalOwner, ToolScope, code_mode_execute_trace,
};
use crate::dispatch::gateway::manager::GatewayManager;
use crate::mcp::catalog::CODE_MODE_READ_TOOL_NAME;
use crate::mcp::context::{
    auth_context_from_extensions, code_mode_read_scope_allowed, tool_execute_scope_allowed,
};
use crate::mcp::envelope::{build_error, build_error_extra};
use crate::mcp::result_format::{
    code_mode_error_envelope, error_result_from_envelope, estimate_tokens, estimate_tokens_args,
    hash_arguments, tool_error_envelope,
};
use crate::mcp::server::LabMcpServer;

type SharedCodeModeResult = Result<CodeModeExecutionResponse, CodeModeExecutionError>;

struct InflightCodeModeExecution {
    leader_execution_id: String,
    result: Mutex<Option<SharedCodeModeResult>>,
    notify: Notify,
}

static CODE_MODE_INFLIGHT: OnceLock<Mutex<HashMap<String, Arc<InflightCodeModeExecution>>>> =
    OnceLock::new();

fn code_mode_inflight() -> &'static Mutex<HashMap<String, Arc<InflightCodeModeExecution>>> {
    CODE_MODE_INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

enum InflightCodeModeRole {
    Leader(CodeModeInflightLeader),
    Follower(Arc<InflightCodeModeExecution>),
}

fn begin_code_mode_execution(key: String, execution_id: String) -> InflightCodeModeRole {
    let mut inflight = code_mode_inflight()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = inflight.get(&key) {
        return InflightCodeModeRole::Follower(Arc::clone(existing));
    }
    let entry = Arc::new(InflightCodeModeExecution {
        leader_execution_id: execution_id,
        result: Mutex::new(None),
        notify: Notify::new(),
    });
    inflight.insert(key.clone(), Arc::clone(&entry));
    InflightCodeModeRole::Leader(CodeModeInflightLeader {
        key,
        entry,
        completed: false,
    })
}

async fn await_code_mode_execution(entry: Arc<InflightCodeModeExecution>) -> SharedCodeModeResult {
    loop {
        // Register before checking the result so a completion between the check
        // and await cannot be missed.
        let notified = entry.notify.notified();
        if let Some(result) = entry
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return result;
        }
        notified.await;
    }
}

struct CodeModeInflightLeader {
    key: String,
    entry: Arc<InflightCodeModeExecution>,
    completed: bool,
}

impl CodeModeInflightLeader {
    fn complete(mut self, result: &SharedCodeModeResult) {
        *self
            .entry
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result.clone());
        code_mode_inflight()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.key);
        self.entry.notify.notify_waiters();
        self.completed = true;
    }
}

impl Drop for CodeModeInflightLeader {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let error = CodeModeExecutionError::from(CodeModeToolError::Sdk {
            sdk_kind: "service_unavailable".to_string(),
            message: "leading duplicate Code Mode execution was cancelled".to_string(),
        });
        *self
            .entry
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Err(error));
        code_mode_inflight()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.key);
        self.entry.notify.notify_waiters();
    }
}

struct StepBufferDropGuard {
    manager: Arc<GatewayManager>,
    execution_id: String,
    armed: bool,
}

/// Persist non-critical Code Mode telemetry after the MCP response path has
/// been released.
///
/// Step journaling, execution history, and source retention are deliberately
/// fail-open. Awaiting them after a run has already reached its wall-clock
/// deadline can make a valid Code Mode timeout indistinguishable from a
/// disconnected daemon to a downstream MCP client. Drain/persist in one
/// background task so the caller receives the execution result promptly.
fn spawn_code_mode_persistence(
    manager: &Arc<GatewayManager>,
    execution_id: String,
    journal_owner: JournalOwner,
    history: CodeModeHistoryEntry,
    source: Option<CodeModeExecutionSource>,
) {
    let manager = Arc::clone(manager);
    tokio::spawn(async move {
        manager
            .flush_step_journal(&execution_id, &journal_owner)
            .await;
        manager.record_code_mode_history(history).await;
        if let Some(source) = source {
            manager.record_code_mode_source(source).await;
        }
    });
}

impl StepBufferDropGuard {
    fn new(manager: Arc<GatewayManager>, execution_id: String) -> Self {
        Self {
            manager,
            execution_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StepBufferDropGuard {
    fn drop(&mut self) {
        if self.armed {
            self.manager.discard_step_buffer(&self.execution_id);
        }
    }
}

/// Static body for the primary `codemode` MCP tool description.
///
/// The final model-visible description is rendered with the current enabled,
/// route-scoped upstream namespace snapshot by `code_mode_description`.
pub(crate) const CODE_MODE_DESCRIPTION_BODY: &str = "\
Execute JavaScript in a sandbox with access to the Labby gateway catalog.

## Workflow

1. Discover: `const hits = await codemode.search({ query: \"short intent phrase\", limit: 5 });`
2. Inspect: `const docs = await codemode.describe(hits.results[0].path);`
3. Read a resource: `await codemode.readResource(\"lab://upstream/<name>/<uri>\");`
4. Call: `await codemode.<upstream>.<tool>(params)` or `await callTool(\"upstream::tool\", params);`

Never guess helper or method names. If you have not already confirmed the exact \
tool, run `codemode.search(...)` first. `codemode.search` returns compact \
signatures; `codemode.describe(\"upstream.tool\")` returns focused TypeScript \
declarations and call details.

Enabled upstream namespaces are summarized below from the current route-scoped \
configuration. Their individual tools and runtime health remain live; discover \
those at execution time with `codemode.search` and `codemode.describe`.

Pass `code` as `async () => { ... }` — the sandbox awaits its return value. \
Whatever it returns becomes `result`.

```ts
async () => {
  const hits = await codemode.search({ query: 'github issues', limit: 1 });
  const docs = await codemode.describe(hits.results[0].path);
  const issues = await codemode.github.search_issues({ q: 'bug' });
  return { tool: docs.path, count: issues.items.length };
}
```

Available globals: `codemode`, `callTool`, and `writeArtifact`. There is no \
`require`, `process`, `fs`, `fetch`, Node.js, Deno, or Bun API. All external I/O \
goes through gateway tools.

Optional top-level inputs to this MCP tool:
- `upstreams`: restrict this run to specific upstream namespaces.
- `tools`: restrict this run to specific tools; accepts raw tool names or \
`upstream::tool` ids.

Every upstream MCP tool is callable two ways: `callTool(id, params)`, or the \
auto-generated `codemode.<upstream>.<tool>(params)` helper (a thin wrapper over \
the same callTool, named from the live catalog). Snippets are discoverable \
through `codemode.search` and `codemode.describe`; run them with \
`codemode.run(\"<snippet>\", input)`.

`codemode.batch(jobs)` runs independent calls concurrently and never rejects: \
pass an array of thunks (`() => codemode.x.y(...)`) or already-started calls, and \
it resolves to `{ ok: [{ i, value }], failed: [{ i, error }], all_ok }` once \
every job has settled. Prefer it over `Promise.all([...])` for fan-out — \
`Promise.all` rejects on the first failure and discards every other in-flight \
result; `codemode.batch` never does.

`codemode.readResource(uri)` reads an upstream MCP resource through the same \
route and caller scope as the current Code Mode run. It returns the MCP \
`ReadResourceResult` object with a `contents` array.

Code Mode has a bounded wall-clock budget. For workflows with many mutating \
calls, use small bounded batches, preserve stable idempotency keys, and inspect \
completed results before retrying a timed-out batch; earlier calls may already \
have committed.

`codemode.step(name, fn)` executes `fn` in the current run, then buffers a bounded, \
redacted result for Labby's best-effort append-only journal. There is no public \
resume or replay operation, and a successful Code Mode response does not prove \
that the detached journal flush has completed.

```ts
// codemode.<upstream>.<tool>() helpers are auto-generated from the live catalog.
// Use codemode.search() / codemode.describe() for compact docs, and callTool for
// dynamic ids.
// Keep the final execution return within the configured envelope budget; project,
// filter, or slice large results before returning.
declare function callTool<T = unknown>(
  id: `${string}::${string}`,
  params: Record<string, unknown>
): Promise<T>;
```

Successful return: the upstream tool's structuredContent if present; otherwise all \
text blocks are joined and parsed as JSON when possible. Mixed/non-text results retain \
the MCP result shape.

Reduce before returning. Do not return a large upstream response raw.

BAD:
```ts
return await callTool(id, params);
```

GOOD — project object fields and slice arrays:
```ts
const r = await callTool(id, params);
return r.items.slice(0, 20).map(({ id, name }) => ({ id, name }));
```

GOOD — filter to the evidence the caller needs:
```ts
const r = await callTool(id, params);
return { failures: r.items.filter(item => item.ok === false) };
```

Error handling:
```ts
try {
  return await callTool(id, params);
} catch (e) {
  const error = JSON.parse(String(e.message));
  // Inspect: kind, origin, recovery, side_effects, cause, evidence.
  // Follow recovery.guidance. Avoid unchanged retries when side_effects is
  // possible/unknown or recovery.same_arguments is discouraged/never.
}
```
A completed MCP tool failure uses `origin: \"tool_execution\"`; inspect its preserved \
`cause` and `evidence`, revise the call, and retry when appropriate. Transport or \
rate-limit failures generally recommend retrying later. Permission, authentication, \
and unknown-target failures require changing state or rediscovering first.

A failed callTool rejects only its own promise — the run continues, so catch it and \
proceed. For catch-and-continue fan-out, prefer `Promise.allSettled` so every call \
settles before you return.

Scope: `codemode_read` accepts `lab:read`, `lab`, or `lab:admin`; `codemode` and \
`codemode_ui` require `lab` or `lab:admin`.

Results are capped to the configured envelope budget (default 24 KB / 6000 tokens). \
Oversized results are replaced with a truncation marker containing `truncated`, \
`original_size`, `original_tokens`, `preview`, and `next_action`. Reduce data inside \
the sandbox before returning — that is the point of Code Mode.

Budget:
- Time: a 30 s wall-clock timeout bounds the whole run. Split work across \
calls or reduce local computation if the `timeout` kind is returned.
- Tool calls: default 512 `callTool` calls per run, configurable by the host up \
to 2048. Extra tool calls reject with `call_budget_exceeded`.
- Memory: 64 MiB heap limit enforced by the QuickJS runtime. Reduce the data \
processed inside the sandbox if the runner exits with `server_error`.
- Stack: QuickJS enforces a native stack depth limit; avoid deep recursion.
- The only recoverable budget kind is `timeout` — retry with a smaller payload \
or split into multiple `codemode` calls.

Lab actions (`lab::*` tool IDs) are not available in Code Mode. For Lab built-in \
actions, use the native Lab service tools instead of Code Mode.";

pub(crate) const CODE_MODE_DESCRIPTION_MAX_BYTES: usize = 8192;

fn code_mode_call_metrics_json(calls: &[CodeModeExecutedCall]) -> String {
    let calls = calls
        .iter()
        .map(|call| {
            let (namespace, tool) = call.id.split_once("::").unwrap_or(("", call.id.as_str()));
            serde_json::json!({
                "id": call.id,
                "namespace": namespace,
                "tool": tool,
                "ok": call.ok,
                "elapsed_ms": call.elapsed_ms,
                "error_kind": call.error_kind,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&calls).unwrap_or_else(|_| "[]".to_string())
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeModeUpstreamDescription {
    pub(crate) name: String,
    pub(crate) hint: Option<String>,
}

fn dynamic_code_mode_description(upstreams: &[CodeModeUpstreamDescription]) -> String {
    let mut out = format!(
        "{}\n\n## Available upstream namespaces\n\n",
        CODE_MODE_DESCRIPTION_BODY.trim_end()
    );
    if upstreams.is_empty() {
        out.push_str("- none currently configured");
        return out;
    }

    for upstream in upstreams {
        match upstream
            .hint
            .as_deref()
            .and_then(labby_runtime::gateway_config::normalize_code_mode_hint)
        {
            Some(hint) => {
                out.push_str(&format!("- `{}` -- {}\n", upstream.name, hint));
            }
            None => {
                out.push_str(&format!("- `{}`\n", upstream.name));
            }
        }
    }
    out.trim_end().to_string()
}

#[must_use]
#[cfg(test)]
pub(crate) fn code_mode_description(upstreams: &[CodeModeUpstreamDescription]) -> String {
    code_mode_description_with_suffix(upstreams, "")
}

/// Compose and cap the final model-visible Code Mode tool description.
///
/// Hosts commonly snapshot the complete `Tool` JSON, so callers must pass all
/// stable tool-specific guidance here instead of appending text after the byte
/// cap has been applied. On overflow, the beginning of the protocol contract
/// and the beginning of the suffix are retained at UTF-8 boundaries.
#[must_use]
pub(crate) fn code_mode_description_with_suffix(
    upstreams: &[CodeModeUpstreamDescription],
    suffix: &str,
) -> String {
    const SEPARATOR: &str = "\n\n";
    const TRUNCATION_NOTE: &str =
        "\n\n[description truncated; use codemode.search for live details]";
    const SUFFIX_PREFIX_MAX_BYTES: usize = 512;

    let body = dynamic_code_mode_description(upstreams);
    let body = body.as_str();
    let suffix = suffix.trim();
    if suffix.is_empty() {
        return utf8_prefix(body, CODE_MODE_DESCRIPTION_MAX_BYTES).to_string();
    }

    if body.len() + SEPARATOR.len() + suffix.len() <= CODE_MODE_DESCRIPTION_MAX_BYTES {
        return format!("{body}{SEPARATOR}{suffix}");
    }

    let suffix = utf8_prefix(suffix, SUFFIX_PREFIX_MAX_BYTES);
    let reserved = SEPARATOR.len() + suffix.len() + TRUNCATION_NOTE.len();
    let body_budget = CODE_MODE_DESCRIPTION_MAX_BYTES.saturating_sub(reserved);
    let body = utf8_prefix(body, body_budget);
    format!("{body}{SEPARATOR}{suffix}{TRUNCATION_NOTE}")
}

pub(crate) fn string_array_arg(
    args: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Vec<String>, DispatchToolError> {
    let Some(value) = args.get(key) else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| DispatchToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("`{key}` must be an array of strings when provided"),
    })?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| DispatchToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message: format!("`{key}` entries must be strings"),
                })
        })
        .collect()
}

pub(crate) fn code_arg(
    args: &JsonObject,
    max_source_bytes: usize,
) -> Result<&str, DispatchToolError> {
    let code = args.get("code").and_then(Value::as_str).unwrap_or_default();
    if code.trim().is_empty() {
        return Err(DispatchToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "code must not be empty".to_string(),
        });
    }
    let max_source_bytes = max_source_bytes.min(MAX_SOURCE_BYTES);
    if code.len() > max_source_bytes {
        return Err(DispatchToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("code exceeds max length {max_source_bytes} bytes"),
        });
    }
    Ok(code)
}

fn route_scoped_capability_filter(
    args: &JsonObject,
    route_allowed: Option<&BTreeSet<String>>,
) -> Result<ToolScope, DispatchToolError> {
    let requested_upstreams = string_array_arg(args, "upstreams")?;
    if let Some(allowed) = route_allowed
        && requested_upstreams
            .iter()
            .any(|name| !allowed.contains(name))
    {
        return Err(DispatchToolError::Sdk {
            sdk_kind: "route_scope_denied".to_string(),
            message: "Code Mode requested an upstream outside this protected route scope"
                .to_string(),
        });
    }

    let tools = string_array_arg(args, "tools")?;
    let Some(allowed) = route_allowed else {
        return Ok(ToolScope::new(requested_upstreams, tools));
    };
    let filter = if requested_upstreams.is_empty() {
        ToolScope::scoped_namespaces(allowed.iter().cloned().collect(), tools)
    } else {
        ToolScope::scoped_namespaces(requested_upstreams, tools)
    };
    Ok(filter)
}

impl LabMcpServer {
    /// `codemode` gateway tool branch. Self-returns.
    pub(crate) async fn call_tool_codemode_impl(
        &self,
        service: &str,
        args: &JsonObject,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let started = Instant::now();
        let input_tokens = estimate_tokens_args(args);
        let subject = self.request_subject_log_tag(context);
        let actor_key = self.request_actor_key(context);
        let auth = auth_context_from_extensions(&context.extensions);
        let read_only = service == CODE_MODE_READ_TOOL_NAME;
        let scope_allowed = if read_only {
            code_mode_read_scope_allowed(auth)
        } else {
            tool_execute_scope_allowed(auth)
        };
        if !scope_allowed {
            let required_scopes = if read_only {
                vec![
                    "lab:read".to_string(),
                    "lab".to_string(),
                    "lab:admin".to_string(),
                ]
            } else {
                vec!["lab".to_string(), "lab:admin".to_string()]
            };
            let err = DispatchToolError::Forbidden {
                message: format!(
                    "{service} requires one of scopes: {}",
                    required_scopes.join(", ")
                ),
                required_scopes,
            };
            tracing::warn!(
                surface = "mcp",
                service = %service,
                action = "call_tool",
                subject,
                actor_key,
                actor_label = subject,
                agent_kind = "agent",
                elapsed_ms = started.elapsed().as_millis(),
                input_tokens,
                kind = "forbidden",
                "gateway codemode denied by scope"
            );
            let env = tool_error_envelope(service, "call_tool", &err);
            return Ok(error_result_from_envelope(env));
        }
        let Some(manager) = &self.gateway_manager else {
            let envelope = build_error(
                service,
                "call_tool",
                "unknown_tool",
                "codemode is not enabled",
            );
            return Ok(error_result_from_envelope(envelope));
        };
        if !self.route_team_credentials_current().await {
            let envelope = build_error(
                service,
                "call_tool",
                "forbidden",
                "Gateway credential is unavailable",
            );
            return Ok(error_result_from_envelope(envelope));
        }
        let config = manager.code_mode_config().await;
        let max_source_bytes = config.max_source_bytes.min(MAX_SOURCE_BYTES);
        let code = match code_arg(args, max_source_bytes) {
            Ok(code) => code,
            Err(err) => {
                let env = build_error_extra(
                    service,
                    "call_tool",
                    err.kind(),
                    &err.to_string(),
                    &serde_json::json!({ "param": "code" }),
                );
                return Ok(error_result_from_envelope(env));
            }
        };
        let capability_filter =
            match route_scoped_capability_filter(args, self.route_scope.allowed_upstreams()) {
                Ok(filter) => filter,
                Err(err) => {
                    let env = tool_error_envelope(service, "call_tool", &err);
                    return Ok(error_result_from_envelope(env));
                }
            };
        let capability_filter = if read_only {
            capability_filter.read_only()
        } else {
            capability_filter
        };
        let code_hash = hash_arguments(&Value::String(code.to_string()));
        // V4: random component so a crashing/restarting host can never mint a
        // colliding id (mirror runtime.ts:360 `exec_<ts>_<uuid>`).
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let execution_id = format!("exec_{now_ms:016}_{}", ulid::Ulid::new());
        let mut step_buffer_guard =
            StepBufferDropGuard::new(Arc::clone(manager), execution_id.clone());
        let capability_filter_fingerprint = capability_filter.fingerprint();
        tracing::info!(
            surface = "mcp",
            service = CODE_MODE_SERVICE,
            code_mode_tool = %service,
            action = "call_tool",
            subject,
            actor_key,
            actor_label = subject,
            agent_kind = "agent",
            code_hash = %code_hash,
            input_tokens,
            "gateway codemode start"
        );

        let caller = match auth {
            None => CodeModeCaller::TrustedLocal,
            Some(auth) => {
                let capabilities = code_mode_capabilities_for_scopes(&auth.scopes);
                let sub = self
                    .route_oauth_subject(
                        self.request_subject(context)
                            .map(std::borrow::Cow::Borrowed),
                    )
                    .map(std::borrow::Cow::into_owned);
                if let (Some(provider_token), Some(provider_request_id)) = (
                    self.request_host_provider_token(context),
                    self.request_host_provider_request_id(context),
                ) {
                    CodeModeCaller::ScopedHostProvider {
                        capabilities,
                        sub,
                        provider_token: provider_token.to_string(),
                        provider_request_id: provider_request_id.to_string(),
                    }
                } else {
                    CodeModeCaller::Scoped { capabilities, sub }
                }
            }
        };

        // Per-run caller identity stamped onto journal rows at the flush
        // boundary (captured once, not per step). Fingerprint is cloned here
        // because `capability_filter` is moved into `execute()` below.
        let journal_owner = JournalOwner {
            actor_key: actor_key.map(ToOwned::to_owned),
            route_scope: self.route_scope.label(),
            capability_filter_fingerprint: Some(capability_filter_fingerprint.clone()),
        };

        let broker = CodeModeBroker::new(Some(manager.as_ref()));
        let before = self.snapshot_tool_catalog_for_request(context).await;
        let dedup_key = format!(
            "{}|{}|{}|{}|{}",
            self.route_scope.label(),
            service,
            actor_key.unwrap_or(subject.as_str()),
            capability_filter_fingerprint,
            code_hash,
        );
        let broker_result = match begin_code_mode_execution(dedup_key, execution_id.clone()) {
            InflightCodeModeRole::Follower(entry) => {
                tracing::info!(
                    surface = "mcp",
                    service = CODE_MODE_SERVICE,
                    code_mode_tool = %service,
                    action = "call_tool.deduplicate",
                    subject,
                    actor_key,
                    actor_label = subject,
                    agent_kind = "agent",
                    code_hash = %code_hash,
                    leader_execution_id = %entry.leader_execution_id,
                    "joining identical in-flight Code Mode execution"
                );
                await_code_mode_execution(entry).await
            }
            InflightCodeModeRole::Leader(leader) => {
                let result = broker
                    .execute(
                        code,
                        caller,
                        self.code_mode_surface(),
                        config,
                        capability_filter,
                        Some(Arc::<str>::from(execution_id.as_str())),
                    )
                    .await;
                leader.complete(&result);
                result
            }
        };
        let mut response = match broker_result {
            Ok(response) => {
                let after = self.snapshot_tool_catalog_for_request(context).await;
                self.notify_catalog_changes(after.changes_since(&before), SOURCE_MCP_CALL_CODEMODE)
                    .await;
                response
            }
            Err(err) => {
                let after = self.snapshot_tool_catalog_for_request(context).await;
                self.notify_catalog_changes(after.changes_since(&before), SOURCE_MCP_CALL_CODEMODE)
                    .await;
                let calls = err.calls().to_vec();
                let code_mode_calls = code_mode_call_metrics_json(&calls);
                let error_kind = err.kind().to_string();
                let elapsed_ms = started.elapsed().as_millis();
                tracing::warn!(
                    surface = "mcp",
                    service = CODE_MODE_SERVICE,
                    code_mode_tool = %service,
                    action = "call_tool",
                    subject,
                    actor_key,
                    actor_label = subject,
                    agent_kind = "agent",
                    code_hash = %code_hash,
                    call_count = calls.len(),
                    code_mode_calls = %code_mode_calls,
                    elapsed_ms,
                    input_tokens,
                    output_tokens = 0,
                    kind = error_kind.as_str(),
                    "gateway codemode failed"
                );
                let call_error = err.into_call_error();
                spawn_code_mode_persistence(
                    manager,
                    execution_id.clone(),
                    journal_owner.clone(),
                    CodeModeHistoryEntry {
                        execution_id: Some(execution_id.clone()),
                        seq: 0,
                        route_scope: self.route_scope.label(),
                        kind: CodeModeHistoryKind::Execute,
                        ok: false,
                        elapsed_ms,
                        input_tokens: Some(input_tokens),
                        output_tokens: Some(0),
                        error_kind: Some(error_kind.clone()),
                        calls: calls.clone(),
                        match_count: None,
                    },
                    None,
                );
                step_buffer_guard.disarm();
                let env = code_mode_error_envelope(service, "call_tool", &call_error);
                // Failures carry a structured trace too — otherwise the inline
                // inspector renders nothing for a failed run (the error text
                // block is host-consumed, not widget-consumed).
                let structured = serde_json::json!({
                    "kind": "code_mode_execute_trace",
                    "call_count": calls.len(),
                    "calls": calls,
                    "error_kind": error_kind,
                    "error": call_error,
                    "execution_id": execution_id,
                    "elapsed_ms": elapsed_ms as u64,
                    "input_tokens": input_tokens as u64,
                    "output_tokens": 0,
                    "result_shape": { "type": "undefined" },
                    // `logs_count` is required by `code_mode_trace_output_schema`;
                    // this is internal consistency for trace consumers (the
                    // inline inspector reads structuredContent on both paths),
                    // not spec conformance — the trace is `isError` and exempt.
                    "logs_count": 0,
                });
                let mut result = CallToolResult::error(vec![ContentBlock::text(env.to_string())]);
                result.structured_content = Some(structured);
                return Ok(result);
            }
        };
        step_buffer_guard.disarm();
        response.execution_id = Some(execution_id.clone());

        // Preserve the last upstream widget both in the structured trace and
        // on the final CallToolResult. The result metadata lets the host render
        // the actual nested MCP App; trace-only capture leaves the app session
        // open but unreachable from the client.
        let captured_resource_uri = response.ui.as_ref().and_then(|ui| {
            ui.ui_meta
                .get("resourceUri")
                .and_then(|value| value.as_str())
        });
        if response.ui.is_some() {
            tracing::info!(
                surface = "mcp",
                service = CODE_MODE_SERVICE,
                code_mode_tool = %service,
                action = "mcp_app.capture",
                subject,
                actor_key,
                actor_label = subject,
                agent_kind = "agent",
                resource_uri = captured_resource_uri.unwrap_or("<unknown>"),
                "captured upstream MCP App widget metadata in codemode trace"
            );
        }
        let output = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
        let output_tokens = estimate_tokens(&output);
        let is_admin = auth.is_none_or(|auth| auth.scopes.iter().any(|scope| scope == "lab:admin"));
        let source = if is_admin && code.len() <= max_source_bytes {
            Some(CodeModeExecutionSource {
                execution_id: execution_id.clone(),
                created_at_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as i64)
                    .unwrap_or_default(),
                actor_key: actor_key.map(ToOwned::to_owned),
                is_admin,
                route_scope: self.route_scope.label(),
                surface: self.code_mode_surface(),
                capability_filter_fingerprint,
                code: code.to_string(),
            })
        } else {
            None
        };
        spawn_code_mode_persistence(
            manager,
            execution_id.clone(),
            journal_owner,
            CodeModeHistoryEntry {
                execution_id: Some(execution_id.clone()),
                seq: 0,
                route_scope: self.route_scope.label(),
                kind: CodeModeHistoryKind::Execute,
                ok: true,
                elapsed_ms: started.elapsed().as_millis(),
                input_tokens: Some(input_tokens),
                output_tokens: Some(output_tokens),
                error_kind: None,
                calls: response.calls.clone(),
                match_count: None,
            },
            source,
        );
        let mut structured = code_mode_execute_trace(&response);
        if let Some(object) = structured.as_object_mut() {
            object.insert(
                "execution_id".to_string(),
                Value::String(execution_id.clone()),
            );
            object.insert(
                "elapsed_ms".to_string(),
                Value::from(started.elapsed().as_millis() as u64),
            );
            object.insert("input_tokens".to_string(), Value::from(input_tokens as u64));
            object.insert(
                "output_tokens".to_string(),
                Value::from(output_tokens as u64),
            );
        }
        let trace_result_type = structured
            .get("result_shape")
            .and_then(|shape| shape.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let trace_has_result = structured.get("result").is_some();
        let shape_truncated = response
            .result_shaping
            .as_ref()
            .map(|shape| shape.truncated)
            .unwrap_or(false);
        let legacy_truncated = response
            .result
            .as_ref()
            .and_then(|result| result.get("truncated"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let truncated = shape_truncated || legacy_truncated;
        let result_shape_policy = response
            .result_shaping
            .as_ref()
            .and_then(|shape| serde_json::to_value(shape.policy).ok())
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "legacy".to_string());
        tracing::info!(
            surface = "mcp",
            service = CODE_MODE_SERVICE,
            code_mode_tool = %service,
            action = "call_tool",
            subject,
            actor_key,
            actor_label = subject,
            agent_kind = "agent",
            code_hash = %code_hash,
            call_count = response.calls.len(),
            code_mode_calls = %code_mode_call_metrics_json(&response.calls),
            artifact_writes = response.artifacts.len(),
            truncated,
            result_shape_policy,
            elapsed_ms = started.elapsed().as_millis(),
            input_tokens,
            output_tokens,
            trace_has_result,
            trace_result_type,
            captured_ui_resource_uri = captured_resource_uri.unwrap_or("<none>"),
            "gateway codemode ok"
        );
        Ok(code_mode_result(
            output,
            structured,
            &response,
            self.route_scope.exposes_resources(),
        ))
    }
}

fn code_mode_capabilities_for_scopes(scopes: &[String]) -> CodeModeCallerCapabilities {
    let is_admin = scopes.iter().any(|scope| scope == "lab:admin");
    CodeModeCallerCapabilities {
        can_read: scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin")),
        can_execute: scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin")),
        can_use_snippets: is_admin,
        is_admin,
    }
}

fn call_result_with_structured(
    text: String,
    structured: Value,
    ui_meta: Option<MetaObject>,
) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(structured);
    result.meta = ui_meta;
    result
}

fn code_mode_result(
    text: String,
    structured: Value,
    response: &CodeModeExecutionResponse,
    expose_resource_ui: bool,
) -> CallToolResult {
    let ui_meta = expose_resource_ui
        .then_some(response.ui.as_ref())
        .flatten()
        .map(|ui| {
            MetaObject(serde_json::Map::from_iter([(
                "ui".to_string(),
                ui.ui_meta.clone(),
            )]))
        });
    call_result_with_structured(text, structured, ui_meta)
}

#[cfg(test)]
mod tests;
