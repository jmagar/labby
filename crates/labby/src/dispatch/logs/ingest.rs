//! Ingest pipeline: raw event → redact → enrich → store + stream.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;

use super::store::LogStore;
use super::stream::StreamHub;
use super::types::{LogEvent, LogLevel, RawLogEvent, Subsystem, Surface};
use crate::dispatch::error::ToolError;

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Counters for ingest-queue health. Only drops are exported via `stats`;
/// accepted-event totals would be redundant with the store row count.
pub struct IngestCounters {
    dropped: AtomicU64,
    saturated: AtomicU64,
}

impl IngestCounters {
    #[must_use]
    pub fn new() -> Self {
        Self {
            dropped: AtomicU64::new(0),
            saturated: AtomicU64::new(0),
        }
    }

    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn record_drop(&self) -> u64 {
        self.dropped.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn record_saturation(&self) -> u64 {
        self.saturated.fetch_add(1, Ordering::Relaxed) + 1
    }
}

impl Default for IngestCounters {
    fn default() -> Self {
        Self::new()
    }
}

pub struct IngestHandle {
    tx: Option<mpsc::Sender<RawLogEvent>>,
    /// Inline write-through path used by `submit` (await). The store and hub
    /// are owned by the LogSystem and shared with the writer task, so this is
    /// a cheap Arc clone.
    #[cfg_attr(not(test), allow(dead_code))]
    #[allow(dead_code)]
    inline: Option<(Arc<LogStore>, Arc<StreamHub>)>,
    counters: Arc<IngestCounters>,
}

impl IngestHandle {
    #[allow(dead_code)]
    pub async fn submit(&self, raw: RawLogEvent) -> Result<(), ToolError> {
        // Await-path: persist inline so callers see their write before returning.
        // Non-await callers (the tracing layer) use `try_submit` instead.
        let Some((store, hub)) = &self.inline else {
            return Err(ToolError::internal_message(
                "log system is read-only (no ingest writer)",
            ));
        };
        let event = canonicalize(raw);
        store.insert(&event).await?;
        hub.publish(event);
        Ok(())
    }

    pub fn try_submit(&self, raw: RawLogEvent) -> Result<(), ToolError> {
        let Some(tx) = &self.tx else {
            return Err(ToolError::internal_message(
                "log system is read-only (no ingest writer)",
            ));
        };
        match tx.try_send(raw) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                let total_dropped = self.counters.record_drop();
                let saturation_events = self.counters.record_saturation();
                tracing::warn!(
                    target: "labby::dispatch::logs",
                    surface = "logs", service = "ingest", action = "event.drop",
                    kind = "rate_limited",
                    queue_capacity = tx.max_capacity(),
                    queue_remaining = tx.capacity(),
                    total_dropped,
                    saturation_events,
                    "log ingest queue full; event dropped",
                );
                Err(ToolError::Sdk {
                    sdk_kind: "rate_limited".to_string(),
                    message: "log ingest queue is full; event dropped".to_string(),
                })
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                let total_dropped = self.counters.record_drop();
                tracing::warn!(
                    target: "labby::dispatch::logs",
                    surface = "logs", service = "ingest", action = "event.drop",
                    kind = "internal_error",
                    queue_capacity = tx.max_capacity(),
                    total_dropped,
                    "log ingest channel closed; event dropped",
                );
                Err(ToolError::internal_message("log ingest channel closed"))
            }
        }
    }
}

pub fn readonly_handle(counters: Arc<IngestCounters>) -> IngestHandle {
    IngestHandle {
        tx: None,
        inline: None,
        counters,
    }
}

pub fn spawn_writer(
    store: Arc<LogStore>,
    hub: Arc<StreamHub>,
    queue_capacity: usize,
) -> (IngestHandle, Arc<IngestCounters>) {
    let (tx, mut rx) = mpsc::channel::<RawLogEvent>(queue_capacity.max(1));
    let counters = Arc::new(IngestCounters::new());
    let handle = IngestHandle {
        tx: Some(tx),
        inline: Some((Arc::clone(&store), Arc::clone(&hub))),
        counters: Arc::clone(&counters),
    };

    tokio::spawn(async move {
        let mut processed_events: u64 = 0;
        while let Some(raw) = rx.recv().await {
            let event = canonicalize(raw);
            if let Err(err) = store.insert(&event).await {
                tracing::warn!(
                    target: "labby::dispatch::logs",
                    surface = "logs", service = "ingest", action = "event.insert_failed",
                    error = %err, "log store insert failed; event dropped",
                );
                continue;
            }
            processed_events += 1;
            hub.publish(event);
        }
        tracing::warn!(
            target: "labby::dispatch::logs",
            surface = "logs", service = "ingest", action = "writer.exit",
            processed_events,
            "log ingest writer task exited; all senders dropped; log ingest offline",
        );
    });

    (handle, counters)
}

// ── Canonicalization ─────────────────────────────────────────────────────────

fn canonicalize(mut raw: RawLogEvent) -> LogEvent {
    raw.message = redact_secrets(&raw.message);

    let ts = raw.ts.unwrap_or_else(now_ms);
    let level = raw
        .level
        .as_deref()
        .and_then(LogLevel::parse)
        .unwrap_or(LogLevel::Info);
    let subsystem = raw
        .subsystem
        .as_deref()
        .and_then(Subsystem::parse)
        .unwrap_or(Subsystem::CoreRuntime);
    let surface = raw
        .surface
        .as_deref()
        .and_then(Surface::parse)
        .unwrap_or(Surface::CoreRuntime);

    let event_id = uuid::Uuid::new_v4().to_string();
    let fields_json = scrub_json(raw.fields_json);

    LogEvent {
        event_id,
        ts,
        level,
        subsystem,
        surface,
        action: raw.action,
        message: raw.message,
        request_id: raw.request_id,
        session_id: raw.session_id,
        correlation_id: raw.correlation_id,
        trace_id: raw.trace_id,
        span_id: raw.span_id,
        instance: raw.instance,
        auth_flow: raw.auth_flow,
        outcome_kind: raw.outcome_kind,
        fields_json,
        source_kind: raw.source_kind,
        source_node_id: raw.source_node_id,
        source_device_id: raw.source_device_id,
        actor_key: raw.actor_key,
        ingest_path: raw.ingest_path.or_else(|| Some("tracing".to_string())),
        upstream_event_id: raw.upstream_event_id,
    }
}

/// Redact free-form secret tokens from a log message string.
///
/// Covered patterns (all case-sensitive unless noted):
/// - `Bearer <token>` — HTTP Authorization header value
/// - `token=<value>` / `api_key=<value>` / `password=<value>` / `secret=<value>`
///   key=value pairs (case-insensitive key match, value ends at whitespace/,/&)
/// - GitHub tokens: `ghp_…`, `ghs_…`, `gho_…`, `ghu_…`, `ghr_…`
///   (min 10 non-whitespace chars after the prefix)
/// - OpenAI/Anthropic-style keys: `sk-…` (min 20 non-whitespace chars, avoids
///   clipping normal short log words like "sk-node")
///
/// Notably NOT redacted: plain UUIDs — those are correlation/request IDs used
/// throughout observability. Do not add UUID redaction.
fn redact_secrets(msg: &str) -> String {
    // Phase 1: Bearer tokens.
    let msg = redact_bearer_inner(msg);
    // Phase 2: key=value pairs with secret-like key names.
    let msg = redact_key_value_secrets(&msg);
    // Phase 3: well-known opaque token prefixes.
    redact_prefixed_tokens(&msg)
}

/// Replace `Bearer <token>` with `<redacted-auth>`.
fn redact_bearer_inner(msg: &str) -> String {
    // Replace the entire "Bearer <token>" sequence with "<redacted-auth>" so the
    // literal substring "Bearer " does not survive — asserted by
    // tests/logs_dispatch.rs:221.
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(pos) = rest.find("Bearer ") {
        out.push_str(&rest[..pos]);
        out.push_str("<redacted-auth>");
        let after = &rest[pos + "Bearer ".len()..];
        let tok_end = after
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after.len());
        rest = &after[tok_end..];
    }
    out.push_str(rest);
    out
}

/// Redact values in `key=value` pairs where the key looks like a secret.
///
/// Matches case-insensitively. Value ends at the first whitespace, `,`, `&`,
/// `"`, or `'` character.  Only redacts if the value is non-empty.
fn redact_key_value_secrets(msg: &str) -> String {
    const SECRET_KEYS: &[&str] = &[
        "token=",
        "api_key=",
        "apikey=",
        "password=",
        "secret=",
        "credential=",
        "private_key=",
        "auth=",
    ];

    let lower = msg.to_ascii_lowercase();
    let mut replacements: Vec<(usize, usize)> = Vec::new(); // (value_start, value_end) byte ranges

    for needle in SECRET_KEYS {
        let mut search_from = 0usize;
        while let Some(rel) = lower[search_from..].find(needle) {
            let key_end = search_from + rel + needle.len();
            if key_end >= msg.len() {
                break;
            }
            let value_part = &msg[key_end..];
            let val_len = value_part
                .find(|c: char| c.is_whitespace() || matches!(c, ',' | '&' | '"' | '\''))
                .unwrap_or(value_part.len());
            if val_len > 0 {
                replacements.push((key_end, key_end + val_len));
            }
            search_from = key_end + val_len.max(1);
        }
    }

    if replacements.is_empty() {
        return msg.to_string();
    }

    // Sort and apply replacements (they don't overlap by construction).
    replacements.sort_unstable_by_key(|&(start, _)| start);
    let mut out = String::with_capacity(msg.len());
    let mut cursor = 0usize;
    for (start, end) in replacements {
        if start < cursor {
            continue; // overlapping — skip (shouldn't happen)
        }
        out.push_str(&msg[cursor..start]);
        out.push_str("<redacted>");
        cursor = end;
    }
    out.push_str(&msg[cursor..]);
    out
}

/// Redact well-known opaque token prefixes that appear as standalone tokens
/// (surrounded by whitespace, start of string, or end of string).
///
/// GitHub prefixes: `ghp_`, `ghs_`, `gho_`, `ghu_`, `ghr_`
/// OpenAI/Anthropic `sk-` keys (require ≥ 20 chars after `sk-` to avoid
/// false-positives on short strings like `sk-node`).
fn redact_prefixed_tokens(msg: &str) -> String {
    // GitHub personal / server / OAuth / user / refresh tokens.
    const GH_PREFIXES: &[&str] = &["ghp_", "ghs_", "gho_", "ghu_", "ghr_"];
    const GH_MIN_LEN: usize = 10; // chars after the prefix

    // sk- keys: require a longer suffix to avoid false positives.
    const SK_PREFIX: &str = "sk-";
    const SK_MIN_SUFFIX_LEN: usize = 20;

    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;

    'outer: while !rest.is_empty() {
        // Try each GitHub prefix.
        for gh_prefix in GH_PREFIXES {
            if let Some(pos) = rest.find(gh_prefix) {
                // Only redact if preceded by whitespace or start-of-string.
                let preceded_ok = pos == 0
                    || rest
                        .as_bytes()
                        .get(pos - 1)
                        .map(|b| b.is_ascii_whitespace())
                        .unwrap_or(true);
                let after_prefix = &rest[pos + gh_prefix.len()..];
                let tok_len = after_prefix
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(after_prefix.len());
                if preceded_ok && tok_len >= GH_MIN_LEN {
                    out.push_str(&rest[..pos]);
                    out.push_str("<redacted-token>");
                    rest = &after_prefix[tok_len..];
                    continue 'outer;
                }
                // Not a match — advance past this occurrence to avoid looping.
                out.push_str(&rest[..pos + gh_prefix.len()]);
                rest = &rest[pos + gh_prefix.len()..];
                continue 'outer;
            }
        }

        // Try sk- prefix.
        if let Some(pos) = rest.find(SK_PREFIX) {
            let preceded_ok = pos == 0
                || rest
                    .as_bytes()
                    .get(pos - 1)
                    .map(|b| b.is_ascii_whitespace())
                    .unwrap_or(true);
            let after_prefix = &rest[pos + SK_PREFIX.len()..];
            let tok_len = after_prefix
                .find(|c: char| c.is_whitespace())
                .unwrap_or(after_prefix.len());
            if preceded_ok && tok_len >= SK_MIN_SUFFIX_LEN {
                out.push_str(&rest[..pos]);
                out.push_str("<redacted-token>");
                rest = &after_prefix[tok_len..];
                continue;
            }
            // Not a match — advance past this occurrence.
            out.push_str(&rest[..pos + SK_PREFIX.len()]);
            rest = &rest[pos + SK_PREFIX.len()..];
            continue;
        }

        // No match found — emit the rest as-is.
        out.push_str(rest);
        break;
    }

    out
}

fn scrub_json(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, val) in map {
                if looks_secret(&k) {
                    out.insert(k, serde_json::Value::String("<redacted>".into()));
                } else {
                    out.insert(k, scrub_json(val));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(scrub_json).collect())
        }
        serde_json::Value::String(s) => serde_json::Value::String(redact_secrets(&s)),
        other => other,
    }
}

fn looks_secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    if matches!(
        k.as_str(),
        "input_tokens" | "output_tokens" | "total_tokens"
    ) {
        return false;
    }
    k.contains("secret")
        || k.contains("token")
        || k.contains("password")
        || k.contains("api_key")
        || k.contains("apikey")
        || k == "authorization"
        || k.contains("cookie")
        || k.contains("private_key")
        || k.contains("credential")
}

// ── tracing Layer ────────────────────────────────────────────────────────────

/// Tracing layer that forwards events into the installed LogSystem.
///
/// Implementation strategy: visit fields, build a RawLogEvent, and hand off
/// via `LogSystem::try_ingest` (non-blocking). If no system is installed, the
/// event is silently dropped.
pub struct LogIngestLayer;

impl<S> tracing_subscriber::Layer<S> for LogIngestLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();

        // Guard against self-log infinite loop: skip events from the logs subsystem itself.
        if metadata.target().starts_with("labby::dispatch::logs") {
            return;
        }

        let Ok(system) = super::client::require_system() else {
            return;
        };

        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        let raw = RawLogEvent {
            ts: Some(now_ms()),
            level: Some(
                match *metadata.level() {
                    tracing::Level::TRACE => "trace",
                    tracing::Level::DEBUG => "debug",
                    tracing::Level::INFO => "info",
                    tracing::Level::WARN => "warn",
                    tracing::Level::ERROR => "error",
                }
                .to_string(),
            ),
            subsystem: visitor
                .subsystem
                .or_else(|| Some("core_runtime".to_string())),
            surface: visitor.surface,
            action: visitor.action,
            message: visitor
                .message
                .unwrap_or_else(|| metadata.target().to_string()),
            request_id: visitor.request_id,
            session_id: visitor.session_id,
            correlation_id: visitor.correlation_id,
            trace_id: None,
            span_id: None,
            instance: visitor.instance,
            auth_flow: visitor.auth_flow,
            outcome_kind: visitor.outcome_kind,
            fields_json: serde_json::Value::Object(visitor.extra),
            source_kind: None,
            source_node_id: None,
            source_device_id: None,
            actor_key: visitor.actor_key,
            ingest_path: Some("tracing".to_string()),
            upstream_event_id: None,
        };

        drop(system.try_ingest(raw));
    }
}

#[derive(Default)]
struct FieldVisitor {
    message: Option<String>,
    subsystem: Option<String>,
    surface: Option<String>,
    action: Option<String>,
    request_id: Option<String>,
    session_id: Option<String>,
    correlation_id: Option<String>,
    instance: Option<String>,
    auth_flow: Option<String>,
    outcome_kind: Option<String>,
    actor_key: Option<String>,
    extra: serde_json::Map<String, serde_json::Value>,
}

impl tracing::field::Visit for FieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.set_field(field.name(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.set_field(field.name(), format!("{value:?}"));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.extra.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.extra.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.extra
            .insert(field.name().to_string(), serde_json::Value::Bool(value));
    }
}

impl FieldVisitor {
    fn set_field(&mut self, name: &str, value: String) {
        match name {
            "message" => self.message = Some(value),
            "subsystem" => self.subsystem = Some(value),
            "surface" => self.surface = Some(value),
            "action" => self.action = Some(value),
            "request_id" => self.request_id = Some(value),
            "session_id" => self.session_id = Some(value),
            "correlation_id" => self.correlation_id = Some(value),
            "instance" => self.instance = Some(value),
            "auth_flow" => self.auth_flow = Some(value),
            "outcome_kind" => self.outcome_kind = Some(value),
            "actor_key" => self.actor_key = Some(value),
            _ => {
                self.extra
                    .insert(name.to_string(), serde_json::Value::String(value));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_bearer_replaces_token() {
        let msg = "Authorization: Bearer secret-value";
        let out = redact_secrets(msg);
        assert!(!out.contains("secret-value"));
        assert!(!out.contains("Bearer "));
        assert!(out.contains("<redacted-auth>"));
    }

    #[test]
    fn scrub_json_replaces_authorization_key() {
        let v = serde_json::json!({"authorization":"Bearer secret-value","safe":"ok"});
        let out = scrub_json(v).to_string();
        assert!(!out.contains("secret-value"));
        assert!(out.contains("\"safe\":\"ok\""));
    }

    #[test]
    fn scrub_json_preserves_token_count_metrics() {
        let v = serde_json::json!({
            "input_tokens": 12,
            "output_tokens": "34",
            "total_tokens": 46,
            "access_token": "secret-value",
        });
        let out = scrub_json(v);
        assert_eq!(out["input_tokens"], 12);
        assert_eq!(out["output_tokens"], "34");
        assert_eq!(out["total_tokens"], 46);
        assert_eq!(out["access_token"], "<redacted>");
    }

    #[test]
    fn canonicalize_preserves_runtime_surfaces_used_by_logs() {
        for (raw, expected) in [
            ("acp", Surface::Acp),
            ("dispatch", Surface::Dispatch),
            ("node", Surface::Node),
        ] {
            let event = canonicalize(RawLogEvent {
                surface: Some(raw.to_string()),
                message: "runtime event".to_string(),
                ..RawLogEvent::default()
            });
            assert_eq!(event.surface, expected);
        }
    }

    #[test]
    fn scrub_json_handles_nested_and_arrays() {
        let v = serde_json::json!({"outer":{"api_key":"abc"},"list":["Bearer xyz"]});
        let out = scrub_json(v).to_string();
        assert!(!out.contains("\"abc\""));
        assert!(!out.contains("Bearer xyz"));
    }

    // ── Extended redaction coverage ───────────────────────────────────────────

    #[test]
    fn redact_key_value_token_param() {
        let msg = "request failed: token=super-secret-value, retry=true";
        let out = redact_secrets(msg);
        assert!(
            !out.contains("super-secret-value"),
            "token value must be redacted: {out}"
        );
        assert!(
            out.contains("token=<redacted>"),
            "key prefix must survive: {out}"
        );
        assert!(
            out.contains("retry=true"),
            "unrelated key must survive: {out}"
        );
    }

    #[test]
    fn redact_key_value_api_key_param() {
        let msg = "api_key=abc123 sent to upstream";
        let out = redact_secrets(msg);
        assert!(
            !out.contains("abc123"),
            "api_key value must be redacted: {out}"
        );
        assert!(
            out.contains("api_key=<redacted>"),
            "key must survive: {out}"
        );
        assert!(
            out.contains("sent to upstream"),
            "surrounding text must survive: {out}"
        );
    }

    #[test]
    fn redact_github_personal_token() {
        let token = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
        let msg = format!("authenticating with {token}");
        let out = redact_secrets(&msg);
        assert!(!out.contains(token), "GitHub token must be redacted: {out}");
        assert!(
            out.contains("<redacted-token>"),
            "placeholder must appear: {out}"
        );
    }

    #[test]
    fn redact_openai_sk_key() {
        let key = "sk-ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        let msg = format!("using key {key} for request");
        let out = redact_secrets(&msg);
        assert!(!out.contains(key), "sk- key must be redacted: {out}");
        assert!(
            out.contains("<redacted-token>"),
            "placeholder must appear: {out}"
        );
    }

    #[test]
    fn plain_uuid_survives_redaction() {
        // Correlation IDs are UUIDs and must never be redacted.
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let msg = format!("request_id={uuid} processed");
        let out = redact_secrets(&msg);
        assert!(
            out.contains(uuid),
            "UUID correlation ID must NOT be redacted: {out}"
        );
    }

    #[test]
    fn short_sk_prefix_not_redacted() {
        // Short sk- strings (e.g. package names) must not be redacted.
        let msg = "package sk-node installed";
        let out = redact_secrets(&msg);
        // "sk-node" is only 4 chars after "sk-", well below the 20-char threshold.
        assert!(
            out.contains("sk-node"),
            "short sk- word must survive: {out}"
        );
    }
}
