//! Leaf helpers for the upstream pool: config knobs, error classification,
//! naming, redaction, the cached-summary snapshot type, and the shared
//! prompt/resource merge/rewrite helpers.
//!
//! These are pure, dependency-light building blocks shared across the `pool/`
//! child modules. They are declared `pub(super)` so the parent `pool` module
//! (and its descendants) can use them unqualified via `use helpers::*;`.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rmcp::model::{CallToolResult, Prompt, ReadResourceResult, Resource, ResourceContents};
use serde_json::Value;

use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::redact::{redact_stdio_value, redact_url};

use super::super::types::UpstreamTool;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpstreamCachedSummary {
    pub discovered_tool_count: usize,
    pub exposed_tool_count: usize,
    pub discovered_resource_count: usize,
    pub exposed_resource_count: usize,
    pub discovered_prompt_count: usize,
    pub exposed_prompt_count: usize,
}

/// Per-upstream timeout for initial discovery (`list_tools`).
pub(super) const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
/// Per-service timeout for in-process peer registration and capability probing.
pub(super) const IN_PROCESS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
/// Default cap for bulk discovery and concurrent lazy reprobes. Stdio upstreams
/// can fan out into several child processes, so unbounded connection attempts
/// can exhaust the container PID limit before any single upstream is unhealthy.
pub(super) const DEFAULT_UPSTREAM_DISCOVERY_CONCURRENCY: usize = 3;
/// Per-request timeout for upstream tool/resource/prompt RPCs.
pub(super) const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Deadline for a *relayed* upstream tool call (the elicitation-relay path).
///
/// Five minutes, not 30 seconds: a relayed call blocks while the upstream's
/// `elicitation/create` is forwarded to the downstream agent and answered by a
/// human. The pool's `relay_timeout` field defaults to this; the binary
/// overrides it from `upstream_relay_timeout_ms`. See `pool/relay.rs`.
pub(super) const DEFAULT_RELAY_TIMEOUT: Duration = Duration::from_secs(300);
pub(super) const STDIO_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
/// Idle TTL for per-`(upstream, subject)` cached connections.
///
/// A connection that has not been used for this long will be evicted from
/// the subject-connection cache on the next access for its key (P-C1), or by
/// the background sweep task ([`SUBJECT_CONN_SWEEP_INTERVAL`]).
pub(super) const SUBJECT_CONN_IDLE_TTL: Duration = Duration::from_secs(300);

/// Interval at which the background subject-connection sweep runs (P-H2).
///
/// Each tick evicts idle-TTL-expired `subject_connections` entries (shutting
/// their peers down cleanly) and prunes orphan `subject_connect_locks`. Set to
/// the idle TTL so a leaked-but-idle connection lives at most ~2× the TTL.
pub(super) const SUBJECT_CONN_SWEEP_INTERVAL: Duration = SUBJECT_CONN_IDLE_TTL;

/// Hard upper bound on the number of live per-`(upstream, subject)` cached
/// connections (P-H2).
///
/// In an OAuth multi-user deployment each unique subject opens one live peer
/// (one stdio child / one HTTP keep-alive + FD). Without a cap a burst of unique
/// subjects could exhaust file descriptors before the idle TTL sweep reclaims
/// them. When an insert would exceed this cap the least-recently-used entries
/// are evicted (and shut down cleanly) down to the cap first.
pub(super) const SUBJECT_CONN_MAX_ENTRIES: usize = 256;

/// Default maximum response size from upstream servers (10 MB).
pub(super) const DEFAULT_MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

pub(super) const IN_PROCESS_PEER_BUFFER_BYTES: usize = 256 * 1024;
pub(super) const AUTH_FAILURE_REPROBE_ATTEMPT_FLOOR: u32 = 5;

pub fn in_process_upstream_name(service_name: &str) -> String {
    format!("__in_process__{service_name}")
}

/// A `Write` sink that counts bytes without allocating.
///
/// Used by `estimate_response_size` so we measure JSON size by streaming
/// through `serde_json::to_writer` instead of building the full string.
struct ByteCounter(usize);

impl Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Estimate the serialized size of a `CallToolResult`.
///
/// Uses `serde_json::to_writer` with a counting sink — no allocation of the
/// full serialized string.  Not exact (ignores transport framing) but sufficient
/// for the size cap guard.
pub(super) fn estimate_response_size(result: &CallToolResult) -> usize {
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, result).map_or(0, |()| counter.0)
}

/// Estimate the serialized size of a `ReadResourceResult`.
///
/// Mirrors `estimate_response_size` but for resource reads — avoids allocating
/// the full JSON string just to measure it.
pub(super) fn estimate_resource_response_size(result: &ReadResourceResult) -> usize {
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, result).map_or(0, |()| counter.0)
}

/// Cached max response size (resolved once from env on first call).
///
/// `LAB_UPSTREAM_MAX_RESPONSE_BYTES` is read at most once per process.
/// Tests that need a different cap should use `max_response_bytes_override`
/// (cfg(test) only) to replace the cached value before the first call.
static MAX_RESPONSE_BYTES_CACHE: OnceLock<usize> = OnceLock::new();

/// Return the max upstream response size.
///
/// Reads `LAB_UPSTREAM_MAX_RESPONSE_BYTES` once and caches the result for the
/// lifetime of the process.  Subsequent calls return the cached value with no
/// syscall overhead.
pub(super) fn max_response_bytes() -> usize {
    *MAX_RESPONSE_BYTES_CACHE.get_or_init(|| {
        std::env::var("LAB_UPSTREAM_MAX_RESPONSE_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_RESPONSE_BYTES)
    })
}

/// Override the cached max-response-bytes value for tests.
///
/// Must be called before `max_response_bytes()` is first invoked in the test
/// process.  If the cache is already initialised the call is a no-op — use a
/// fresh process (e.g. a dedicated `#[test]` binary shard) if you need a
/// different value after first use.
#[cfg(test)]
pub(super) fn max_response_bytes_override(value: usize) -> bool {
    MAX_RESPONSE_BYTES_CACHE.set(value).is_ok()
}

pub(super) fn classify_upstream_error(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("auth required")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("invalid_token")
        || lower.contains("oauth")
    {
        "auth_failed"
    } else if lower.contains("bearer")
        || lower.contains("token")
        || lower.contains("api key")
        || lower.contains("api_key")
    {
        "auth_required"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "timeout"
    } else if lower.contains("dns") || lower.contains("name or service not known") {
        "dns_error"
    } else if lower.contains("connection refused") {
        "connection_refused"
    } else {
        "connection_error"
    }
}

pub(super) fn auth_error_should_backoff_aggressively(kind: &str) -> bool {
    matches!(kind, "auth_failed" | "auth_required")
}

pub(super) fn upstream_transport(config: &UpstreamConfig) -> &'static str {
    if config.url.as_deref().is_some_and(is_websocket_url) {
        "websocket"
    } else if config.url.is_some() {
        "http"
    } else {
        "stdio"
    }
}

pub(crate) fn upstream_discovery_concurrency() -> usize {
    std::env::var("LAB_UPSTREAM_DISCOVERY_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_UPSTREAM_DISCOVERY_CONCURRENCY)
}

pub(super) fn is_websocket_url(url: &str) -> bool {
    matches!(
        url::Url::parse(url)
            .ok()
            .map(|parsed| parsed.scheme().to_string())
            .as_deref(),
        Some("ws" | "wss")
    )
}

pub(super) fn upstream_name_is_uri_safe(name: &str) -> bool {
    !name.contains('/') && !name.contains('?') && !name.contains('#')
}

pub fn redact_resource_uri_for_logging(uri: &str) -> &str {
    let cut = uri.find('?').or_else(|| uri.find('#')).unwrap_or(uri.len());
    &uri[..cut]
}

pub(super) fn upstream_target_redacted(config: &UpstreamConfig) -> String {
    // SECURITY: Never log raw URLs or command fragments without central redaction.
    match &config.url {
        Some(url_str) => redact_url(url_str),
        None => config
            .command
            .as_deref()
            .map(redact_stdio_value)
            .or_else(|| Some("<missing>".to_string()))
            .expect("static fallback is present"),
    }
}

/// Namespace an upstream prompt name with its owning upstream, mirroring how
/// `rewrite_resource_uri` prefixes resources. This keeps prompts with the same
/// bare name from different upstreams distinct (e.g. two `quick_start` prompts
/// become `rustarr/quick_start` and `sonarr/quick_start`).
pub(super) fn prefixed_upstream_prompt_name(upstream_name: &str, prompt_name: &str) -> String {
    format!("{upstream_name}/{prompt_name}")
}

/// Reverse `prefixed_upstream_prompt_name` for forwarding a `prompts/get` to the
/// upstream, which only knows the bare prompt name. The owning `upstream_name`
/// is already resolved by the caller, so strip exactly that prefix; fall back to
/// the input unchanged if it isn't prefixed (e.g. legacy/unprefixed callers).
pub(super) fn bare_upstream_prompt_name<'a>(upstream_name: &str, prompt_name: &'a str) -> &'a str {
    prompt_name
        .strip_prefix(&format!("{upstream_name}/"))
        .unwrap_or(prompt_name)
}

/// Merge upstream prompts deterministically and return the winning owner for each prompt.
///
/// Every prompt is namespaced by its owning upstream (see
/// `prefixed_upstream_prompt_name`), so cross-upstream name collisions cannot
/// occur. The `seen_names` guard below now only catches the degenerate case of a
/// single upstream advertising the same prompt name twice.
pub(super) fn merge_upstream_prompts(
    builtin_names: &[&str],
    mut upstream_prompts: Vec<(String, Vec<Prompt>)>,
) -> (Vec<Prompt>, HashMap<String, String>) {
    upstream_prompts.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    let mut prompts = Vec::new();
    let mut owners = HashMap::new();
    let mut seen_names: std::collections::HashSet<String> = builtin_names
        .iter()
        .map(|name| (*name).to_string())
        .collect();

    for (upstream_name, upstream_prompts) in upstream_prompts {
        for mut prompt in upstream_prompts {
            let prompt_name = prefixed_upstream_prompt_name(&upstream_name, &prompt.name);
            if seen_names.insert(prompt_name.clone()) {
                prompt.name = prompt_name.clone();
                owners.insert(prompt_name, upstream_name.clone());
                prompts.push(prompt);
            } else {
                tracing::warn!(
                    upstream = %upstream_name,
                    prompt = %prompt_name,
                    "duplicate prompt name encountered while merging upstream prompts"
                );
            }
        }
    }

    (prompts, owners)
}

/// Normalize a proxied resource read so its contents use the gateway URI.
pub(super) fn normalize_resource_result_uri(
    mut result: ReadResourceResult,
    gateway_uri: &str,
) -> ReadResourceResult {
    for content in &mut result.contents {
        match content {
            ResourceContents::TextResourceContents { uri, .. }
            | ResourceContents::BlobResourceContents { uri, .. } => {
                *uri = gateway_uri.to_string();
            }
        }
    }

    result
}

/// Rewrite an upstream resource's URI to the gateway-prefixed form.
///
/// Strips any embedded upstream name from existing `lab://upstream/…` URIs
/// and re-prefixes with the caller's `upstream_name`.
pub(super) fn rewrite_resource_uri(resource: &mut Resource, upstream_name: &str) {
    let bare_uri = bare_upstream_resource_uri(&resource.uri);
    resource.uri = format!("lab://upstream/{upstream_name}/{bare_uri}");
}

pub(super) fn bare_upstream_resource_uri(uri: &str) -> &str {
    uri.strip_prefix("lab://upstream/")
        .and_then(|rest| rest.split_once('/').map(|x| x.1).or(Some(rest)))
        .unwrap_or(uri)
}

pub(super) fn cached_upstream_tool(
    tool: rmcp::model::Tool,
    upstream_name: &Arc<str>,
) -> (String, UpstreamTool) {
    let name = tool.name.to_string();
    // Absent or null annotations.destructiveHint → false (conservative: only
    // treat as destructive when explicitly set to true by the upstream server).
    let destructive = tool
        .annotations
        .as_ref()
        .and_then(|a| a.destructive_hint)
        .unwrap_or(false);
    (
        name,
        UpstreamTool {
            input_schema: (!tool.input_schema.is_empty())
                .then(|| Value::Object((*tool.input_schema).clone())),
            output_schema: tool
                .output_schema
                .as_ref()
                .filter(|schema| !schema.is_empty())
                .map(|schema| Value::Object((**schema).clone())),
            tool,
            upstream_name: Arc::clone(upstream_name),
            destructive,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_upstream_config() -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: None,
            bearer_token_env: None,
            command: None,
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }
    }

    #[test]
    fn upstream_target_redacts_url_credentials_and_sensitive_query_values() {
        let mut config = test_upstream_config();
        config.url = Some("https://user:pass@example.com/mcp?token=secret&mode=1#frag".into());

        assert_eq!(
            upstream_target_redacted(&config),
            "https://example.com/mcp?token=[redacted]&mode=1"
        );
    }

    #[test]
    fn upstream_target_redacts_stdio_secret_flags() {
        let mut config = test_upstream_config();
        config.command = Some("--api-key=secret".into());

        assert_eq!(upstream_target_redacted(&config), "--api-key=[redacted]");
    }
}
